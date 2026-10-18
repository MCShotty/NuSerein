//! Offline group responses, including a bounded owner map so refreshes preserve transfers.
use client_core::{
	State,
	auth::Failure,
	group_actions::{Action, Event, Members},
};
use model::{Id, Patch};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Groups {
	generation: u64,
	owners: BTreeMap<Id, Id>,
}
impl Groups {
	pub fn respond(
		&mut self,
		state: &State,
		action: Action,
		request: u64,
		sequence: &mut u64,
	) -> Event {
		if self.generation != state.generation {
			self.generation = state.generation;
			self.owners.clear();
		}
		self.owners.retain(|channel, _| state.is_group_dm(*channel));
		let owner = state.user.as_ref().unwrap().id;
		if let Action::Create(users) = action {
			if self.owners.len() >= 64 {
				return Event::Created {
					request,
					result: Err(Failure::ProtocolAt("Offline group fixture is full")),
				};
			}
			*sequence += 1;
			let channel = Id(*sequence);
			let recipients: Vec<_> = users
				.iter()
				.filter_map(|id| state.friends().find(|friend| friend.id == *id).cloned())
				.collect();
			self.owners.insert(channel, owner);
			let group = model::Channel {
				id: channel,
				guild: None,
				kind: 3,
				name: recipients
					.iter()
					.map(|user| user.name.as_str())
					.collect::<Vec<_>>()
					.join(", "),
				icon: None,
				last_message: None,
				parent_id: None,
				position: 0,
				recipients: recipients.clone(),
				member_list_id: None,
				tags: None,
				message_count: None,
			};
			return Event::Created {
				request,
				result: Ok((
					Box::new(group),
					Members {
						channel,
						owner,
						recipients,
					},
				)),
			};
		}
		let channel = action.channel();
		if matches!(
			action,
			Action::LoadMembers(_) | Action::TransferOwner { .. }
		) {
			if !self.owners.contains_key(&channel) && self.owners.len() >= 64 {
				return Event::MembersLoaded {
					channel,
					request,
					result: Err(Failure::ProtocolAt("Offline group fixture is full")),
				};
			}
			let owner = self.owners.entry(channel).or_insert(owner);
			if let Action::TransferOwner { user, .. } = action {
				*owner = user;
			}
			return Event::MembersLoaded {
				channel,
				request,
				result: state
					.channel(channel)
					.map(|group| Members {
						channel,
						owner: *owner,
						recipients: group.recipients.clone(),
					})
					.ok_or(Failure::Forbidden),
			};
		}
		let patch = match action {
			Action::Edit { name, icon, .. } => Some(model::ChannelPatch {
				id: channel,
				name: name.map_or(Patch::Absent, Patch::Value),
				icon: match icon {
					Patch::Absent => state
						.channel(channel)
						.and_then(|group| group.icon.clone())
						.map_or(Patch::Null, Patch::Value),
					Patch::Null => Patch::Null,
					Patch::Value(_) => Patch::Value("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
				},
				last_message: Patch::Absent,
				parent_id: Patch::Absent,
				position: Patch::Absent,
				kind: Patch::Absent,
				message_count: Patch::Absent,
				tags: Patch::Absent,
			}),
			_ => None,
		};
		Event::Written {
			channel,
			request,
			result: Ok(patch),
		}
	}
}
