//! Permission and reference admission for community administration. No participant answers change.
use crate::State;
use model::{
	Id,
	community_admin::{Action, Data, Section, Snapshot},
	permissions as p,
};

impl State {
	pub fn can_open_community_settings(&self, guild: Id) -> bool {
		self.can_manage_guild(guild)
	}
	pub fn can_edit_community_settings(&self, guild: Id, section: Section) -> bool {
		self.can_manage_guild(guild)
			&& (section != Section::Onboarding || self.guild_permission(guild, p::MANAGE_ROLES))
	}
	pub fn community_channel_allowed(&self, guild: Id, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|c| c.guild == Some(guild) && matches!(c.kind, 0 | 2 | 5 | 13 | 15 | 16))
			&& self.can_view(channel)
	}
	pub fn community_guide_channel_allowed(&self, guild: Id, channel: Id, chat: bool) -> bool {
		self.community_channel_allowed(guild, channel)
			&& self.channel(channel).is_some_and(|c| {
				if chat {
					matches!(c.kind, 0 | 5)
				} else {
					matches!(c.kind, 0 | 5 | 15 | 16)
				}
			})
	}
	pub fn community_role_allowed(&self, guild: Id, role: Id) -> bool {
		let Some(snapshot) = self
			.server_admin
			.community
			.as_ref()
			.filter(|s| s.guild == guild)
		else {
			return false;
		};
		let Some(target) = snapshot.roles.iter().find(|r| r.id == role) else {
			return false;
		};
		if target.managed || target.id == guild {
			return false;
		}
		let Some(metadata) = self.permissions.guilds.get(&guild) else {
			return false;
		};
		let Some(roles) = metadata.roles.as_ref() else {
			return false;
		};
		let Some(current) = roles.iter().find(|r| r.id == role) else {
			return false;
		};
		if current.bits != target.bits || current.position != target.position {
			return false;
		}
		if metadata.owner == self.user.as_ref().map(|u| u.id) {
			return true;
		}
		metadata.member.as_ref().is_some_and(|member| {
			roles
				.iter()
				.filter(|r| member.roles.contains(&r.id))
				.any(|r| r.cmp_hierarchy(current).is_gt())
		})
	}
	/// Permissions of @everyone only, including its channel overwrite; owner bypass never counts.
	pub fn community_everyone_can(&self, guild: Id, channel: Id, bits: u128) -> bool {
		let Some(metadata) = self.permissions.guilds.get(&guild) else {
			return false;
		};
		let Some(role) = metadata
			.roles
			.as_ref()
			.and_then(|r| r.iter().find(|r| r.id == guild))
		else {
			return false;
		};
		let Some(overwrites) = self
			.permissions
			.channels
			.get(&channel)
			.filter(|c| c.guild == guild)
			.and_then(|c| c.overwrites.as_ref())
		else {
			return false;
		};
		let mut value = role.bits;
		if value & p::ADMINISTRATOR != 0 {
			return true;
		}
		if let Some(row) = overwrites.iter().find(|o| o.kind == 0 && o.id == guild) {
			value = (value & !row.deny) | row.allow;
		}
		value & bits == bits
	}
	pub fn community_author_allowed(&self, guild: Id, user: Id) -> bool {
		let Some(metadata) = self.permissions.guilds.get(&guild) else {
			return false;
		};
		if metadata.owner == Some(user) {
			return true;
		}
		if self.user.as_ref().is_some_and(|u| u.id == user) {
			return self.guild_permission(guild, p::MANAGE_GUILD)
				|| self.guild_permission(guild, p::MANAGE_ROLES);
		}
		let Some(member) = self
			.server_admin
			.members
			.as_ref()
			.and_then(|p| p.items.iter().find(|m| m.user.id == user))
		else {
			return false;
		};
		let Some(roles) = metadata.roles.as_ref() else {
			return false;
		};
		let mut value = 0;
		for id in std::iter::once(guild).chain(member.roles.iter().copied()) {
			let Some(role) = roles.iter().find(|r| r.id == id) else {
				return false;
			};
			value |= role.bits;
		}
		value & (p::ADMINISTRATOR | p::MANAGE_GUILD | p::MANAGE_ROLES) != 0
	}
	pub(super) fn community_admin_action_allowed(&self, guild: Id, action: &Action) -> bool {
		if !action.valid() || !self.can_open_community_settings(guild) {
			return false;
		}
		if let Action::Artwork {
			channel,
			resource,
			before,
			..
		} = action
		{
			let Some(snapshot) = self
				.server_admin
				.community
				.as_ref()
				.filter(|s| s.guild == guild)
			else {
				return false;
			};
			let Data::Guide(guide) = &snapshot.data else {
				return false;
			};
			return if let Some(channel) = channel {
				self.community_channel_allowed(guild, *channel)
					&& (if *resource {
						&guide.resources
					} else {
						&guide.actions
					})
					.iter()
					.any(|row| row.channel == *channel && row.icon == *before)
			} else {
				guide.header_available && guide.header == *before
			};
		}
		let Action::Edit { before, after, .. } = action else {
			return true;
		};
		if !self.can_edit_community_settings(guild, action.section())
			|| !self
				.server_admin
				.community
				.as_ref()
				.is_some_and(|s| s.guild == guild && s.data == **before)
		{
			return false;
		}
		match (&**before, &**after) {
			(Data::Onboarding(old), Data::Onboarding(new)) => {
				// Existing references may be removed even when a channel/role became unavailable.
				if new.default_channels != old.default_channels
					&& !new
						.default_channels
						.iter()
						.all(|id| self.community_channel_allowed(guild, *id))
				{
					return false;
				}
				if new.prompts != old.prompts {
					for prompt in &new.prompts {
						for option in &prompt.options {
							let previous = old
								.prompts
								.iter()
								.flat_map(|p| &p.options)
								.find(|o| o.id == option.id);
							if previous.is_none_or(|old| old.channels != option.channels)
								&& !option
									.channels
									.iter()
									.all(|id| self.community_channel_allowed(guild, *id))
							{
								return false;
							}
							if previous.is_none_or(|old| old.roles != option.roles)
								&& !option
									.roles
									.iter()
									.all(|id| self.community_role_allowed(guild, *id))
							{
								return false;
							}
						}
					}
				}
				if new.enabled && new.mode <= 1 {
					let mut channels = new.default_channels.clone();
					if new.mode == 1 {
						for option in new
							.prompts
							.iter()
							.filter(|p| p.in_onboarding)
							.flat_map(|p| &p.options)
						{
							for id in &option.channels {
								if !channels.contains(id) {
									channels.push(*id);
								}
							}
						}
					}
					// Published API constraints; the service validates feature-specific criteria too.
					if channels.len() < 7
						|| channels
							.iter()
							.filter(|id| {
								self.community_everyone_can(
									guild,
									**id,
									p::VIEW_CHANNEL | p::SEND_MESSAGES,
								)
							})
							.count() < 5
					{
						return false;
					}
				}
				true
			}
			(Data::Screening(_), Data::Screening(_)) => true,
			(Data::Guide(old), Data::Guide(new)) => {
				if new.authors != old.authors
					&& !new
						.authors
						.iter()
						.all(|id| self.community_author_allowed(guild, *id))
				{
					return false;
				}
				new.actions.iter().chain(&new.resources).all(|r| {
					self.community_guide_channel_allowed(
						guild,
						r.channel,
						new.actions
							.iter()
							.any(|a| a.channel == r.channel && a.action == 1),
					) && (!new.enabled
						|| self.community_everyone_can(guild, r.channel, p::VIEW_CHANNEL))
				}) && (!new.enabled
					|| new.actions.iter().filter(|r| r.action == 1).all(|r| {
						self.community_everyone_can(
							guild,
							r.channel,
							p::VIEW_CHANNEL | p::SEND_MESSAGES,
						)
					}))
			}
			_ => false,
		}
	}
	pub(super) fn community_snapshot_current(&self, snapshot: &Snapshot) -> bool {
		self.permissions
			.guilds
			.get(&snapshot.guild)
			.and_then(|g| g.roles.as_ref())
			.is_some_and(|roles| {
				roles.len() == snapshot.roles.len()
					&& snapshot.roles.iter().all(|r| {
						roles
							.iter()
							.any(|p| p.id == r.id && p.bits == r.bits && p.position == r.position)
					})
			})
	}
	pub(crate) fn observe_community_event(&mut self, event: &crate::Event) {
		let Some(snapshot) = self.server_admin.community.as_ref() else {
			return;
		};
		let relevant = match event {
			crate::Event::Onboarding(crate::onboarding::Event::Gates { gates, .. }) => {
				!self.server_admin.pending
					&& gates
						.iter()
						.any(|g| g.guild == snapshot.guild && g.features.is_some())
			}
			crate::Event::Unavailable(id) => self
				.channel(*id)
				.is_some_and(|c| c.guild == Some(snapshot.guild)),
			crate::Event::ChannelChanged(patch) => self
				.channel(patch.id)
				.is_some_and(|c| c.guild == Some(snapshot.guild)),
			_ => false,
		};
		if relevant {
			self.server_admin.community = None;
			self.server_admin.invalidate_community();
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::{
		Command, Envelope, Event,
		auth::{AuthState, Failure},
		server_admin,
	};
	use model::{
		community_admin as m,
		server_admin::{Action as AdminAction, Result as Outcome},
	};
	fn role(id: u64, bits: u128, position: i32) -> p::Role {
		p::Role {
			icon: None,
			unicode_emoji: None,
			id: Id(id),
			name: format!("Role {id}"),
			bits,
			position,
			color: 0,
			secondary_color: None,
			tertiary_color: None,
			hoist: false,
		}
	}
	pub(crate) fn state(bits: u128) -> State {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			user: Some(model::User {
				member_avatar: None,
				username: None,
				id: Id(1),
				name: "Synthetic".into(),
				avatar: None,
				discriminator: 0,
				kind: Default::default(),
				webhook: false,
				primary_guild: None,
				cosmetics: None,
			}),
			guilds: vec![model::Guild {
				id: Id(2),
				name: "Synthetic".into(),
				icon: None,
				emojis: None,
				stickers: None,
				default_message_notifications: None,
			}],
			..Default::default()
		};
		state.permissions.guilds.insert(
			Id(2),
			p::Guild {
				id: Id(2),
				owner: Some(Id(99)),
				roles: Some(vec![
					role(2, p::VIEW_CHANNEL | p::SEND_MESSAGES, 0),
					role(3, bits, 10),
					role(4, 0, 1),
					role(5, 0, 11),
					role(6, 0, 2),
				]),
				member: Some(p::Member {
					roles: vec![Id(3)],
					timeout_until: None,
				}),
			},
		);
		for id in 10..18 {
			state.channels.push(model::Channel {
				id: Id(id),
				guild: Some(Id(2)),
				name: format!("channel-{id}"),
				kind: 0,
				icon: None,
				last_message: None,
				parent_id: None,
				position: id as i32,
				recipients: vec![],
				member_list_id: None,
				message_count: None,
				tags: None,
			});
			state.permissions.channels.insert(
				Id(id),
				p::Channel {
					id: Id(id),
					guild: Id(2),
					overwrites: Some(vec![]),
				},
			);
		}
		state.server_admin.guild = Some(Id(2));
		state.server_admin.community = Some(snapshot(&state));
		state
	}
	fn snapshot(state: &State) -> Snapshot {
		Snapshot {
			guild: Id(2),
			data: Data::Onboarding(m::Onboarding {
				default_channels: (10..18).map(Id).collect(),
				prompts: vec![m::Prompt {
					id: Id(100),
					title: "Choose".into(),
					options: vec![m::OptionRow {
						id: Id(101),
						title: "One".into(),
						..Default::default()
					}],
					..Default::default()
				}],
				..Default::default()
			}),
			roles: state.permissions.guilds[&Id(2)]
				.roles
				.as_ref()
				.unwrap()
				.iter()
				.map(|r| m::Role {
					id: r.id,
					name: r.name.clone(),
					bits: r.bits,
					position: r.position,
					managed: r.id == Id(6),
				})
				.collect(),
		}
	}
	fn edit(state: &State) -> Action {
		let before = state.server_admin.community.as_ref().unwrap().data.clone();
		let mut after = before.clone();
		let Data::Onboarding(o) = &mut after else {
			panic!()
		};
		o.mode = 1;
		Action::Edit {
			before: Box::new(before),
			after: Box::new(after),
			pending: Default::default(),
		}
	}
	fn request(state: &mut State, action: Action) -> u64 {
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(Id(2), AdminAction::Community(action))
			.unwrap()
		else {
			panic!()
		};
		request
	}
	fn deliver(state: &mut State, request: u64, result: Result<Snapshot, Failure>) {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ServerAdmin(server_admin::Event {
				guild: Id(2),
				request,
				result: result.map(Outcome::Community),
			}),
		});
	}
	#[test]
	fn community_permission_and_role_hierarchy_admission() {
		let mut manager = state(p::MANAGE_GUILD);
		assert!(manager.community_admin_action_allowed(Id(2), &Action::Load(Section::Onboarding)));
		assert!(!manager.community_admin_action_allowed(Id(2), &edit(&manager)));
		manager.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(1));
		manager.permissions.clear_cache();
		assert!(manager.community_admin_action_allowed(Id(2), &edit(&manager)));
		let administrator = state(p::ADMINISTRATOR);
		assert!(administrator.community_admin_action_allowed(Id(2), &edit(&administrator)));
		assert!(administrator.community_role_allowed(Id(2), Id(4)));
		assert!(!administrator.community_role_allowed(Id(2), Id(5)));
		assert!(!administrator.community_role_allowed(Id(2), Id(6)));
		assert!(!administrator.community_role_allowed(Id(2), Id(2)));
		assert!(!state(0).can_open_community_settings(Id(2)));
	}
	#[test]
	fn community_channel_deletion_fences_pending_write_and_stale_response() {
		let mut state = state(p::MANAGE_GUILD | p::MANAGE_ROLES);
		let action = edit(&state);
		let request = request(&mut state, action);
		let response = snapshot(&state);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Unavailable(Id(10)),
		});
		assert!(state.server_admin.community.is_none());
		deliver(&mut state, request, Ok(response));
		assert!(state.server_admin.community.is_none());
		assert!(state.server_admin.needs_refresh);
	}
	#[test]
	fn community_role_removal_revokes_reference_and_dispatch_admission() {
		let mut state = state(p::MANAGE_GUILD | p::MANAGE_ROLES);
		let action = edit(&state);
		let request = request(&mut state, action.clone());
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::RoleRemoved {
				guild: Id(2),
				id: Id(4),
			}),
		});
		assert!(state.server_admin.community.is_none());
		assert!(!state.server_admin_command_allowed(
			Id(2),
			request,
			&AdminAction::Community(action)
		));
	}
	#[test]
	fn community_unknown_write_requires_deliberate_reload_and_disconnect_fences_late_result() {
		let mut state = state(p::MANAGE_GUILD | p::MANAGE_ROLES);
		let action = edit(&state);
		let request_id = request(&mut state, action.clone());
		deliver(&mut state, request_id, Err(Failure::Ambiguous));
		assert!(state.server_admin.needs_refresh);
		assert!(state.server_admin.community.is_none());
		// Successful loads from another settings page may clear the shared notice, but cannot
		// restore the community baseline after an uncertain write.
		state.server_admin.needs_refresh = false;
		assert!(
			state
				.request_server_admin(Id(2), AdminAction::Community(action.clone()))
				.is_none()
		);
		let request_id = request(&mut state, Action::Load(Section::Onboarding));
		let fresh = snapshot(&state);
		deliver(&mut state, request_id, Ok(fresh));
		assert!(!state.server_admin.needs_refresh);
		let request_id = request(&mut state, action);
		let fresh = snapshot(&state);
		state.cancel_server_admin();
		deliver(&mut state, request_id, Ok(fresh));
		assert!(state.server_admin.needs_refresh);
		assert!(!state.server_admin.pending);
	}
	#[test]
	fn community_owner_does_not_satisfy_everyone_channel_requirements() {
		let mut state = state(p::ADMINISTRATOR);
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(1));
		for channel in state.permissions.channels.values_mut() {
			channel.overwrites = Some(vec![p::Overwrite {
				id: Id(2),
				kind: 0,
				allow: 0,
				deny: p::SEND_MESSAGES,
			}]);
		}
		state.permissions.clear_cache();
		let mut action = edit(&state);
		let Action::Edit { after, .. } = &mut action else {
			panic!()
		};
		let Data::Onboarding(o) = &mut **after else {
			panic!()
		};
		o.enabled = true;
		assert!(!state.community_admin_action_allowed(Id(2), &action));
	}
}
