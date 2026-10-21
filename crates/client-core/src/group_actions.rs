//! One explicit group operation at a time; image data is never retained in state or echoed back.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{ChannelPatch, Id, Patch};

pub const MAX_ICON_DATA_URI: usize = 22 + 4 * (256_usize * 1024).div_ceil(3);

pub enum Action {
	Create(Vec<Id>),
	Leave(Id),
	LoadMembers(Id),
	AddMember {
		channel: Id,
		user: Id,
	},
	RemoveMember {
		channel: Id,
		user: Id,
	},
	TransferOwner {
		channel: Id,
		user: Id,
	},
	Edit {
		channel: Id,
		name: Option<String>,
		icon: Patch<String>,
	},
}

impl Action {
	pub fn channel(&self) -> Id {
		match self {
			Self::Create(_) => Id(0),
			Self::Leave(channel)
			| Self::LoadMembers(channel)
			| Self::Edit { channel, .. }
			| Self::AddMember { channel, .. }
			| Self::RemoveMember { channel, .. }
			| Self::TransferOwner { channel, .. } => *channel,
		}
	}
	pub fn valid(&self) -> bool {
		(matches!(self, Self::Create(_)) || self.channel().0 != 0)
			&& match self {
				Self::Create(users) => {
					(2..=9).contains(&users.len())
						&& users.capacity() <= 16
						&& users
							.iter()
							.enumerate()
							.all(|(index, user)| user.0 != 0 && !users[..index].contains(user))
				}
				Self::Leave(_) | Self::LoadMembers(_) => true,
				Self::AddMember { user, .. }
				| Self::RemoveMember { user, .. }
				| Self::TransferOwner { user, .. } => user.0 != 0,
				Self::Edit { name, icon, .. } => {
					(name.is_some() || !matches!(icon, Patch::Absent))
						&& name.as_ref().is_none_or(|name| {
							!name.trim().is_empty()
								&& name.chars().count() <= 100
								&& name.capacity() <= 400
								&& !name.chars().any(char::is_control)
						}) && match icon {
						Patch::Value(uri) => {
							uri.capacity() <= MAX_ICON_DATA_URI
								&& uri
									.strip_prefix("data:image/png;base64,")
									.is_some_and(|data| {
										!data.is_empty()
											&& data.bytes().all(|b| {
												b.is_ascii_alphanumeric() || b"+/=".contains(&b)
											})
									})
						}
						_ => true,
					}
				}
			}
	}
}
pub enum Event {
	Created {
		request: u64,
		result: Result<(Box<model::Channel>, Members), Failure>,
	},
	MembersLoaded {
		channel: Id,
		request: u64,
		result: Result<Members, Failure>,
	},
	Written {
		channel: Id,
		request: u64,
		result: Result<Option<ChannelPatch>, Failure>,
	},
}
/// One on-demand group snapshot; ordinary-user recipient acquisition is unofficial.
pub struct Members {
	pub channel: Id,
	pub owner: Id,
	pub recipients: Vec<model::User>,
}
impl Members {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.recipients.capacity() * size_of::<model::User>()
			+ self
				.recipients
				.iter()
				.map(model::User::heap_bytes)
				.sum::<usize>()
	}
	pub fn valid(&self, owner: Id) -> bool {
		self.channel.0 != 0
			&& self.owner.0 != 0
			&& self.recipients.len() <= 9
			&& self.bytes() <= 32 * 1024
			&& (self.owner == owner || self.recipients.iter().any(|user| user.id == self.owner))
			&& self.recipients.iter().enumerate().all(|(index, user)| {
				user.id.0 != 0
					&& user.id != owner
					&& !user.webhook
					&& user.name.len() <= 256
					&& !self.recipients[..index]
						.iter()
						.any(|other| user.id == other.id)
			})
	}
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberOperation {
	Create,
	Load,
	Add(Id),
	Remove(Id),
	Transfer(Id),
}
#[derive(Default)]
pub struct Actions {
	sequence: u64,
	// channel, request, leaving, renaming, newer service change observed
	pending: Option<(Id, u64, bool, bool, bool)>,
	status: Option<(Id, &'static str)>,
	completed: Option<(Id, u64, bool)>,
	members: Option<Members>,
	member_operation: Option<MemberOperation>,
	creating: Vec<Id>,
	created: Option<(u64, Id)>,
}
impl Actions {
	pub(crate) fn reset(&mut self) {
		*self = Self {
			sequence: self.sequence,
			// READY can follow a canceled write without changing session generation.
			// Keep its bounded outcome so an open editor can leave the busy state.
			completed: self.completed,
			status: self.status,
			..Self::default()
		};
	}
}
impl State {
	pub fn can_create_group(&self, users: &[Id]) -> bool {
		(2..=9).contains(&users.len())
			&& users.iter().enumerate().all(|(index, user)| {
				!users[..index].contains(user)
					&& self.user.as_ref().is_some_and(|owner| owner.id != *user)
					&& self.friends().any(|friend| friend.id == *user)
					&& self.user_blocked(*user) == Some(false)
			})
	}
	pub fn create_group(&mut self, users: Vec<Id>) -> Option<Command> {
		if !self.can_create_group(&users) {
			return None;
		}
		self.request_group_action(Action::Create(users))
	}
	pub fn created_group(&self, request: u64) -> Option<Id> {
		self.group_actions
			.created
			.filter(|(sequence, id)| *sequence == request && self.is_group_dm(*id))
			.map(|(_, id)| id)
	}
	pub fn transfer_group_owner(&mut self, channel: Id, user: Id) -> Option<Command> {
		if !self.can_remove_group_member(channel, user) {
			return None;
		}
		self.request_group_action(Action::TransferOwner { channel, user })
	}
	pub fn group_members(&self, channel: Id) -> Option<&Members> {
		self.group_actions
			.members
			.as_ref()
			.filter(|members| members.channel == channel && self.is_group_dm(channel))
	}
	pub fn can_add_group_member(&self, channel: Id, user: Id) -> bool {
		self.user.as_ref().is_some_and(|owner| owner.id != user)
			&& self.group_members(channel).is_some_and(|members| {
				members.recipients.len() < 9
					&& !members.recipients.iter().any(|member| member.id == user)
			}) && self.friends().any(|friend| friend.id == user)
			&& self.user_blocked(user) == Some(false)
	}
	pub fn can_remove_group_member(&self, channel: Id, user: Id) -> bool {
		self.group_members(channel).is_some_and(|members| {
			self.user
				.as_ref()
				.is_some_and(|owner| owner.id == members.owner && owner.id != user)
				&& members.recipients.iter().any(|member| member.id == user)
		})
	}
	pub fn request_group_members(&mut self, channel: Id) -> Option<Command> {
		self.request_group_action(Action::LoadMembers(channel))
	}
	pub fn add_group_member(&mut self, channel: Id, user: Id) -> Option<Command> {
		if !self.can_add_group_member(channel, user) {
			return None;
		}
		self.request_group_action(Action::AddMember { channel, user })
	}
	pub fn remove_group_member(&mut self, channel: Id, user: Id) -> Option<Command> {
		if !self.can_remove_group_member(channel, user) {
			return None;
		}
		self.request_group_action(Action::RemoveMember { channel, user })
	}
	pub fn group_action_pending(&self) -> bool {
		self.group_actions.pending.is_some()
	}
	pub fn group_action_status(&self, channel: Id) -> Option<&'static str> {
		self.group_actions
			.status
			.filter(|(id, _)| *id == channel)
			.map(|(_, text)| text)
	}
	pub fn group_action_completed(&self, channel: Id, request: u64) -> Option<bool> {
		self.group_actions
			.completed
			.filter(|(id, seq, _)| *id == channel && *seq == request)
			.map(|(_, _, success)| success)
	}
	pub fn clear_group_action_result(&mut self, channel: Id) {
		if self
			.group_actions
			.status
			.is_some_and(|(id, _)| id == channel)
		{
			self.group_actions.status = None;
		}
	}
	pub fn is_group_dm(&self, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|c| c.guild.is_none() && c.kind == 3)
	}
	pub fn leave_group_reason(&self, channel: Id) -> Option<&'static str> {
		if !self.is_group_dm(channel) {
			return Some("Group is no longer available");
		}
		if self.pending.iter().any(|p| p.channel == channel)
			|| self
				.voice
				.active
				.as_ref()
				.is_some_and(|call| call.channel == channel)
		{
			return Some("Finish pending messages and leave the call before leaving this group");
		}
		None
	}
	pub fn leave_group(&mut self, channel: Id) -> Option<Command> {
		if let Some(reason) = self.leave_group_reason(channel) {
			self.group_actions.status = Some((channel, reason));
			return None;
		}
		self.request_group_action(Action::Leave(channel))
	}
	pub fn edit_group(
		&mut self,
		channel: Id,
		name: Option<String>,
		icon: Patch<String>,
	) -> Option<Command> {
		self.request_group_action(Action::Edit {
			channel,
			name,
			icon,
		})
	}
	fn request_group_action(&mut self, action: Action) -> Option<Command> {
		let channel = action.channel();
		if self.group_action_pending()
			|| (!matches!(action, Action::Create(_)) && !self.is_group_dm(channel))
		{
			return None;
		}
		if !action.valid() {
			self.group_actions.status = Some((
				channel,
				"Use a group name of 1–100 characters and a supported icon",
			));
			return None;
		}
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			self.group_actions.status =
				Some((channel, "Group actions unavailable while disconnected"));
			return None;
		}
		self.group_actions.sequence = self.group_actions.sequence.wrapping_add(1);
		let request = self.group_actions.sequence;
		self.group_actions.pending = Some((
			channel,
			request,
			matches!(action, Action::Leave(_)),
			matches!(&action, Action::Edit { name: Some(_), .. }),
			false,
		));
		self.group_actions.member_operation = match action {
			Action::Create(ref users) => {
				self.group_actions.creating = users.clone();
				Some(MemberOperation::Create)
			}
			Action::LoadMembers(_) => Some(MemberOperation::Load),
			Action::AddMember { user, .. } => Some(MemberOperation::Add(user)),
			Action::RemoveMember { user, .. } => Some(MemberOperation::Remove(user)),
			Action::TransferOwner { user, .. } => Some(MemberOperation::Transfer(user)),
			_ => None,
		};
		self.group_actions.status = None;
		self.group_actions.completed = None;
		Some(Command::GroupAction { action, request })
	}
	pub(crate) fn cancel_group_action(&mut self) {
		if let Some((channel, request, _, _, _)) = self.group_actions.pending.take() {
			self.group_actions.status = Some((
				channel,
				if self.group_actions.member_operation == Some(MemberOperation::Load) {
					"Group member list load interrupted; refresh to try again"
				} else {
					"Outcome unknown; check Discord before retrying"
				},
			));
			self.group_actions.completed = Some((channel, request, false));
		}
		self.group_actions.members = None;
		self.group_actions.member_operation = None;
		self.group_actions.creating.clear();
	}
	pub(crate) fn observe_group_change(&mut self, channel: Id, recreated: bool) {
		if self
			.group_actions
			.members
			.as_ref()
			.is_some_and(|members| members.channel == channel)
		{
			self.group_actions.members = None;
		}
		if let Some((target, _, leaving, _, observed)) = &mut self.group_actions.pending
			&& *target == channel
			&& (recreated || !*leaving)
		{
			*observed = true;
		}
	}
	pub(crate) fn apply_group_action(&mut self, event: Event) -> Result<(), &'static str> {
		if let Event::Created { request, result } = event {
			if !self
				.group_actions
				.pending
				.is_some_and(|(channel, sequence, ..)| channel == Id(0) && sequence == request)
				|| self.group_actions.member_operation != Some(MemberOperation::Create)
			{
				return Ok(());
			}
			self.group_actions.pending = None;
			self.group_actions.member_operation = None;
			let users = std::mem::take(&mut self.group_actions.creating);
			let result =
				result.and_then(|(channel, members)| {
					if channel.id == members.channel
						&& channel.id.0 != 0
						&& channel.guild.is_none()
						&& channel.kind == 3
						&& channel.bytes() <= 32 * 1024
						&& self.user.as_ref().is_some_and(|owner| {
							members.owner == owner.id && members.valid(owner.id)
						}) && members.recipients.len() == users.len()
						&& members
							.recipients
							.iter()
							.all(|user| users.contains(&user.id))
						&& channel.recipients.len() == users.len()
						&& channel.recipients.iter().enumerate().all(|(index, user)| {
							users.contains(&user.id)
								&& !channel.recipients[..index]
									.iter()
									.any(|other| other.id == user.id)
						}) && self
						.channel(channel.id)
						.is_none_or(|known| known.kind == 3 && known.guild.is_none())
					{
						Ok((channel, members))
					} else {
						Err(Failure::Ambiguous)
					}
				});
			self.group_actions.completed = Some((Id(0), request, false));
			match result {
				Ok((channel, members)) => {
					let id = channel.id;
					let existing = self.channel(id).is_some();
					if !existing {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::ChannelCreated(*channel),
						});
					}
					if !self.is_group_dm(id) {
						self.group_actions.status = Some((Id(0), Failure::Ambiguous.label()));
						return Ok(());
					}
					if !existing {
						self.group_actions.members = Some(members);
					}
					self.group_actions.created = Some((request, id));
					self.group_actions.completed = Some((Id(0), request, true));
					self.group_actions.status = Some((Id(0), "Group created"));
				}
				Err(failure) => {
					if failure.ends_session() {
						self.fail(failure);
					}
					self.group_actions.status = Some((Id(0), failure.label()));
				}
			}
			return Ok(());
		}
		if let Event::MembersLoaded {
			channel,
			request,
			result,
		} = event
		{
			if !self
				.group_actions
				.pending
				.is_some_and(|(target, sequence, _, _, _)| target == channel && sequence == request)
				|| !matches!(
					self.group_actions.member_operation,
					Some(MemberOperation::Load | MemberOperation::Transfer(_))
				) {
				return Ok(());
			}
			let observed = self
				.group_actions
				.pending
				.take()
				.is_some_and(|(_, _, _, _, observed)| observed);
			let operation = self.group_actions.member_operation.take();
			let result = result.and_then(|members| {
				if members.channel == channel
					&& match operation {
						Some(MemberOperation::Transfer(owner)) => members.owner == owner,
						_ => true,
					} && self
					.user
					.as_ref()
					.is_some_and(|owner| members.valid(owner.id))
				{
					Ok(members)
				} else {
					Err(if matches!(operation, Some(MemberOperation::Transfer(_))) {
						Failure::Ambiguous
					} else {
						Failure::Protocol
					})
				}
			});
			match result {
				Ok(members) if !observed && self.is_group_dm(channel) => {
					let old = self.channel(channel).unwrap().recipients.clone();
					for user in old.into_iter().filter(|user| {
						!members.recipients.iter().any(|member| member.id == user.id)
					}) {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::RecipientRemoved {
								channel,
								user: user.id,
							},
						});
					}
					for user in &members.recipients {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::RecipientAdded {
								channel,
								user: user.clone(),
							},
						});
					}
					self.group_actions.members = Some(members);
					self.group_actions.status = None;
					self.group_actions.completed = Some((channel, request, true));
				}
				Ok(_) => {
					self.group_actions.status =
						Some((channel, "Group membership changed; refresh the member list"));
					self.group_actions.completed = Some((channel, request, false));
				}
				Err(failure) => {
					if matches!(operation, Some(MemberOperation::Transfer(_))) {
						self.group_actions.members = None;
					}
					if failure.ends_session() {
						self.fail(failure);
					}
					self.group_actions.status = Some((channel, failure.label()));
					self.group_actions.completed = Some((channel, request, false));
				}
			}
			return Ok(());
		}
		let Event::Written {
			channel,
			request,
			result,
		} = event
		else {
			return Ok(());
		};
		let Some((target, sequence, leaving, renaming, observed)) = self.group_actions.pending
		else {
			return Ok(());
		};
		if channel != target
			|| request != sequence
			|| matches!(
				self.group_actions.member_operation,
				Some(
					MemberOperation::Create | MemberOperation::Load | MemberOperation::Transfer(_)
				)
			) {
			return Ok(());
		}
		self.group_actions.pending = None;
		let member_operation = self.group_actions.member_operation.take();
		let result = result.and_then(|patch| {
			if matches!(
				member_operation,
				Some(MemberOperation::Add(_) | MemberOperation::Remove(_))
			) && patch.is_none()
			{
				return Ok(None);
			}
			if leaving && patch.is_none() {
				return Ok(None);
			}
			if !leaving
				&& patch.as_ref().is_some_and(|p| {
					p.id == channel
						&& match &p.name {
							Patch::Value(name) => {
								!name.is_empty()
									&& name.chars().count() <= 100
									&& name.capacity() <= 400
							}
							Patch::Absent => !renaming,
							Patch::Null => false,
						} && match &p.icon {
						Patch::Value(hash) => {
							model::valid_avatar_hash(hash) && hash.capacity() <= 128
						}
						Patch::Null => true,
						Patch::Absent => false,
					}
				}) {
				Ok(patch)
			} else {
				Err(Failure::Ambiguous)
			}
		});
		self.group_actions.completed = Some((channel, request, result.is_ok()));
		let status = match result {
			Err(failure) => {
				if member_operation.is_some() {
					self.group_actions.members = None;
				}
				if failure.ends_session() {
					self.fail(failure);
				}
				failure.label()
			}
			Ok(_) if member_operation.is_some() => {
				if !observed && self.is_group_dm(channel) {
					match member_operation {
						Some(MemberOperation::Add(user)) => {
							let friend = self.friends().find(|friend| friend.id == user).cloned();
							if let Some(user) = friend {
								self.apply(crate::Envelope {
									generation: self.generation,
									event: crate::Event::RecipientAdded { channel, user },
								});
							}
						}
						Some(MemberOperation::Remove(user)) => self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::RecipientRemoved { channel, user },
						}),
						_ => {}
					}
				}
				self.group_actions.members = None;
				"Group membership updated; refresh the member list"
			}
			Ok(_) if observed => "Request completed; latest group settings shown",
			Ok(Some(patch)) => {
				if let Some(group) = self
					.channels
					.iter_mut()
					.find(|c| c.id == channel && c.kind == 3 && c.guild.is_none())
				{
					if let Patch::Value(name) = patch.name {
						group.name = name;
					}
					group.icon = match patch.icon {
						Patch::Value(hash) => Some(hash),
						_ => None,
					};
					self.invalidate_navigation();
				}
				"Group updated"
			}
			Ok(None) => {
				self.remove_channels(&std::collections::BTreeSet::from([channel]));
				if self.selected == Some(channel) {
					self.arrived_home();
				}
				"Left group"
			}
		};
		self.group_actions.status = Some((channel, status));
		self.status = status;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event as CoreEvent};
	fn state() -> State {
		State {
			demo: true,
			selected: Some(Id(10)),
			channels: vec![model::Channel {
				id: Id(10),
				guild: None,
				kind: 3,
				name: "Group".into(),
				icon: None,
				last_message: None,
				parent_id: None,
				position: 0,
				recipients: vec![],
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			..State::default()
		}
	}
	fn patch(name: &str) -> ChannelPatch {
		ChannelPatch {
			id: Id(10),
			name: Patch::Value(name.into()),
			icon: Patch::Null,
			last_message: Patch::Absent,
			parent_id: Patch::Absent,
			position: Patch::Absent,
			kind: Patch::Absent,
			message_count: Patch::Absent,
			tags: Patch::Absent,
		}
	}
	fn finish(state: &mut State, command: Command, result: Result<Option<ChannelPatch>, Failure>) {
		let Command::GroupAction { action, request } = command else {
			panic!("wrong command");
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::Written {
				channel: action.channel(),
				request,
				result,
			}),
		});
	}
	fn person(id: u64) -> model::User {
		model::User {
			username: None,
			id: Id(id),
			name: format!("Synthetic {id}"),
			avatar: None,
			webhook: false,
			kind: Default::default(),
			discriminator: 0,
			primary_guild: None,
			cosmetics: None,
		}
	}
	fn members(state: &mut State, owner: Id, recipients: Vec<model::User>) {
		let Command::GroupAction { request, .. } = state.request_group_members(Id(10)).unwrap()
		else {
			panic!();
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::MembersLoaded {
				channel: Id(10),
				request,
				result: Ok(Members {
					channel: Id(10),
					owner,
					recipients,
				}),
			}),
		});
	}
	#[test]
	fn group_creation_and_owner_transfer_confirm_identity_revoke_ownership_and_ignore_old_results()
	{
		let mut state = state();
		state.user = Some(person(1));
		state.drafts.insert(Id(10), "Keep original draft".into());
		state
			.apply_user_action(crate::user_actions::Event::Friends(Some(vec![
				(person(2), "two".into()),
				(person(3), "three".into()),
			])))
			.unwrap();
		assert!(!state.can_create_group(&[Id(2)]));
		assert!(!state.can_create_group(&[Id(2), Id(2)]));
		assert!(!state.can_create_group(&[Id(2), Id(7)]));
		assert!(state.can_create_group(&[Id(2), Id(3)]));
		let Command::GroupAction { request, .. } = state.create_group(vec![Id(2), Id(3)]).unwrap()
		else {
			panic!()
		};
		assert!(state.create_group(vec![Id(2), Id(3)]).is_none());
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::Written {
				channel: Id(0),
				request,
				result: Ok(None),
			}),
		});
		assert!(
			state.group_action_pending(),
			"a write acknowledgement cannot substitute for the requested creation result"
		);
		let mut group = state.channels[0].clone();
		group.id = Id(11);
		group.recipients = vec![person(2), person(3)];
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::Created {
				request,
				result: Ok((
					Box::new(group),
					Members {
						channel: Id(11),
						owner: Id(1),
						recipients: vec![person(2), person(3)],
					},
				)),
			}),
		});
		assert_eq!(state.created_group(request), Some(Id(11)));
		assert_eq!(state.group_action_completed(Id(0), request), Some(true));
		assert_eq!(
			state.selected,
			Some(Id(10)),
			"only an unmodified UI context may navigate after creation"
		);
		let Command::GroupAction { request, .. } =
			state.transfer_group_owner(Id(11), Id(2)).unwrap()
		else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::MembersLoaded {
				channel: Id(11),
				request,
				result: Ok(Members {
					channel: Id(11),
					owner: Id(2),
					recipients: vec![person(2), person(3)],
				}),
			}),
		});
		assert_eq!(state.group_action_completed(Id(11), request), Some(true));
		assert!(!state.can_remove_group_member(Id(11), Id(3)));
		assert!(state.transfer_group_owner(Id(11), Id(3)).is_none());
		let Command::GroupAction { request, .. } = state.create_group(vec![Id(2), Id(3)]).unwrap()
		else {
			panic!()
		};
		let old_generation = state.generation;
		state.cancel_group_action();
		state.generation += 1;
		let mut group = state.channels[0].clone();
		group.id = Id(12);
		group.recipients = vec![person(2), person(3)];
		state.apply(Envelope {
			generation: old_generation,
			event: CoreEvent::GroupAction(Event::Created {
				request,
				result: Ok((
					Box::new(group),
					Members {
						channel: Id(12),
						owner: Id(1),
						recipients: vec![person(2), person(3)],
					},
				)),
			}),
		});
		assert!(state.channel(Id(12)).is_none());
		assert_eq!(state.drafts[&Id(10)], "Keep original draft");
	}
	#[test]
	fn group_membership_checks_owner_capacity_stale_reads_and_uncertain_writes() {
		let mut state = state();
		state.user = Some(person(1));
		state.drafts.insert(Id(10), "Keep group draft".into());
		state
			.apply_user_action(crate::user_actions::Event::Friends(Some(vec![
				(person(2), "two".into()),
				(person(3), "three".into()),
			])))
			.unwrap();
		assert!(
			state.add_group_member(Id(10), Id(2)).is_none(),
			"unknown membership cannot establish capacity"
		);
		members(&mut state, Id(1), vec![person(2)]);
		assert!(state.can_remove_group_member(Id(10), Id(2)));
		assert!(!state.can_add_group_member(Id(10), Id(2)));
		assert!(
			!state.can_add_group_member(Id(10), Id(99)),
			"normal group invitations require friendship"
		);
		let add = state.add_group_member(Id(10), Id(3)).unwrap();
		assert!(state.add_group_member(Id(10), Id(3)).is_none());
		finish(&mut state, add, Err(Failure::Ambiguous));
		assert!(state.group_members(Id(10)).is_none());
		assert!(
			state.add_group_member(Id(10), Id(3)).is_none(),
			"an uncertain write requires an explicit membership refresh"
		);
		members(&mut state, Id(2), vec![person(2), person(3)]);
		assert!(
			state.remove_group_member(Id(10), Id(3)).is_none(),
			"owner revocation removes the permission"
		);
		members(&mut state, Id(1), (2..=10).map(person).collect());
		assert!(!state.can_add_group_member(Id(10), Id(3)));
		let Command::GroupAction { request, .. } = state.request_group_members(Id(10)).unwrap()
		else {
			panic!();
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::RecipientRemoved {
				channel: Id(10),
				user: Id(2),
			},
		});
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::GroupAction(Event::MembersLoaded {
				channel: Id(10),
				request,
				result: Ok(Members {
					channel: Id(10),
					owner: Id(1),
					recipients: vec![person(2)],
				}),
			}),
		});
		assert!(
			state.group_members(Id(10)).is_none(),
			"a late snapshot cannot restore removed membership"
		);
		assert!(
			!state
				.channel(Id(10))
				.unwrap()
				.recipients
				.iter()
				.any(|user| user.id == Id(2))
		);
		assert_eq!(state.drafts[&Id(10)], "Keep group draft");
		members(&mut state, Id(1), vec![person(3)]);
		let remove = state.remove_group_member(Id(10), Id(3)).unwrap();
		finish(&mut state, remove, Ok(None));
		assert!(
			state.channel(Id(10)).is_some(),
			"removing a member must not leave the whole group"
		);
		assert!(state.channel(Id(10)).unwrap().recipients.is_empty());
	}
	#[test]
	fn group_actions_confirm_writes_keep_drafts_and_respect_newer_service_state() {
		{
			let mut state = state();
			assert!(state.close_dm(Id(10)).is_none());
			assert!(state.set_dm_muted(Id(10), true).is_some());
			assert!(
				state
					.edit_group(Id(10), Some(" ".into()), Patch::Absent)
					.is_none()
			);
			assert!(
				state
					.edit_group(Id(10), Some("x".repeat(101)), Patch::Absent)
					.is_none()
			);
			let edit = state
				.edit_group(Id(10), Some("New name".into()), Patch::Absent)
				.unwrap();
			assert_eq!(state.channel(Id(10)).unwrap().name, "Group");
			assert!(state.leave_group(Id(10)).is_none());
			finish(&mut state, edit, Err(Failure::Forbidden));
			assert_eq!(state.group_action_completed(Id(10), 1), Some(false));
			assert_eq!(state.channel(Id(10)).unwrap().name, "Group");
			let edit = state
				.edit_group(Id(10), Some("New name".into()), Patch::Absent)
				.unwrap();
			finish(&mut state, edit, Ok(Some(patch("New name"))));
			assert_eq!(state.channel(Id(10)).unwrap().name, "New name");
			assert_eq!(state.group_action_completed(Id(10), 2), Some(true));
			let edit = state
				.edit_group(Id(10), Some("Older".into()), Patch::Absent)
				.unwrap();
			state.apply(Envelope {
				generation: state.generation,
				event: CoreEvent::ChannelChanged(patch("Latest")),
			});
			finish(&mut state, edit, Ok(Some(patch("Older"))));
			assert_eq!(state.channel(Id(10)).unwrap().name, "Latest");
			let stale = state.leave_group(Id(10)).unwrap();
			state.cancel_group_action();
			let current = state.leave_group(Id(10)).unwrap();
			finish(&mut state, stale, Ok(None));
			assert!(state.group_action_pending());
			state.drafts.insert(Id(10), "Keep draft".into());
			finish(&mut state, current, Ok(None));
			assert!(state.channel(Id(10)).is_none());
			assert_eq!(state.selected, None);
			assert_eq!(state.drafts[&Id(10)], "Keep draft");
		}
		{
			let mut state = state();
			let name = "A long recipient name, ".repeat(8);
			state.channels[0].name = name.clone();
			state.channels[0].icon = Some("0123456789abcdef0123456789abcdef".into());
			assert!(state.edit_group(Id(10), None, Patch::Absent).is_none());
			let edit = state.edit_group(Id(10), None, Patch::Null).unwrap();
			let mut response = patch("unused");
			response.name = Patch::Absent;
			finish(&mut state, edit, Ok(Some(response)));
			assert_eq!(state.channels[0].name, name);
			assert!(state.channels[0].icon.is_none());
			let rename = state
				.edit_group(Id(10), Some("Renamed".into()), Patch::Absent)
				.unwrap();
			let mut response = patch("unused");
			response.name = Patch::Absent;
			finish(&mut state, rename, Ok(Some(response)));
			assert_eq!(state.group_action_completed(Id(10), 2), Some(false));
			assert_eq!(state.channels[0].name, name);
		}
	}
	#[test]
	fn group_leave_guards_rejoined_channel_pending_messages_and_session_reset() {
		let mut state = state();
		state.pending.push(crate::Pending {
			sticker: None,
			channel: Id(10),
			nonce: "pending".into(),
			content: "pending".into(),
			attachments: vec![],
			delivery: crate::Delivery::Sending,
			confirmed: None,
			reply: None,
		});
		assert!(state.leave_group(Id(10)).is_none());
		state.pending.clear();
		let leave = state.leave_group(Id(10)).unwrap();
		let channel = state.channel(Id(10)).unwrap().clone();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Unavailable(Id(10)),
		});
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ChannelCreated(channel),
		});
		finish(&mut state, leave, Ok(None));
		assert!(state.channel(Id(10)).is_some());
		let edit = state
			.edit_group(Id(10), Some("New name".into()), Patch::Absent)
			.unwrap();
		state.command_rejected(edit);
		assert!(!state.group_action_pending());
		let interrupted = state
			.edit_group(Id(10), Some("Interrupted".into()), Patch::Absent)
			.unwrap();
		let request = match &interrupted {
			Command::GroupAction { request, .. } => *request,
			_ => unreachable!(),
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Ready {
				user: model::User {
					username: None,
					primary_guild: None,
					cosmetics: None,
					id: Id(1),
					name: "Synthetic".into(),
					avatar: None,
					webhook: false,
					kind: Default::default(),
					discriminator: 0,
				},
				channels: state.channels.clone(),
				guilds: vec![],
				permissions: Default::default(),
			},
		});
		assert_eq!(state.group_action_completed(Id(10), request), Some(false));
		finish(&mut state, interrupted, Ok(Some(patch("Interrupted"))));
		assert_eq!(state.channel(Id(10)).unwrap().name, "Group");
		state.demo = false;
		state.gateway_connected = false;
		assert!(state.leave_group(Id(10)).is_none());
		state.demo = true;
		let leave = state.leave_group(Id(10)).unwrap();
		let Command::GroupAction { action, request } = leave else {
			unreachable!()
		};
		let generation = state.generation;
		state.logout();
		state.apply(Envelope {
			generation,
			event: CoreEvent::GroupAction(Event::Written {
				channel: action.channel(),
				request,
				result: Ok(None),
			}),
		});
		assert_eq!(state.group_action_completed(Id(10), request), None);
	}
}
