//! Call signaling, recipient and takeover fences.
use super::{Arc, BTreeMap, ConfirmationFailure, Event, Mutex, egui, mpsc, watch};

// Separate revisions retain interruptions even when a fast Resume coalesces the online watch.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct VoiceLifecycle {
	pub(super) interruption: u64,
	pub(super) session: u64,
}

impl VoiceLifecycle {
	pub(super) fn observe(&mut self, event: &Event) -> bool {
		let reset = event.ready_navigation().is_some() || matches!(event, Event::Resync);
		if reset || matches!(event, Event::Disconnected) {
			self.interruption = self.interruption.wrapping_add(1);
			if reset {
				self.session = self.session.wrapping_add(1);
			}
			true
		} else {
			false
		}
	}
}

pub(super) fn apply_voice_lifecycle(
	previous: &mut VoiceLifecycle,
	current: VoiceLifecycle,
	active: &mut Option<(model::Id, u64, bool)>,
	calls: &RecipientCalls,
	channels: &BTreeMap<model::Id, Vec<model::Id>>,
) -> bool {
	if *previous == current {
		return false;
	}
	let reset = previous.session != current.session;
	*previous = current;
	if reset
		|| active.is_some_and(|(channel, request, _)| {
			channels.contains_key(&channel)
				&& calls.as_ref().is_none_or(|call| {
					(call.channel, call.request, call.confirmed) != (channel, Some(request), true)
				})
		}) {
		*active = None;
	}
	true
}

pub(super) async fn wait_for_voice_interruption(
	mut lifecycle: watch::Receiver<VoiceLifecycle>,
	revision: u64,
) {
	loop {
		if lifecycle.borrow_and_update().interruption != revision {
			return;
		}
		if lifecycle.changed().await.is_err() {
			return;
		}
	}
}

pub(super) async fn wait_for_soundboard_owner(
	mut owner: watch::Receiver<Option<(model::Id, u64)>>,
	expected: (model::Id, u64),
) {
	loop {
		if *owner.borrow_and_update() != Some(expected) {
			return;
		}
		if owner.changed().await.is_err() {
			return;
		}
	}
}

pub(super) async fn scoped_soundboard_event(
	operation: impl std::future::Future<Output = Event>,
	owner: watch::Receiver<Option<(model::Id, u64)>>,
	expected: (model::Id, u64),
	lifecycle: watch::Receiver<VoiceLifecycle>,
	revision: u64,
) -> Option<Event> {
	tokio::select! {
		biased;
		_ = wait_for_soundboard_owner(owner, expected) => None,
		_ = wait_for_voice_interruption(lifecycle, revision) => None,
		event = operation => Some(event),
	}
}

pub(super) fn private_call(channel: &model::Channel) -> bool {
	channel.guild.is_none()
		&& ((channel.kind == 1 && channel.recipients.len() == 1)
			|| (channel.kind == 3
				&& channel.recipients.len() < client_core::voice::MAX_PARTICIPANTS))
}

// Match the core's 64-call discovery cache, plus one explicitly joined attempt.
// Each record owns two <=64-ID buffers: 65 KiB (66,560 bytes) of IDs total.
// Also bounded: observed Vec capacity * size_of::<Option<RecipientCall>>(),
// the active/Vec header in RecipientCalls, and the shared Arc/Mutex metadata.
pub(super) struct RecipientCalls {
	pub(super) active: Option<RecipientCall>,
	pub(super) observed: Vec<Option<RecipientCall>>,
}

impl Default for RecipientCalls {
	fn default() -> Self {
		Self {
			active: None,
			observed: Vec::with_capacity(client_core::voice::MAX_DM_CALLS),
		}
	}
}

impl RecipientCalls {
	pub(super) fn as_ref(&self) -> Option<&RecipientCall> {
		self.active.as_ref()
	}
	pub(super) fn observe_scoped(
		&mut self,
		event: &Event,
		owner: model::Id,
		channels: &BTreeMap<model::Id, Vec<model::Id>>,
	) -> (bool, Option<(model::Id, u64)>) {
		let previous = self
			.as_ref()
			.and_then(|call| call.request.map(|request| (call.channel, request)));
		let invalidates = self.observe(event, owner, channels);
		let released = (self.active.is_none() && !matches!(event, Event::Disconnected))
			.then_some(previous)
			.flatten();
		(invalidates, released)
	}
	pub(super) fn join(&mut self, channel: model::Id, request: u64, private: bool) {
		if self
			.active
			.as_ref()
			.is_none_or(|call| call.channel != channel)
		{
			self.active = self
				.observed
				.iter()
				.flatten()
				.find(|call| call.channel == channel)
				.map(RecipientCall::snapshot);
		}
		RecipientCall::join(&mut self.active, channel, request, private);
	}
	// Only events affecting the current action invalidate its HTTP worker.
	pub(super) fn observe(
		&mut self,
		event: &Event,
		owner: model::Id,
		channels: &BTreeMap<model::Id, Vec<model::Id>>,
	) -> bool {
		use client_core::voice::Event as V;
		let invalidates = self.active.as_ref().is_some_and(|call| {
			event.ready_navigation().is_some()
				|| !channels.contains_key(&call.channel)
				|| match event {
					Event::Disconnected | Event::Resync => true,
					Event::ChannelCreated(channel) => channel.id == call.channel,
					Event::ChannelChanged(patch) => patch.id == call.channel,
					Event::Unavailable(channel)
					| Event::RecipientAdded { channel, .. }
					| Event::RecipientRemoved { channel, .. }
					| Event::Voice(V::Call { channel, .. } | V::Deleted { channel }) => *channel == call.channel,
					Event::Voice(
						V::TakenOver { channel, request } | V::Departed { channel, request },
					) => (*channel, Some(*request)) == (call.channel, call.request),
					Event::Voice(V::State {
						guild,
						channel,
						user,
						..
					}) => {
						if *user == owner {
							call.joined.contains(user)
								&& (guild.is_some() || *channel != Some(call.channel))
						} else {
							*channel == Some(call.channel) || call.joined.contains(user)
						}
					}
					_ => false,
				}
		});
		if event.ready_navigation().is_some() || matches!(event, Event::Resync) {
			self.active = None;
			self.observed.clear();
			return invalidates;
		}
		if matches!(event, Event::Disconnected) {
			// Resume retains the confirmed local call. Discovery and unconfirmed
			// attempts are discarded; pending HTTP is invalidated independently.
			self.active = self.active.take().filter(|call| call.confirmed);
			self.observed.clear();
			return invalidates;
		}
		if self.active.is_some() {
			RecipientCall::observe(&mut self.active, event, owner, channels);
		}
		for call in &mut self.observed {
			RecipientCall::observe(call, event, owner, channels);
		}
		self.observed.retain(Option::is_some);
		if let Event::Voice(V::Call {
			channel,
			unavailable: false,
			..
		}) = event && channels.contains_key(channel)
			&& !self
				.observed
				.iter()
				.flatten()
				.any(|call| call.channel == *channel)
		{
			let mut call = None;
			RecipientCall::observe(&mut call, event, owner, channels);
			if call.is_some() {
				if self.observed.len() == client_core::voice::MAX_DM_CALLS {
					self.observed.remove(0);
				}
				self.observed.push(call);
			}
		}
		invalidates
	}
}

// One observed/current DM call retains metadata: two 64-ID buffers (1024 bytes).
pub(super) struct RecipientCall {
	pub(super) channel: model::Id,
	pub(super) request: Option<u64>,
	pub(super) confirmed: bool,
	pub(super) ringing: Option<Vec<model::Id>>,
	pub(super) joined: Vec<model::Id>,
}

impl RecipientCall {
	pub(super) fn new(channel: model::Id, request: u64) -> Self {
		Self {
			channel,
			request: Some(request),
			confirmed: false,
			ringing: None,
			joined: Vec::with_capacity(client_core::voice::MAX_PARTICIPANTS),
		}
	}
	pub(super) fn valid_call(event: &Event, owner: model::Id, members: &[model::Id]) -> bool {
		let Event::Voice(client_core::voice::Event::Call {
			ringing,
			participants,
			..
		}) = event
		else {
			return true;
		};
		ringing.as_ref().is_none_or(|ids| {
			ids.len() <= client_core::voice::MAX_PARTICIPANTS
				&& ids.iter().enumerate().all(|(index, id)| {
					(*id == owner || members.contains(id)) && !ids[..index].contains(id)
				})
		}) && participants.as_ref().is_none_or(|peers| {
			peers.len() <= client_core::voice::MAX_PARTICIPANTS
				&& peers.iter().enumerate().all(|(index, peer)| {
					(peer.user == owner || members.contains(&peer.user))
						&& !peers[..index].iter().any(|old| old.user == peer.user)
				})
		})
	}
	pub(super) fn snapshot(&self) -> Self {
		let mut copy = Self::new(self.channel, 0);
		copy.request = None;
		copy.joined.extend(&self.joined);
		if let Some(ids) = &self.ringing {
			let mut retained = Vec::with_capacity(client_core::voice::MAX_PARTICIPANTS);
			retained.extend(ids);
			copy.ringing = Some(retained);
		}
		copy
	}
	pub(super) fn join(active: &mut Option<Self>, channel: model::Id, request: u64, private: bool) {
		let retained = active.take().filter(|call| call.channel == channel);
		if private {
			let mut call = retained.unwrap_or_else(|| Self::new(channel, request));
			call.request = Some(request);
			call.confirmed = false;
			*active = Some(call);
		}
	}
	pub(super) fn allows(
		&self,
		channel: model::Id,
		request: u64,
		recipient: model::Id,
		stop: bool,
	) -> bool {
		self.channel == channel
			&& self.request == Some(request)
			&& self.confirmed
			&& self.ringing.as_ref().is_some_and(|ringing| {
				ringing.contains(&recipient) == stop && (stop || !self.joined.contains(&recipient))
			})
	}
	pub(super) fn observe(
		active: &mut Option<Self>,
		event: &Event,
		owner: model::Id,
		channels: &BTreeMap<model::Id, Vec<model::Id>>,
	) {
		use client_core::voice::Event as V;
		if event.ready_navigation().is_some() || matches!(event, Event::Resync) {
			*active = None;
			return;
		}
		if matches!(event, Event::Disconnected) {
			*active = active.take().filter(|call| call.confirmed);
			return;
		}
		// Preserve one validated pre-join observation for the existing-call flow.
		// An active attempt never admits another channel's metadata.
		if let Event::Voice(V::Call {
			channel,
			unavailable: false,
			..
		}) = event && channels
			.get(channel)
			.is_some_and(|members| Self::valid_call(event, owner, members))
			&& active.is_none()
		{
			let mut call = Self::new(*channel, 0);
			call.request = None;
			*active = Some(call);
		}
		let Some(call) = active.as_mut() else {
			return;
		};
		let Some(members) = channels.get(&call.channel) else {
			*active = None;
			return;
		};

		match event {
			Event::Voice(V::SessionConfirmed {
				channel, request, ..
			}) if (*channel, Some(*request)) == (call.channel, call.request) => {
				call.confirmed = true;
			}
			Event::Voice(V::TakenOver { channel, request } | V::Departed { channel, request })
				if (*channel, Some(*request)) == (call.channel, call.request) =>
			{
				*active = None;
			}
			Event::Voice(V::Deleted { channel }) | Event::Unavailable(channel)
				if *channel == call.channel =>
			{
				*active = None;
			}
			Event::Voice(V::Call {
				channel,
				ringing,
				participants,
				unavailable,
			}) if *channel == call.channel => {
				if *unavailable {
					*active = None;
					return;
				}
				if !Self::valid_call(event, owner, members) {
					return;
				}
				if let Some(ids) = ringing {
					let retained = call.ringing.get_or_insert_with(|| {
						Vec::with_capacity(client_core::voice::MAX_PARTICIPANTS)
					});
					retained.clear();
					retained.extend(ids);
				}
				if let Some(peers) = participants {
					call.joined.clear();
					call.joined.extend(peers.iter().map(|peer| peer.user));
				}
			}
			Event::Voice(V::State {
				guild,
				channel,
				user,
				..
			}) => {
				if *user == owner
					&& call.joined.contains(user)
					&& (guild.is_some() || *channel != Some(call.channel))
				{
					// Reject already queued recipient actions before departure is acknowledged.
					call.confirmed = false;
				}
				call.joined.retain(|peer| peer != user);
				if guild.is_none()
					&& *channel == Some(call.channel)
					&& (*user == owner || members.contains(user))
				{
					if call.joined.len() == client_core::voice::MAX_PARTICIPANTS {
						*active = None;
						return;
					}
					call.joined.push(*user);
				}
			}
			Event::RecipientRemoved { channel, user } if *channel == call.channel => {
				if *user == owner {
					*active = None;
					return;
				}
				call.joined.retain(|peer| peer != user);
				if let Some(ringing) = &mut call.ringing {
					ringing.retain(|peer| peer != user);
				}
			}
			_ => {}
		}
	}
}

// Reserve the fixed byte budget once; later member additions must not grow capacity.
pub(super) fn recipient_ids(channel: &model::Channel) -> Vec<model::Id> {
	let mut recipients = Vec::with_capacity(client_core::voice::MAX_PARTICIPANTS);
	recipients.extend(
		channel
			.recipients
			.iter()
			.take(client_core::voice::MAX_PARTICIPANTS - 1)
			.map(|user| user.id),
	);
	recipients
}

// A recipient write never allocates media or changes the once-only initial ring state.
pub(super) fn recipient_action(
	channel: model::Id,
	request: u64,
	recipient: model::Id,
	owner: model::Id,
	active: Option<(model::Id, u64, bool)>,
	recipients: Option<&[model::Id]>,
) -> bool {
	recipient != owner
		&& active.is_some_and(|(id, r, _)| id == channel && r == request)
		&& recipients.is_some_and(|ids| {
			ids.len() < client_core::voice::MAX_PARTICIPANTS && ids.contains(&recipient)
		})
}

pub(super) fn recipient_write_allowed(
	control: client_core::voice::Command,
	calls: &RecipientCalls,
	channels: &BTreeMap<model::Id, Vec<model::Id>>,
) -> bool {
	let client_core::voice::Command::RingRecipient {
		channel,
		request,
		recipient,
		stop,
	} = control
	else {
		return false;
	};
	channels.get(&channel).is_some_and(|ids| {
		ids.len() < client_core::voice::MAX_PARTICIPANTS && ids.contains(&recipient)
	}) && calls
		.as_ref()
		.is_some_and(|call| call.allows(channel, request, recipient, stop))
}

// Recheck the exact target on revisions; unrelated peer controls do not cancel HTTP.
pub(super) async fn wait_for_recipient_invalidation(
	mut members: watch::Receiver<u64>,
	mut available: watch::Receiver<bool>,
	control: client_core::voice::Command,
	calls: Arc<Mutex<RecipientCalls>>,
	channels: Arc<Mutex<BTreeMap<model::Id, Vec<model::Id>>>>,
) {
	loop {
		members.borrow_and_update();
		let online = *available.borrow_and_update();
		let allowed = {
			let Ok(channels) = channels.lock() else {
				return;
			};
			let Ok(calls) = calls.lock() else {
				return;
			};
			recipient_write_allowed(control, &calls, &channels)
		};
		if !allowed || !online {
			return;
		}
		tokio::select! {
			changed=members.changed()=>if changed.is_err() {return;},
			changed=available.changed()=>if changed.is_err() {return;},
		}
	}
}

// Latest scoped invalidation is rechecked at dispatch, independent of select branch order.
pub(super) fn release_taken_over(
	active: &mut Option<(model::Id, u64, bool)>,
	latest: Option<(model::Id, u64)>,
) -> bool {
	if active.is_some_and(|(channel, request, _)| latest == Some((channel, request))) {
		*active = None;
		true
	} else {
		false
	}
}

// A duplicate queued Join must not recreate the invalidated local request.
pub(super) fn join_taken_over(
	control: client_core::voice::Command,
	latest: Option<(model::Id, u64)>,
) -> bool {
	matches!(control, client_core::voice::Command::Join { channel, request, .. } if latest == Some((channel, request)))
}

pub(super) async fn wait_for_takeover(
	mut takeover: watch::Receiver<Option<(model::Id, u64)>>,
	scope: (model::Id, u64),
) {
	loop {
		let matching = *takeover.borrow_and_update() == Some(scope);
		if matching || takeover.changed().await.is_err() {
			return;
		}
	}
}

// Queue pressure must not end text signaling or allow a fresh Join ahead of local cleanup.
pub(super) fn queue_abandonment(
	sender: &mpsc::Sender<client_core::voice::Command>,
	pending: &mut Option<client_core::voice::Command>,
	control: client_core::voice::Command,
	owner: Option<(model::Id, u64)>,
) {
	if let client_core::voice::Command::AbandonSession { channel, request } = control
		&& owner.is_some_and(|scope| scope != (channel, request))
	{
		return;
	}
	debug_assert!(matches!(
		control,
		client_core::voice::Command::AbandonSession { .. }
	));
	if pending.is_some() {
		// A new attempt is blocked until this first release is queued; stale releases cannot replace it.
		return;
	}
	if let Err(mpsc::error::TrySendError::Full(control)) = sender.try_send(control) {
		*pending = Some(control);
	}
}

// A stale candidate cannot fail its replacement: the desktop rechecks the tagged revision.
pub(super) fn report_confirmation_failure(
	sender: &watch::Sender<Option<ConfirmationFailure>>,
	generation: u64,
	event: client_core::voice::Event,
	owner: Option<(model::Id, u64, bool)>,
	wake: &egui::Context,
) {
	let client_core::voice::Event::SessionConfirmationFailed {
		channel,
		request,
		revision,
		message,
	} = event
	else {
		unreachable!("only candidate confirmation failures use this report");
	};
	if !owner.is_some_and(|(id, attempt, _)| (id, attempt) == (channel, request)) {
		return;
	}
	let failure = ConfirmationFailure {
		generation,
		channel,
		request,
		revision,
		message,
	};
	if sender.send_if_modified(|current| {
		if current.is_some_and(|old| {
			(old.generation, old.channel, old.request) == (generation, channel, request)
				&& old.revision >= revision
		}) {
			return false;
		}
		*current = Some(failure);
		true
	}) {
		wake.request_repaint();
	}
}

pub(super) fn queue_confirmation(
	sender: &mpsc::Sender<client_core::voice::Command>,
	control: client_core::voice::Command,
	owner: Option<(model::Id, u64, bool)>,
	online: bool,
) -> Option<client_core::voice::Event> {
	let client_core::voice::Command::ConfirmSession {
		channel,
		request,
		revision,
	} = control
	else {
		unreachable!("only local transport confirmations use this admission path");
	};
	let message = if !online {
		"Call confirmation was not sent; voice signaling is disconnected"
	} else if !owner.is_some_and(|(id, attempt, _)| (id, attempt) == (channel, request)) {
		"Call confirmation expired; no action was sent"
	} else {
		match sender.try_send(control) {
			Ok(()) => return None,
			Err(mpsc::error::TrySendError::Full(_)) => {
				"Call confirmation was not sent; the voice queue is full"
			}
			Err(mpsc::error::TrySendError::Closed(_)) => {
				"Call confirmation was not sent; voice signaling is disconnected"
			}
		}
	};
	Some(client_core::voice::Event::SessionConfirmationFailed {
		channel,
		request,
		revision,
		message,
	})
}

// A fresh Join can meet the still-full queue just after local release was queued.
pub(super) fn queue_join(
	sender: &mpsc::Sender<client_core::voice::Command>,
	control: client_core::voice::Command,
	scope: (model::Id, u64),
	owner: &mut Option<(model::Id, u64, bool)>,
) -> Option<client_core::voice::Event> {
	debug_assert!(matches!(control, client_core::voice::Command::Join { .. }));
	let error = sender.try_send(control).err()?;
	release_taken_over(owner, Some(scope));
	Some(client_core::voice::Event::Failed {
		channel: scope.0,
		request: scope.1,
		message: match error {
			mpsc::error::TrySendError::Full(_) => "Call join was not sent; the voice queue is full",
			mpsc::error::TrySendError::Closed(_) => {
				"Call join was not sent; voice signaling is disconnected"
			}
		},
	})
}

pub(super) fn abandonment_blocks_join(
	pending: Option<client_core::voice::Command>,
	control: client_core::voice::Command,
) -> bool {
	pending.is_some() && matches!(control, client_core::voice::Command::Join { .. })
}

// Ring only after the media adapter confirms transport allocation, and only once per current call.
pub(super) fn ring_action(
	control: client_core::voice::Command,
	owner: model::Id,
	active: &mut Option<(model::Id, u64, bool)>,
	dm: bool,
) -> Result<Option<(Option<model::Id>, bool)>, ()> {
	use client_core::voice::Command as V;
	if let V::AbandonSession { channel, request } = control {
		if active.is_some_and(|(id, r, _)| id == channel && r == request) {
			*active = None;
		}
		return Ok(None);
	}
	if let V::Leave { channel, request } = control {
		if active.is_some_and(|(id, r, _)| id == channel && r == request) {
			*active = None;
			return Ok(dm.then_some((None, true)));
		}
		return Ok(None);
	}
	if !dm {
		return match control {
			V::Ring { .. }
			| V::RingRecipient { .. }
			| V::Decline { .. }
			| V::Join { ring: true, .. } => Err(()),
			V::Join {
				channel,
				request,
				ring: false,
				..
			} => {
				*active = Some((channel, request, true));
				Ok(None)
			}
			V::AbandonSession { .. }
			| V::ConfirmSession { .. }
			| V::Sync { .. }
			| V::ChannelInfo { .. }
			| V::Leave { .. }
			| V::SetMute { .. }
			| V::SetCamera { .. }
			| V::StartStream { .. }
			| V::StopStream { .. }
			| V::WatchStream { .. }
			| V::StopWatching { .. } => Ok(None),
		};
	}
	match control {
		V::Join {
			channel,
			request,
			ring,
			..
		} => {
			*active = Some((channel, request, !ring));
			Ok(None)
		}
		V::Ring { channel, request } => {
			let Some((current, current_request, rang)) = active else {
				return Err(());
			};
			if *current != channel || *current_request != request || *rang {
				return Err(());
			}
			*rang = true;
			Ok(Some((None, false)))
		}
		V::Decline { .. } => Ok(Some((Some(owner), true))),
		V::RingRecipient { .. } => Err(()),
		V::AbandonSession { .. }
		| V::ConfirmSession { .. }
		| V::Sync { .. }
		| V::ChannelInfo { .. }
		| V::Leave { .. }
		| V::SetMute { .. }
		| V::SetCamera { .. }
		| V::StartStream { .. }
		| V::StopStream { .. }
		| V::WatchStream { .. }
		| V::StopWatching { .. } => Ok(None),
	}
}
