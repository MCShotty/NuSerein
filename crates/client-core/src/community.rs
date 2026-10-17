//! One bounded community request lane, independent of server settings and message drafts.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{Id, community::*, permissions as p};

#[derive(Default)]
pub struct Community {
	pub events: Option<EventPage>,
	pub stage: Option<StageInstance>,
	pub stage_channel: Option<Id>,
	pub stale: bool,
	pub stage_stale: bool,
	refresh_channel: Option<Id>,
	pub error: Option<&'static str>,
	pub needs_refresh: bool,
	pub completed: u64,
	pub fence: u64,
	pub opened: Option<Id>,
	sequence: u64,
	pending: Option<(Id, u64, Action)>,
}
impl Community {
	pub fn pending(&self) -> bool {
		self.pending.is_some()
	}
	pub fn invalidate(&mut self) {
		self.stale = true;
		self.stage_stale = true;
		if self.pending.as_ref().is_some_and(|(_, _, a)| a.write()) {
			self.needs_refresh = true;
			self.refresh_channel = self
				.pending
				.as_ref()
				.and_then(|(_, _, a)| a.stage_channel());
			self.error = Some("Action outcome is unknown; refresh before another change");
		}
		self.pending = None;
		self.sequence = self.sequence.wrapping_add(1);
	}
	pub fn invalidate_metadata(&mut self, guild: Id, channel: Option<Id>) {
		if self.events.as_ref().is_some_and(|p| p.guild == guild) {
			self.stale = true;
		}
		if channel.is_some() && self.stage_channel == channel {
			self.stage_stale = true;
		}
		if self
			.pending
			.as_ref()
			.is_some_and(|(g, _, a)| *g == guild && !a.write())
		{
			self.invalidate();
		}
	}
}
pub struct Event {
	pub guild: Id,
	pub request: u64,
	pub result: Result<Outcome, Failure>,
}
impl State {
	pub(crate) fn retire_community_call(&mut self, channel: Id, request: u64) {
		if self.community.pending.as_ref().is_some_and(|(_,_,action)|matches!(action,
			Action::StageVoice {channel:c,call_request:r,..} | Action::GetStage {channel:c,call_request:Some(r)} if *c==channel && *r==request)) {
			self.community.invalidate();
		}
	}
	pub(crate) fn stop_stage_participation(&mut self, channel: Id) {
		if let Some(call) = &mut self.voice.active
			&& call.channel == channel
			&& call.media_mode == crate::voice::MediaMode::Stage
		{
			call.stage = StageParticipation::Unknown;
			call.stage_consent = false;
			call.stage_revision = call.stage_revision.wrapping_add(1);
			call.muted = true;
			call.camera = false;
		}
	}
	pub(crate) fn prune_community(&mut self) {
		if let Some(channel) = self
			.voice
			.active
			.as_ref()
			.filter(|c| c.media_mode == crate::voice::MediaMode::Stage)
			.map(|c| c.channel)
		{
			let blocked = !self.can_speak(channel);
			if let Some(call) = &mut self.voice.active {
				call.stage_revision = call.stage_revision.wrapping_add(1);
				if blocked {
					call.stage_consent = false;
					call.muted = true;
					call.camera = false;
				}
			}
		}
		if let Some(mut page) = self.community.events.take() {
			if self.can_browse_events(page.guild) {
				page.events.retain(|e| self.event_visible(e));
				self.community.events = Some(page);
			}
		}
		if self.community.stage.as_ref().is_some_and(|s| {
			!self.can_view(s.channel)
				|| self
					.channel(s.channel)
					.is_none_or(|c| c.guild != Some(s.guild) || c.kind != 13)
		}) {
			self.community.stage = None;
			self.community.stage_channel = None;
		}
	}

	pub fn can_browse_events(&self, guild: Id) -> bool {
		self.guild(guild).is_some()
			&& self.user.is_some()
			&& (self.demo || self.auth == AuthState::Authenticated)
	}
	pub fn is_stage(&self, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|c| c.guild.is_some() && c.kind == 13)
	}
	pub fn stage_moderator(&self, channel: Id) -> bool {
		self.is_stage(channel)
			&& self.permission(
				channel,
				p::VIEW_CHANNEL
					| p::CONNECT | p::MANAGE_CHANNELS
					| p::MUTE_MEMBERS
					| p::MOVE_MEMBERS,
			) == Some(true)
	}
	pub fn event_visible(&self, event: &ScheduledEvent) -> bool {
		self.can_browse_events(event.guild)
			&& event.draft.channel.is_none_or(|c| {
				self.channel(c)
					.is_some_and(|c| c.guild == Some(event.guild))
					&& self.can_view(c)
			})
	}
	fn event_permission(&self, guild: Id, draft: &EventDraft, own: Option<bool>) -> bool {
		let bits = match own {
			None => p::CREATE_EVENTS,
			Some(false) => p::MANAGE_EVENTS,
			Some(true) => 0,
		};
		let permission = |bits| {
			if let Some(channel) = draft.channel {
				self.permission(channel, bits) == Some(true)
			} else {
				self.guild_permission(guild, bits)
			}
		};
		let capability = if own == Some(true) {
			permission(p::CREATE_EVENTS) || permission(p::MANAGE_EVENTS)
		} else {
			permission(bits)
		};
		capability
			&& match draft.entity_type {
				1 => draft.channel.is_some_and(|c| {
					self.channel(c).is_some_and(|c| c.guild == Some(guild))
						&& self.stage_moderator(c)
				}),
				2 => draft.channel.is_some_and(|c| {
					self.channel(c)
						.is_some_and(|c| c.kind == 2 && c.guild == Some(guild))
						&& self.permission(c, p::VIEW_CHANNEL | p::CONNECT) == Some(true)
				}),
				3 => draft.channel.is_none(),
				_ => false,
			}
	}
	pub fn can_create_event(&self, guild: Id, draft: &EventDraft) -> bool {
		self.can_browse_events(guild) && self.event_permission(guild, draft, None)
	}
	pub fn can_manage_event(&self, event: &ScheduledEvent) -> bool {
		self.event_visible(event)
			&& self.event_permission(
				event.guild,
				&event.draft,
				Some(
					self.user
						.as_ref()
						.is_some_and(|u| Some(u.id) == event.creator),
				),
			)
	}
	fn known_event(&self, guild: Id, id: Id) -> Option<&ScheduledEvent> {
		self.community
			.events
			.as_ref()
			.filter(|p| p.guild == guild)?
			.events
			.iter()
			.find(|e| e.id == id && self.event_visible(e))
	}
	pub fn community_action_allowed(&self, guild: Id, action: &Action) -> bool {
		if !self.can_browse_events(guild) || !action.valid() {
			return false;
		}
		match action {
			Action::ListEvents | Action::GetEvent(_) => true,
			Action::EditOccurrence { before, .. } | Action::RestoreOccurrence { before, .. } => {
				before.guild == guild
					&& self.known_event(guild, before.id) == Some(before)
					&& self.can_manage_event(before)
					&& matches!(before.status, 1 | 2)
			}
			Action::ResetOccurrenceRsvp { event, occurrence }
			| Action::RsvpOccurrence {
				event, occurrence, ..
			} => self.known_event(guild, *event).is_some_and(|e| {
				matches!(e.status, 1 | 2)
					&& occurrence_time(*occurrence).is_some_and(|at| {
						occurrence_id(at) == Some(*occurrence)
							&& e.draft
								.recurrence
								.as_ref()
								.is_some_and(|r| r.matches_occurrence(at))
					})
			}),

			Action::Create(d) => self.can_create_event(guild, d),
			Action::Edit { before, after } => {
				before.guild == guild
					&& self.known_event(guild, before.id) == Some(before)
					&& self.can_manage_event(before)
					&& self.event_permission(
						guild,
						after,
						Some(
							self.user
								.as_ref()
								.is_some_and(|u| Some(u.id) == before.creator),
						),
					) && matches!(before.status, 1 | 2)
			}
			Action::Status { event, status } => self.known_event(guild, *event).is_some_and(|e| {
				self.can_manage_event(e) && matches!((e.status, *status), (1, 2 | 4) | (2, 3))
			}),
			Action::Delete(id) => self
				.known_event(guild, *id)
				.is_some_and(|e| self.can_manage_event(e)),
			Action::Rsvp { event, .. } => self
				.known_event(guild, *event)
				.is_some_and(|e| matches!(e.status, 1 | 2)),
			Action::GetStage {
				channel,
				call_request,
			} => {
				if call_request.is_some_and(|r| {
					self.voice
						.active
						.as_ref()
						.is_none_or(|c| c.channel != *channel || c.request != r)
				}) {
					return false;
				}
				self.channel(*channel)
					.is_some_and(|c| c.guild == Some(guild))
					&& self.is_stage(*channel)
					&& self.can_view(*channel)
			}
			Action::CreateStage { channel, event, .. } => {
				self.channel(*channel)
					.is_some_and(|c| c.guild == Some(guild))
					&& self.stage_moderator(*channel)
					&& event.is_none_or(|id| {
						self.known_event(guild, id).is_some_and(|e| {
							e.draft.channel == Some(*channel)
								&& e.draft.entity_type == 1
								&& e.status == 1
						})
					})
			}
			Action::EditStage { channel, .. } | Action::EndStage(channel) => {
				self.stage_moderator(*channel)
					&& self
						.community
						.stage
						.as_ref()
						.is_some_and(|s| s.guild == guild && s.channel == *channel)
			}
			Action::StageVoice {
				channel,
				call_request,
				change,
			} => {
				let Some(call) = self.voice.active.as_ref().filter(|c| {
					c.channel == *channel
						&& c.media_mode == crate::voice::MediaMode::Stage
						&& c.guild == Some(guild)
						&& c.request == *call_request
						&& matches!(
							c.phase,
							crate::voice::Phase::Connected | crate::voice::Phase::Waiting
						)
				}) else {
					return false;
				};
				if !self.is_stage(*channel) || (!self.demo && !self.can_call(*channel)) {
					return false;
				}
				match change {
					StageVoiceChange::RequestToSpeak(requested) => {
						!*requested
							|| (self.permission(*channel, p::REQUEST_TO_SPEAK) == Some(true)
								&& matches!(call.stage, StageParticipation::Audience { .. }))
					}
					StageVoiceChange::AcceptInvitation => matches!(
						call.stage,
						StageParticipation::Invited | StageParticipation::Speaker
					),
					StageVoiceChange::BecomeSpeaker => {
						self.permission(*channel, p::MUTE_MEMBERS) == Some(true)
					}
					StageVoiceChange::Audience => true,
					StageVoiceChange::Invite(user) | StageVoiceChange::MoveToAudience(user) => {
						self.permission(*channel, p::MUTE_MEMBERS) == Some(true)
							&& self.user.as_ref().is_some_and(|u| u.id != *user)
							&& call.participants.iter().any(|p| p.user == *user)
					}
				}
			}
		}
	}
	pub fn request_community(&mut self, guild: Id, action: Action) -> Option<Command> {
		if self.community.pending()
			|| (!self.demo && !self.gateway_connected)
			|| !self.community_action_allowed(guild, &action)
			|| (action.write()
				&& (self.community.needs_refresh
					|| if action.stage_channel().is_some() {
						self.community.stage_stale
					} else {
						self.community.stale
					})) {
			return None;
		}
		self.community.sequence = self.community.sequence.wrapping_add(1).max(1);
		let request = self.community.sequence;
		if let Action::StageVoice {
			change: StageVoiceChange::Audience,
			..
		} = &action
			&& let Some(call) = &mut self.voice.active
		{
			call.stage = StageParticipation::Audience { requested: false };
			call.stage_consent = false;
			call.stage_revision = call.stage_revision.wrapping_add(1);
			call.muted = true;
			call.camera = false;
		}
		if let Action::StageVoice {
			change: StageVoiceChange::AcceptInvitation | StageVoiceChange::BecomeSpeaker,
			..
		} = &action
			&& let Some(call) = &mut self.voice.active
		{
			call.stage_consent = true;
			call.stage_revision = call.stage_revision.wrapping_add(1);
		}
		self.community.error = None;
		self.community.pending = Some((guild, request, action.clone()));
		Some(Command::Community {
			guild,
			fence: self.community.fence,
			user: self.user.as_ref()?.id,
			request,
			action: Box::new(action),
		})
	}
	pub fn community_command_allowed(&self, guild: Id, request: u64, action: &Action) -> bool {
		self.community
			.pending
			.as_ref()
			.is_some_and(|(g, r, a)| *g == guild && *r == request && a == action)
			&& self.gateway_connected
			&& self.community_action_allowed(guild, action)
	}
	pub fn close_events(&mut self) {
		self.community.opened = None;
		if self
			.community
			.pending
			.as_ref()
			.is_none_or(|(_, _, a)| !a.write())
		{
			self.community.events = None;
		}
		if self
			.community
			.pending
			.as_ref()
			.is_some_and(|(_, _, a)| !a.write())
		{
			self.community.invalidate();
		}
	}
	pub(crate) fn apply_community(&mut self, event: Event) -> Result<(), &'static str> {
		let Some((guild, request, action)) = self.community.pending.as_ref() else {
			return Ok(());
		};
		if *guild != event.guild || *request != event.request {
			return Ok(());
		}
		let action = action.clone();
		self.community.pending = None;
		// A matching Gateway deletion may retire the instance before its DELETE response.
		// The accepted request still requires the current guild and moderator permissions.
		let allowed = if let Action::EndStage(channel) = &action {
			self.can_browse_events(event.guild)
				&& self
					.channel(*channel)
					.is_some_and(|c| c.guild == Some(event.guild))
				&& self.stage_moderator(*channel)
		} else {
			self.community_action_allowed(event.guild, &action)
		};
		if !allowed {
			self.community.stale = true;
			if action.write() {
				self.community.needs_refresh = true;
				self.community.refresh_channel = action.stage_channel();
				self.community.error = Some("Action scope changed; refresh to confirm its outcome");
			}
			return Ok(());
		}
		let result = event.result;
		if let Err(failure) = result {
			if let Action::StageVoice {
				channel,
				change: StageVoiceChange::AcceptInvitation | StageVoiceChange::BecomeSpeaker,
				..
			} = &action
			{
				self.stop_stage_participation(*channel);
			}
			if failure.ends_session() && failure != Failure::Capacity {
				self.fail(failure);
				return Ok(());
			}
			self.community.error = Some(failure.label());
			if action.write()
				&& matches!(
					failure,
					Failure::Ambiguous | Failure::Network | Failure::Protocol
				) {
				self.community.needs_refresh = true;
				self.community.refresh_channel = action.stage_channel();
				self.community.stale = true;
				self.community.stage_stale = true;
			}
			return Ok(());
		}
		let outcome = result.unwrap();
		if outcome.bytes() > MAX_BYTES
			|| !self.community_outcome_expected(event.guild, &action, &outcome)
		{
			self.community.error =
				Some("Community response did not match the request; refresh to continue");
			self.community.needs_refresh = action.write();
			self.community.refresh_channel = action.stage_channel();
			self.community.stale = true;
			self.community.stage_stale = true;
			return Ok(());
		}
		match outcome {
			Outcome::Events(mut page) => {
				page.events.retain(|e| self.event_visible(e));
				if self.community.opened == Some(event.guild) {
					self.community.events = Some(page);
				}
				self.community.stale = false;
				if self.community.refresh_channel.is_none() {
					self.community.needs_refresh = false;
				}
			}
			Outcome::Event(event) => {
				if self.event_visible(&event) && self.community.opened == Some(event.guild) {
					let page = self.community.events.get_or_insert_with(|| EventPage {
						guild: event.guild,
						events: vec![],
						subscriptions: None,
						subscriptions_complete: false,
					});
					page.events.retain(|e| e.id != event.id);
					if page.events.len() < MAX_EVENTS {
						page.events.push(event);
					} else {
						self.community.stale = true;
					}
				}
				// A detail read cannot reconcile a possibly accepted create or changes to other events.
			}
			Outcome::Deleted(id) => {
				if let Some(page) = &mut self.community.events {
					page.events.retain(|e| e.id != id);
				}
			}
			Outcome::Rsvp {
				event,
				interested,
				occurrence,
			} => {
				if let Some(page) = &mut self.community.events {
					let user = self.user.as_ref().unwrap().id;
					let subscriptions = page.subscriptions.get_or_insert_with(Vec::new);
					subscriptions.retain(|s| s.event != event || s.occurrence != occurrence);
					if subscriptions.len() < MAX_EVENTS * 4 {
						subscriptions.push(Subscription {
							event,
							occurrence,
							interested,
							user,
						});
					}
				}
			}
			Outcome::OccurrenceRsvpReset { event, occurrence } => {
				if let Some(subscriptions) = self
					.community
					.events
					.as_mut()
					.and_then(|p| p.subscriptions.as_mut())
				{
					subscriptions.retain(|s| s.event != event || s.occurrence != Some(occurrence));
				}
			}

			Outcome::Exception { event, exception } => {
				if let Some(e) = self
					.community
					.events
					.as_mut()
					.and_then(|p| p.events.iter_mut().find(|e| e.id == event))
				{
					e.exceptions.retain(|x| x.id != exception.id);
					if e.exceptions.len() < MAX_EXCEPTIONS {
						e.exceptions.push(exception);
					} else {
						self.community.stale = true;
					}
				}
			}
			Outcome::ExceptionRestored { event, exception } => {
				if let Some(e) = self
					.community
					.events
					.as_mut()
					.and_then(|p| p.events.iter_mut().find(|e| e.id == event))
				{
					e.exceptions.retain(|x| x.id != exception);
				}
			}
			Outcome::Stage {
				channel,
				instance,
				voice,
			} => {
				if instance.is_none() {
					self.stop_stage_participation(channel);
				}
				if let Some(voice) = voice
					&& let Some(call) = &mut self.voice.active
				{
					call.stage = voice.participation;
					call.stage_consent = false;
					call.stage_revision = call.stage_revision.wrapping_add(1);
					call.muted = true;
					call.camera = false;
				}
				self.community.stage_channel = Some(channel);
				self.community.stage = instance;
				if matches!(action, Action::GetStage { .. }) {
					self.community.stage_stale = false;
				}
				if self.community.refresh_channel == Some(channel) {
					self.community.needs_refresh = false;
					self.community.refresh_channel = None;
				}
			}
			Outcome::StageVoice { .. } => {} // Only a current Gateway voice state can grant speaker capture.
		}
		if self
			.community
			.events
			.as_ref()
			.is_some_and(|page| !page.valid())
		{
			self.community.events = None;
			self.community.stale = true;
			self.community.error =
				Some("Event data exceeded the bounded cache; refresh to continue");
		}
		if self.community.opened.is_none() {
			self.community.events = None;
			if action.write() {
				self.status = "Community action confirmed";
			}
		}
		self.community.completed = event.request;
		Ok(())
	}
	fn community_outcome_expected(&self, guild: Id, action: &Action, outcome: &Outcome) -> bool {
		match (action, outcome) {
			(Action::ListEvents, Outcome::Events(page)) => {
				page.guild == guild
					&& page.valid() && page.subscriptions.as_ref().is_none_or(|s| {
					self.user
						.as_ref()
						.is_some_and(|u| s.iter().all(|s| s.user == u.id))
				})
			}
			(Action::GetEvent(id), Outcome::Event(e)) => {
				e.id == *id && e.guild == guild && e.valid()
			}
			(Action::Create(d), Outcome::Event(e)) => {
				e.guild == guild && e.valid() && e.draft == *d
			}
			(Action::Edit { before, after }, Outcome::Event(e)) => {
				e.id == before.id && e.guild == guild && e.valid() && e.draft == *after
			}
			(Action::Status { event, status }, Outcome::Event(e)) => {
				e.id == *event && e.guild == guild && e.valid() && e.status == *status
			}
			(Action::Delete(id), Outcome::Deleted(deleted)) => id == deleted,
			(
				Action::ResetOccurrenceRsvp { event, occurrence },
				Outcome::OccurrenceRsvpReset {
					event: e,
					occurrence: o,
				},
			) => event == e && occurrence == o,
			(
				Action::Rsvp { event, interested },
				Outcome::Rsvp {
					event: confirmed,
					interested: yes,
					occurrence: None,
				},
			) => event == confirmed && interested == yes,
			(
				Action::EditOccurrence {
					before,
					original,
					canceled,
					start,
					end,
				},
				Outcome::Exception { event, exception },
			) => {
				*event == before.id
					&& exception.event == *event
					&& exception.valid()
					&& Some(exception.id) == occurrence_id(*original)
					&& exception.canceled == *canceled
					&& exception.start == *start
					&& exception.end == *end
			}
			(
				Action::RestoreOccurrence { before, exception },
				Outcome::ExceptionRestored {
					event,
					exception: confirmed,
				},
			) => before.id == *event && exception == confirmed,
			(
				Action::RsvpOccurrence {
					event,
					occurrence,
					interested,
				},
				Outcome::Rsvp {
					event: e,
					occurrence: Some(o),
					interested: i,
				},
			) => event == e && occurrence == o && interested == i,

			(
				Action::GetStage {
					channel: c,
					call_request,
				},
				Outcome::Stage {
					channel,
					instance,
					voice,
				},
			) => {
				if voice.is_some() != call_request.is_some()
					|| voice.as_ref().is_some_and(|v| {
						v.channel != *c || self.user.as_ref().is_none_or(|u| u.id != v.user)
					}) {
					return false;
				}
				c == channel
					&& instance
						.as_ref()
						.is_none_or(|s| s.guild == guild && s.channel == *c && s.valid())
			}
			(
				Action::CreateStage {
					channel,
					topic,
					event,
				},
				Outcome::Stage {
					channel: c,
					instance: Some(s),
					voice: None,
				},
			) => {
				channel == c
					&& s.channel == *c
					&& s.guild == guild
					&& s.topic == *topic
					&& s.event == *event
					&& s.valid()
			}
			(
				Action::EditStage { channel, topic },
				Outcome::Stage {
					channel: c,
					instance: Some(s),
					voice: None,
				},
			) => {
				channel == c
					&& s.channel == *c
					&& s.guild == guild
					&& s.topic == *topic
					&& s.valid()
			}
			(
				Action::EndStage(c),
				Outcome::Stage {
					channel,
					instance: None,
					voice: None,
				},
			) => c == channel,
			(Action::StageVoice { channel, .. }, Outcome::StageVoice { channel: c }) => {
				channel == c
			}
			_ => false,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event as CoreEvent, voice};
	fn fixture(bits: u128) -> State {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			user: Some(model::User {
				id: Id(1),
				name: "Synthetic owner".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
			}),
			guilds: vec![model::Guild {
				id: Id(10),
				name: "Synthetic community".into(),
				icon: None,
				default_message_notifications: None,
				stickers: None,
				emojis: None,
			}],
			..Default::default()
		};
		for (id, kind) in [(20, 2), (21, 13)] {
			state.channels.push(model::Channel {
				id: Id(id),
				guild: Some(Id(10)),
				parent_id: None,
				kind,
				name: "Synthetic channel".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			});
			state.permissions.channels.insert(
				Id(id),
				p::Channel {
					id: Id(id),
					guild: Id(10),
					overwrites: Some(vec![]),
				},
			);
		}
		state.permissions.guilds.insert(
			Id(10),
			p::Guild {
				id: Id(10),
				owner: Some(Id(99)),
				roles: Some(vec![p::Role {
					id: Id(10),
					name: "@everyone".into(),
					bits,
					color: 0,
					secondary_color: None,
					tertiary_color: None,
					position: 0,
					hoist: false,
				}]),
				member: Some(p::Member {
					roles: vec![],
					timeout_until: None,
				}),
			},
		);
		state.community.opened = Some(Id(10));
		state
	}
	fn draft() -> EventDraft {
		EventDraft {
			name: "Community meetup".into(),
			description: "Offline synthetic event".into(),
			channel: None,
			entity_type: 3,
			location: "Community park".into(),
			start: 1_900_000_000,
			end: Some(1_900_003_600),
			recurrence: None,
		}
	}
	fn event() -> ScheduledEvent {
		ScheduledEvent {
			id: Id(30),
			guild: Id(10),
			creator: Some(Id(1)),
			status: 1,
			draft: draft(),
			exceptions: vec![],
			user_count: Some(0),
		}
	}
	fn request(state: &mut State, action: Action) -> u64 {
		let Command::Community { request, .. } = state.request_community(Id(10), action).unwrap()
		else {
			panic!()
		};
		request
	}
	fn apply(state: &mut State, request: u64, result: Result<Outcome, Failure>) {
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Community(Event {
				guild: Id(10),
				request,
				result,
			}),
		});
	}
	fn listed(state: &mut State) {
		let r = request(state, Action::ListEvents);
		apply(
			state,
			r,
			Ok(Outcome::Events(EventPage {
				guild: Id(10),
				events: vec![event()],
				subscriptions: Some(vec![]),
				subscriptions_complete: true,
			})),
		);
	}
	fn voice_state(state: &mut State, request: u64, stage: StageParticipation) {
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Voice(voice::Event::State {
				guild: Some(Id(10)),
				member: None,
				server_muted: false,
				server_deafened: false,
				stage,
				request: Some(request),
				channel: Some(Id(21)),
				user: Id(1),
				session: None,
				negotiation_revision: None,
				muted: true,
				deafened: false,
				video: false,
				streaming: false,
			}),
		});
	}
	#[test]
	fn events_create_edit_rsvp_status_delete_and_owner_permissions_complete_the_native_path() {
		let mut state = fixture(p::VIEW_CHANNEL | p::CONNECT | p::CREATE_EVENTS);
		let r = request(&mut state, Action::Create(draft()));
		apply(&mut state, r, Ok(Outcome::Event(event())));
		assert_eq!(state.community.events.as_ref().unwrap().events.len(), 1);
		let r = request(
			&mut state,
			Action::Rsvp {
				event: Id(30),
				interested: true,
			},
		);
		apply(
			&mut state,
			r,
			Ok(Outcome::Rsvp {
				occurrence: None,
				event: Id(30),
				interested: true,
			}),
		);
		assert!(
			state
				.community
				.events
				.as_ref()
				.unwrap()
				.subscriptions
				.as_ref()
				.unwrap()[0]
				.interested
		);
		let mut after = draft();
		after.name = "Updated meetup".into();
		let mut changed = event();
		changed.draft = after.clone();
		let r = request(
			&mut state,
			Action::Edit {
				before: event(),
				after,
			},
		);
		apply(&mut state, r, Ok(Outcome::Event(changed.clone())));
		assert!(
			state
				.request_community(
					Id(10),
					Action::Edit {
						before: event(),
						after: draft()
					}
				)
				.is_none()
		);
		let r = request(
			&mut state,
			Action::Status {
				event: Id(30),
				status: 2,
			},
		);
		changed.status = 2;
		apply(&mut state, r, Ok(Outcome::Event(changed.clone())));
		assert!(
			state
				.request_community(
					Id(10),
					Action::Status {
						event: Id(30),
						status: 4
					}
				)
				.is_none()
		);
		let r = request(
			&mut state,
			Action::Status {
				event: Id(30),
				status: 3,
			},
		);
		changed.status = 3;
		apply(&mut state, r, Ok(Outcome::Event(changed)));
		assert!(
			state
				.request_community(
					Id(10),
					Action::Status {
						event: Id(30),
						status: 2
					}
				)
				.is_none()
		);
		let r = request(&mut state, Action::Delete(Id(30)));
		apply(&mut state, r, Ok(Outcome::Deleted(Id(30))));
		assert!(state.community.events.as_ref().unwrap().events.is_empty());
	}
	#[test]
	fn events_reject_stale_scope_generation_revoked_permissions_and_uncertain_repeats() {
		let mut state = fixture(p::VIEW_CHANNEL | p::CONNECT | p::CREATE_EVENTS);
		listed(&mut state);
		let r = request(&mut state, Action::Delete(Id(30)));
		apply(&mut state, r + 1, Ok(Outcome::Deleted(Id(30))));
		assert!(state.community.pending());
		apply(&mut state, r, Err(Failure::Ambiguous));
		assert!(state.community.needs_refresh);
		assert!(
			state
				.request_community(Id(10), Action::Delete(Id(30)))
				.is_none()
		);
		let stage = request(
			&mut state,
			Action::GetStage {
				channel: Id(21),
				call_request: None,
			},
		);
		apply(
			&mut state,
			stage,
			Ok(Outcome::Stage {
				channel: Id(21),
				instance: None,
				voice: None,
			}),
		);
		assert!(
			state.community.needs_refresh,
			"unrelated Stage refresh cannot authorize an uncertain event repeat"
		);
		listed(&mut state);
		assert!(!state.community.needs_refresh);
		let r = request(&mut state, Action::Delete(Id(30)));
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::PermissionsChanged,
		});
		apply(&mut state, r, Ok(Outcome::Deleted(Id(30))));
		assert!(state.community.events.as_ref().unwrap().events.len() == 1);
		let mut readonly = fixture(p::VIEW_CHANNEL | p::CONNECT);
		listed(&mut readonly);
		assert!(!readonly.can_manage_event(&event()));
		let r = request(&mut readonly, Action::ListEvents);
		readonly.close_events();
		apply(
			&mut readonly,
			r,
			Ok(Outcome::Events(EventPage {
				guild: Id(10),
				events: vec![event()],
				subscriptions: None,
				subscriptions_complete: false,
			})),
		);
		assert!(readonly.community.events.is_none());
		readonly.logout();
		apply(
			&mut readonly,
			r,
			Ok(Outcome::Events(EventPage {
				guild: Id(10),
				events: vec![event()],
				subscriptions: None,
				subscriptions_complete: false,
			})),
		);
		assert!(readonly.community.events.is_none());
	}
	#[test]
	fn stage_audience_invitation_speaker_demotion_reconnect_and_permission_changes_gate_capture() {
		let mut state = fixture(
			p::VIEW_CHANNEL
				| p::CONNECT | p::SPEAK
				| p::USE_VAD | p::REQUEST_TO_SPEAK
				| p::MUTE_MEMBERS,
		);
		assert!(!state.can_speak(Id(21)));
		assert!(state.start_call(Id(21), false).is_some());
		let r = state.voice.active.as_ref().unwrap().request;
		state.voice.active.as_mut().unwrap().phase = voice::Phase::Connected;
		assert!(!state.can_speak(Id(21)));
		voice_state(&mut state, r, StageParticipation::Speaker);
		assert!(
			!state.can_speak(Id(21)),
			"a service speaker state never overrides an audience join's missing consent"
		);
		let action = Action::StageVoice {
			channel: Id(21),
			call_request: r,
			change: StageVoiceChange::AcceptInvitation,
		};
		let write = request(&mut state, action.clone());
		assert!(state.community_command_allowed(Id(10), write, &action));
		apply(
			&mut state,
			write,
			Ok(Outcome::StageVoice { channel: Id(21) }),
		);
		assert!(state.can_speak(Id(21)));
		voice_state(
			&mut state,
			r,
			StageParticipation::Audience { requested: false },
		);
		assert!(!state.can_speak(Id(21)));
		let rev = state.voice.active.as_ref().unwrap().stage_revision;
		voice_state(&mut state, r, StageParticipation::Invited);
		assert!(!state.can_speak(Id(21)));
		voice_state(&mut state, r + 1, StageParticipation::Speaker);
		assert!(!state.can_speak(Id(21)));
		assert!(state.voice.active.as_ref().unwrap().stage_revision > rev);
		let accept = request(
			&mut state,
			Action::StageVoice {
				channel: Id(21),
				call_request: r,
				change: StageVoiceChange::AcceptInvitation,
			},
		);
		voice_state(&mut state, r, StageParticipation::Speaker);
		apply(
			&mut state,
			accept,
			Ok(Outcome::StageVoice { channel: Id(21) }),
		);
		assert!(state.can_speak(Id(21)));
		state.reconnect_call(r, 1, "Synthetic media loss");
		assert!(state.rejoin_call().is_some());
		assert!(!state.can_speak(Id(21)));
		let next = state.voice.active.as_ref().unwrap().request;
		voice_state(&mut state, r, StageParticipation::Speaker);
		assert!(!state.can_speak(Id(21)));
		assert_ne!(next, r);
		state.permissions.guilds.clear();
		state.permissions.clear_cache();
		assert!(!state.can_speak(Id(21)));
	}
	#[test]
	fn recurring_occurrence_override_rsvp_reset_and_restore_preserve_the_series() {
		let mut state = fixture(p::VIEW_CHANNEL | p::CONNECT | p::CREATE_EVENTS);
		let mut series = event();
		let original = series.draft.start;
		series.draft.recurrence = Some(Recurrence {
			start: original,
			end: None,
			frequency: 3,
			interval: 1,
			by_weekday: vec![],
			by_n_weekday: vec![],
			by_month: vec![],
			by_month_day: vec![],
			by_year_day: vec![],
			count: None,
		});
		let r = request(&mut state, Action::ListEvents);
		apply(
			&mut state,
			r,
			Ok(Outcome::Events(EventPage {
				guild: Id(10),
				events: vec![series.clone()],
				subscriptions: Some(vec![]),
				subscriptions_complete: true,
			})),
		);
		assert!(!state.community_action_allowed(
			Id(10),
			&Action::EditOccurrence {
				before: series.clone(),
				original: original + 1,
				canceled: true,
				start: None,
				end: None
			}
		));
		let id = occurrence_id(original).unwrap();
		let exception = EventException {
			event: series.id,
			id,
			canceled: false,
			start: Some(original + 1800),
			end: Some(original + 5400),
		};
		let r = request(
			&mut state,
			Action::EditOccurrence {
				before: series.clone(),
				original,
				canceled: false,
				start: exception.start,
				end: exception.end,
			},
		);
		apply(
			&mut state,
			r,
			Ok(Outcome::Exception {
				event: series.id,
				exception: exception.clone(),
			}),
		);
		let cached = state.community.events.as_ref().unwrap().events[0].clone();
		assert_eq!(cached.draft, series.draft);
		assert_eq!(cached.exceptions, vec![exception.clone()]);
		for interested in [true, false] {
			let r = request(
				&mut state,
				Action::RsvpOccurrence {
					event: series.id,
					occurrence: id,
					interested,
				},
			);
			apply(
				&mut state,
				r,
				Ok(Outcome::Rsvp {
					event: series.id,
					occurrence: Some(id),
					interested,
				}),
			);
			assert_eq!(
				state
					.community
					.events
					.as_ref()
					.unwrap()
					.subscriptions
					.as_ref()
					.unwrap()[0]
					.interested,
				interested
			);
		}
		let r = request(
			&mut state,
			Action::ResetOccurrenceRsvp {
				event: series.id,
				occurrence: id,
			},
		);
		apply(
			&mut state,
			r,
			Ok(Outcome::OccurrenceRsvpReset {
				event: series.id,
				occurrence: id,
			}),
		);
		assert!(
			state
				.community
				.events
				.as_ref()
				.unwrap()
				.subscriptions
				.as_ref()
				.unwrap()
				.is_empty()
		);
		let r = request(
			&mut state,
			Action::EditOccurrence {
				before: cached,
				original,
				canceled: true,
				start: exception.start,
				end: exception.end,
			},
		);
		let mut canceled = exception;
		canceled.canceled = true;
		apply(
			&mut state,
			r,
			Ok(Outcome::Exception {
				event: series.id,
				exception: canceled,
			}),
		);
		let before = state.community.events.as_ref().unwrap().events[0].clone();
		let r = request(
			&mut state,
			Action::RestoreOccurrence {
				before,
				exception: id,
			},
		);
		apply(
			&mut state,
			r,
			Ok(Outcome::ExceptionRestored {
				event: series.id,
				exception: id,
			}),
		);
		assert_eq!(state.community.events.as_ref().unwrap().events[0], series);
	}
	#[test]
	fn stage_access_revocation_end_failed_acceptance_and_local_retirement_revoke_consent() {
		let mut state = fixture(p::ADMINISTRATOR);
		state.start_call(Id(21), false).unwrap();
		let r = state.voice.active.as_ref().unwrap().request;
		state.voice.active.as_mut().unwrap().phase = voice::Phase::Connected;
		voice_state(&mut state, r, StageParticipation::Speaker);
		let a = Action::StageVoice {
			channel: Id(21),
			call_request: r,
			change: StageVoiceChange::AcceptInvitation,
		};
		let req = request(&mut state, a.clone());
		assert!(state.can_speak(Id(21)));
		apply(&mut state, req, Err(Failure::Forbidden));
		assert!(!state.can_speak(Id(21)));
		voice_state(&mut state, r, StageParticipation::Speaker);
		let req = request(&mut state, a.clone());
		apply(&mut state, req, Ok(Outcome::StageVoice { channel: Id(21) }));
		assert!(state.can_speak(Id(21)));
		let revision = state.voice.active.as_ref().unwrap().stage_revision;
		state
			.permissions
			.guilds
			.get_mut(&Id(10))
			.unwrap()
			.roles
			.as_mut()
			.unwrap()[0]
			.bits = p::VIEW_CHANNEL | p::CONNECT;
		state.permissions.clear_cache();
		state.prune_community();
		assert!(!state.can_speak(Id(21)));
		state
			.permissions
			.guilds
			.get_mut(&Id(10))
			.unwrap()
			.roles
			.as_mut()
			.unwrap()[0]
			.bits = p::ADMINISTRATOR;
		state.permissions.clear_cache();
		state.prune_community();
		assert!(!state.can_speak(Id(21)));
		assert!(state.voice.active.as_ref().unwrap().stage_revision > revision);
		let req = request(&mut state, a.clone());
		apply(&mut state, req, Ok(Outcome::StageVoice { channel: Id(21) }));
		assert!(state.can_speak(Id(21)));
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::CommunityChanged {
				guild: Id(10),
				channel: Some(Id(21)),
				stage_ended: true,
			},
		});
		assert!(!state.can_speak(Id(21)));
		assert!(state.voice.active.as_ref().unwrap().muted);
		state.community.stage_stale = false;
		voice_state(&mut state, r, StageParticipation::Invited);
		let req = request(&mut state, a.clone());
		state.leave_call().unwrap();
		assert!(!state.community.pending());
		assert!(state.community.needs_refresh);
		apply(&mut state, req, Ok(Outcome::StageVoice { channel: Id(21) }));
		assert!(state.voice.active.is_none());
		state.community.stage_stale = false;
		state.start_call(Id(21), false).unwrap();
		let next = state.voice.active.as_ref().unwrap().request;
		assert_ne!(next, r);
		voice_state(&mut state, r, StageParticipation::Speaker);
		assert!(!state.can_speak(Id(21)));
	}
	#[test]
	fn stage_audience_metadata_exceeds_mixer_limit_without_expanding_ordinary_voice_limit() {
		let mut state = fixture(p::ADMINISTRATOR);
		let member = |id| voice::Participant {
			user: Id(id),
			muted: true,
			deafened: false,
			server_muted: false,
			server_deafened: false,
			stage: StageParticipation::Audience { requested: false },
			video: false,
			streaming: false,
		};
		state.voice.roster = (100..228)
			.map(|id| voice::RosterEntry {
				guild: Id(10),
				channel: Id(21),
				participant: member(id),
				member: None,
			})
			.collect();
		state.start_call(Id(21), false).unwrap();
		assert_eq!(state.voice.active.as_ref().unwrap().participants.len(), 128);
		assert!(!state.can_speak(Id(21)));
		state.leave_call();
		for row in &mut state.voice.roster {
			row.channel = Id(20);
		}
		assert!(state.start_call(Id(20), false).is_none());
		assert!(state.status.contains("participant limit"));
		assert_eq!(
			voice::MediaMode::Stage.participant_limit(),
			voice::MAX_ROSTER
		);
		assert_eq!(
			voice::MediaMode::Dave.participant_limit(),
			voice::MAX_PARTICIPANTS
		);
	}
	#[test]
	fn ordinary_voice_keeps_suppression_gating_and_channel_type_cannot_change_media_mode() {
		let mut state = fixture(p::ADMINISTRATOR);
		state.start_call(Id(20), false).unwrap();
		let request = state.voice.active.as_ref().unwrap().request;
		for suppress in [true, false] {
			state.apply(Envelope {
				generation: state.generation,
				event: CoreEvent::Voice(voice::Event::State {
					guild: Some(Id(10)),
					member: None,
					server_muted: false,
					server_deafened: false,
					stage: if suppress {
						StageParticipation::Audience { requested: false }
					} else {
						StageParticipation::Speaker
					},
					request: Some(request),
					channel: Some(Id(20)),
					user: Id(1),
					session: None,
					negotiation_revision: None,
					muted: suppress,
					deafened: false,
					video: false,
					streaming: false,
				}),
			});
			assert_eq!(state.voice.active.as_ref().unwrap().server_muted, suppress);
		}
		assert_eq!(
			state.voice.active.as_ref().unwrap().media_mode,
			voice::MediaMode::Dave
		);
		state
			.channels
			.iter_mut()
			.find(|c| c.id == Id(20))
			.unwrap()
			.kind = 13;
		assert!(!state.can_call(Id(20)));
		assert!(!state.can_speak(Id(20)));
		assert_eq!(
			state.voice.active.as_ref().unwrap().media_mode,
			voice::MediaMode::Dave
		);
	}
	#[test]
	fn concurrent_fence_notices_cannot_rewind_scope_or_restore_an_old_account() {
		let mut state = fixture(p::ADMINISTRATOR);
		let generation = state.generation;
		state.apply(Envelope {
			generation,
			event: CoreEvent::CommunityFence(2),
		});
		assert_eq!(state.community.fence, 2);
		let r = request(&mut state, Action::ListEvents);
		state.apply(Envelope {
			generation,
			event: CoreEvent::CommunityFence(1),
		});
		assert_eq!(state.community.fence, 2);
		assert!(state.community.pending());
		state.apply(Envelope {
			generation,
			event: CoreEvent::CommunityFence(3),
		});
		assert_eq!(state.community.fence, 3);
		assert!(!state.community.pending());
		apply(
			&mut state,
			r,
			Ok(Outcome::Events(EventPage {
				guild: Id(10),
				events: vec![event()],
				subscriptions: None,
				subscriptions_complete: false,
			})),
		);
		assert!(state.community.events.is_none());
		state.logout();
		state.apply(Envelope {
			generation,
			event: CoreEvent::CommunityFence(4),
		});
		assert_eq!(state.community.fence, 0);
		assert!(state.community.events.is_none());
	}
}
