//! One member guide with bounded completion intents. Only confirmed visits/messages enqueue writes.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id,
	member_guide::{Action, Snapshot},
};
pub struct Event {
	pub guild: Id,
	pub user: Id,
	pub request: u64,
	pub result: Result<Snapshot, Failure>,
}
#[derive(Default)]
pub struct View {
	pub guild: Option<Id>,
	pub open: bool,
	pub snapshot: Option<Snapshot>,
	pub pending: bool,
	pub error: Option<&'static str>,
	pub needs_refresh: bool,
	sequence: u64,
	action: Option<Action>,
	queue: Vec<(Id, u8)>,
}
impl View {
	pub(crate) fn reset(&mut self) {
		*self = Self {
			sequence: self.sequence.wrapping_add(1),
			..Default::default()
		};
	}
}
impl State {
	pub fn open_server_guide(&mut self, guild: Id) -> Option<Command> {
		self.guild(guild)?;
		if self.member_guide.guild != Some(guild) {
			self.member_guide.reset();
		}
		self.member_guide.guild = Some(guild);
		self.member_guide.open = true;
		self.request_member_guide(Action::Load)
	}
	pub fn close_server_guide(&mut self) {
		self.member_guide.open = false;
	}
	pub fn request_member_guide(&mut self, action: Action) -> Option<Command> {
		let guild = self.member_guide.guild?;
		let user = self.user.as_ref()?.id;
		if self.member_guide.pending
			|| self.guild(guild).is_none()
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return None;
		}
		if let Action::Complete { channel, kind } = action
			&& (self.member_guide.needs_refresh
				|| !self.member_guide.queue.contains(&(channel, kind))
				|| !self.can_view(channel)
				|| !self.member_guide.snapshot.as_ref().is_some_and(|s| {
					s.guild == guild
						&& s.user == user && s.guide.enabled
						&& s.guide
							.actions
							.iter()
							.any(|r| r.channel == channel && r.action == kind)
						&& !s.completed.contains(&channel)
				})) {
			return None;
		}
		self.member_guide.sequence = self.member_guide.sequence.wrapping_add(1);
		self.member_guide.pending = true;
		self.member_guide.action = Some(action);
		self.member_guide.error = None;
		Some(Command::MemberGuide {
			guild,
			user,
			request: self.member_guide.sequence,
			action,
		})
	}
	pub fn member_guide_command_allowed(&self, guild: Id, user: Id, request: u64) -> bool {
		self.member_guide.guild == Some(guild)
			&& self.user.as_ref().is_some_and(|u| u.id == user)
			&& self.guild(guild).is_some()
			&& self.member_guide.pending
			&& self.member_guide.sequence == request
			&& self.member_guide.action.is_some_and(|a| match a {
				Action::Load => true,
				Action::Complete { channel: c, .. } => {
					self.can_view(c) && self.channel(c).is_some_and(|c| c.guild == Some(guild))
				}
			})
	}
	pub fn continue_member_guide(&mut self) -> Option<Command> {
		if self.member_guide.pending || self.member_guide.needs_refresh {
			return None;
		}
		let (channel, kind) = *self.member_guide.queue.first()?;
		self.request_member_guide(Action::Complete { channel, kind })
	}
	fn queue_guide_completion(&mut self, channel: Id, kind: u8) {
		if !self.can_view(channel) || self.member_guide.needs_refresh {
			return;
		}
		if self.member_guide.snapshot.as_ref().is_some_and(|s| {
			s.guide.enabled
				&& s.guide
					.actions
					.iter()
					.any(|r| r.channel == channel && r.action == kind)
				&& !s.completed.contains(&channel)
		}) && !self.member_guide.queue.iter().any(|(id, _)| *id == channel)
			&& self.member_guide.queue.len() < model::community_admin::MAX_ACTIONS
		{
			self.member_guide.queue.push((channel, kind));
		}
	}
	pub fn visit_server_guide_channel(&mut self, channel: Id) -> Option<Command> {
		let guild = self.member_guide.guild?;
		if !self
			.channel(channel)
			.is_some_and(|c| c.guild == Some(guild))
			|| !self.can_view(channel)
		{
			return None;
		}
		self.queue_guide_completion(channel, 0);
		self.member_guide.open = false;
		self.select(channel)
	}
	pub(crate) fn observe_member_guide_event(&mut self, event: &crate::Event) {
		match event {
			crate::Event::Message(m) | crate::Event::SendResult { result: Ok(m), .. }
				if self.user.as_ref().is_some_and(|u| u.id == m.author.id) =>
			{
				self.queue_guide_completion(m.channel, 1)
			}
			crate::Event::Unavailable(channel)
				if self.member_guide.snapshot.as_ref().is_some_and(|s| {
					s.guide
						.actions
						.iter()
						.chain(&s.guide.resources)
						.any(|r| r.channel == *channel)
				}) =>
			{
				self.cancel_member_guide();
				self.member_guide.snapshot = None;
				self.member_guide.needs_refresh = true;
				self.member_guide.error = Some("A guide channel was removed; reload Server Guide");
			}
			_ => {}
		}
	}
	pub(crate) fn reconcile_member_guide_access(&mut self) {
		if self
			.member_guide
			.guild
			.is_some_and(|g| self.guild(g).is_none())
		{
			self.member_guide.reset();
		}
		self.member_guide.queue = self
			.member_guide
			.queue
			.iter()
			.copied()
			.filter(|(id, kind)| {
				self.can_view(*id)
					&& self.member_guide.snapshot.as_ref().is_some_and(|s| {
						!s.completed.contains(id)
							&& s.guide
								.actions
								.iter()
								.any(|row| row.channel == *id && row.action == *kind)
					})
			})
			.collect();
		if matches!(self.member_guide.action,Some(Action::Complete{channel:c,..}) if !self.can_view(c))
		{
			self.cancel_member_guide();
		}
	}
	pub(crate) fn cancel_member_guide(&mut self) {
		self.member_guide.queue.clear();
		if !self.member_guide.pending {
			return;
		}
		self.member_guide.needs_refresh |=
			matches!(self.member_guide.action, Some(Action::Complete { .. }));
		self.member_guide.pending = false;
		self.member_guide.sequence = self.member_guide.sequence.wrapping_add(1);
		self.member_guide.action = None;
		self.member_guide.error = Some("Server Guide interrupted; reload before continuing");
	}
	pub(crate) fn apply_member_guide(&mut self, event: Event) -> Result<(), &'static str> {
		if self.member_guide.guild != Some(event.guild)
			|| self.member_guide.sequence != event.request
			|| !self.member_guide.pending
			|| !self.user.as_ref().is_some_and(|u| u.id == event.user)
			|| self.guild(event.guild).is_none()
		{
			return Ok(());
		}
		self.member_guide.pending = false;
		let action = self.member_guide.action.take();
		let result = event.result.and_then(|s| {
			if !s.valid()
				|| s.guild != event.guild
				|| s.user != event.user
				|| matches!(action,Some(Action::Complete{channel:c,..}) if !s.completed.contains(&c))
			{
				Err(Failure::Ambiguous)
			} else {
				Ok(s)
			}
		});
		if let Some(Action::Complete { channel, .. }) = action {
			self.member_guide.queue.retain(|(id, _)| *id != channel);
		}
		match result {
			Ok(mut snapshot) => {
				if matches!(action, Some(Action::Complete { .. }))
					&& !snapshot.header_known
					&& let Some(previous) = &self.member_guide.snapshot
				{
					snapshot.header_known = previous.header_known;
					snapshot.guide.header.clone_from(&previous.guide.header);
					snapshot.guide.header_available = previous.guide.header_available;
				}
				if action == Some(Action::Load) {
					self.member_guide.needs_refresh = false;
				}
				self.member_guide.snapshot = Some(snapshot);
				self.reconcile_member_guide_access();
			}
			Err(failure) => {
				self.member_guide.error = Some(failure.label());
				self.member_guide.needs_refresh |= matches!(action, Some(Action::Complete { .. }));
				if failure == Failure::Forbidden {
					self.member_guide.snapshot = None;
				}
				if failure.ends_session() {
					self.fail(failure);
				}
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event as CoreEvent};
	fn state() -> State {
		crate::community_admin::tests::state(0)
	}
	fn snapshot() -> Snapshot {
		Snapshot {
			guild: Id(2),
			user: Id(1),
			guide: model::community_admin::Guide {
				enabled: true,
				actions: vec![
					model::community_admin::GuideChannel {
						channel: Id(10),
						title: "Read".into(),
						..Default::default()
					},
					model::community_admin::GuideChannel {
						channel: Id(11),
						action: 1,
						title: "Say hello".into(),
						..Default::default()
					},
				],
				..Default::default()
			},
			completed: vec![],
			header_known: false,
		}
	}
	fn load(state: &mut State) {
		let Command::MemberGuide { request, .. } = state.open_server_guide(Id(2)).unwrap() else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::MemberGuide(Event {
				guild: Id(2),
				user: Id(1),
				request,
				result: Ok(snapshot()),
			}),
		});
	}
	#[test]
	fn member_guide_needs_no_admin_and_only_visits_or_confirmed_own_messages_queue_completion() {
		let mut state = state();
		assert!(!state.can_manage_guild(Id(2)));
		load(&mut state);
		assert!(
			state
				.request_member_guide(Action::Complete {
					channel: Id(10),
					kind: 0
				})
				.is_none()
		);
		state.visit_server_guide_channel(Id(11));
		assert!(
			state.continue_member_guide().is_none(),
			"Opening a chat task does not complete it"
		);
		let mut message = crate::tests::message(200);
		message.channel = Id(11);
		message.author = state.user.clone().unwrap();
		state.observe_member_guide_event(&CoreEvent::Message(message.clone()));
		state.observe_member_guide_event(&CoreEvent::Message(message));
		assert_eq!(state.member_guide.queue, vec![(Id(11), 1)]);
		let Command::MemberGuide {
			action, request, ..
		} = state.continue_member_guide().unwrap()
		else {
			panic!()
		};
		assert_eq!(
			action,
			Action::Complete {
				channel: Id(11),
				kind: 1
			}
		);
		let mut done = snapshot();
		done.completed.push(Id(11));
		state
			.apply_member_guide(Event {
				guild: Id(2),
				user: Id(1),
				request,
				result: Ok(done),
			})
			.unwrap();
		assert!(state.continue_member_guide().is_none());
		state.visit_server_guide_channel(Id(10));
		assert!(matches!(
			state.continue_member_guide(),
			Some(Command::MemberGuide {
				action: Action::Complete {
					channel: Id(10),
					kind: 0
				},
				..
			})
		));
	}
	#[test]
	fn member_guide_changed_task_kind_does_not_reuse_a_previous_visit() {
		let mut state = state();
		load(&mut state);
		state.queue_guide_completion(Id(10), 0);
		let Command::MemberGuide { request, .. } =
			state.request_member_guide(Action::Load).unwrap()
		else {
			panic!()
		};
		let mut changed = snapshot();
		changed.guide.actions[0].action = 1;
		state
			.apply_member_guide(Event {
				guild: Id(2),
				user: Id(1),
				request,
				result: Ok(changed),
			})
			.unwrap();
		assert!(state.member_guide.queue.is_empty());
		assert!(state.continue_member_guide().is_none());
	}
	#[test]
	fn member_guide_permission_loss_and_disconnect_fence_late_completions() {
		let mut state = state();
		load(&mut state);
		state.visit_server_guide_channel(Id(10));
		let Command::MemberGuide { request, .. } = state.continue_member_guide().unwrap() else {
			panic!()
		};
		state.cancel_member_guide();
		let mut done = snapshot();
		done.completed.push(Id(10));
		state
			.apply_member_guide(Event {
				guild: Id(2),
				user: Id(1),
				request,
				result: Ok(done),
			})
			.unwrap();
		assert!(state.member_guide.needs_refresh);
		assert!(
			state
				.member_guide
				.snapshot
				.as_ref()
				.unwrap()
				.completed
				.is_empty()
		);
		assert!(state.continue_member_guide().is_none());
		load(&mut state);
		state.visit_server_guide_channel(Id(10));
		let Command::MemberGuide { request, .. } = state.continue_member_guide().unwrap() else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Permissions(crate::permissions::Event::Channel {
				guild: Some(Id(2)),
				channel: Id(10),
				overwrites: model::Patch::Value(vec![model::permissions::Overwrite {
					id: Id(2),
					kind: 0,
					allow: 0,
					deny: model::permissions::VIEW_CHANNEL,
				}]),
			}),
		});
		assert!(!state.member_guide.pending);
		assert!(!state.member_guide_command_allowed(Id(2), Id(1), request));
		assert!(state.member_guide.needs_refresh);
	}
	#[test]
	fn member_guide_wrong_user_and_removed_channel_cannot_replace_snapshot() {
		let mut state = state();
		load(&mut state);
		let Command::MemberGuide { request, .. } =
			state.request_member_guide(Action::Load).unwrap()
		else {
			panic!()
		};
		let old = state.member_guide.snapshot.clone();
		state
			.apply_member_guide(Event {
				guild: Id(2),
				user: Id(99),
				request,
				result: Ok(snapshot()),
			})
			.unwrap();
		assert_eq!(state.member_guide.snapshot, old);
		state.cancel_member_guide();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Unavailable(Id(10)),
		});
		assert!(state.member_guide.snapshot.is_none());
		assert!(state.member_guide.needs_refresh);
	}
}
