//! Current permission/hierarchy checks shared by settings and the desktop dispatch gate.
use crate::State;
use model::{Id, automod as a, moderation as m, permissions as p};
impl State {
	pub fn can_open_mod_view(&self, guild: Id) -> bool {
		self.can_open_moderation(guild)
			|| self.can_manage_guild(guild)
			|| self.guild_permission(guild, p::MANAGE_ROLES)
			|| self.guild_permission(guild, p::MANAGE_NICKNAMES)
	}
	pub fn can_open_moderation(&self, guild: Id) -> bool {
		[
			p::KICK_MEMBERS,
			p::BAN_MEMBERS,
			p::MODERATE_MEMBERS,
			p::VIEW_AUDIT_LOG,
			p::MUTE_MEMBERS,
			p::DEAFEN_MEMBERS,
			p::MOVE_MEMBERS,
		]
		.into_iter()
		.any(|bits| self.guild_permission(guild, bits))
	}
	pub fn can_view_guild_bans(&self, guild: Id) -> bool {
		[p::BAN_MEMBERS, p::VIEW_AUDIT_LOG]
			.into_iter()
			.any(|bits| self.guild_permission(guild, bits))
	}
	pub fn can_ban_guild_member(&self, guild: Id, user: Id) -> bool {
		self.guild_permission(guild, p::BAN_MEMBERS) && self.above_admin_member(guild, user)
	}
	pub fn can_timeout_guild_member(&self, guild: Id, user: Id) -> bool {
		self.guild_permission(guild, p::MODERATE_MEMBERS)
			&& self.above_admin_member(guild, user)
			&& self
				.admin_member(guild, user)
				.zip(
					self.permissions
						.guilds
						.get(&guild)
						.and_then(|guild| guild.roles.as_ref()),
				)
				.is_some_and(|(member, roles)| {
					if !roles.iter().any(|role| role.id == guild)
						|| member
							.roles
							.iter()
							.any(|id| !roles.iter().any(|role| role.id == *id))
					{
						return false;
					}

					roles
						.iter()
						.filter(|role| role.id == guild || member.roles.contains(&role.id))
						.all(|role| role.bits & p::ADMINISTRATOR == 0)
				})
	}
	pub(super) fn moderation_action_allowed(&self, guild: Id, action: &m::Action) -> bool {
		if !self.can_open_mod_view(guild) || !action.valid() {
			return false;
		}
		match action {
			m::Action::BanSearch { .. } => self.can_view_guild_bans(guild),
			m::Action::Bans { after } => {
				self.can_view_guild_bans(guild)
					&& after.is_none_or(|after| {
						self.server_admin
							.bans
							.as_ref()
							.is_some_and(|page| page.guild == guild && page.next == Some(after))
					})
			}
			m::Action::Review {
				user,
				channel,
				audit,
			} => {
				self.user.as_ref().is_none_or(|actor| actor.id != *user)
					&& self
						.channel(*channel)
						.is_some_and(|row| row.guild == Some(guild))
					&& self.can_view(*channel)
					&& (!audit || self.guild_permission(guild, p::VIEW_AUDIT_LOG))
			}
			m::Action::BulkBan { users, .. } => {
				self.can_manage_guild(guild)
					&& users
						.iter()
						.all(|user| self.can_ban_guild_member(guild, *user))
					&& (self.server_admin.pending
						&& self
							.server_admin
							.bulk_report
							.as_ref()
							.is_some_and(|report| report.matches(users))
						|| self.server_admin.bulk_report.as_ref().is_none_or(|report| {
							report.users.iter().all(|(_, status)| {
								matches!(status, m::BanStatus::Banned | m::BanStatus::Failed)
							})
						}))
			}
			m::Action::BulkRefresh { users } => {
				self.can_view_guild_bans(guild)
					&& self
						.server_admin
						.bulk_report
						.as_ref()
						.is_some_and(|report| report.matches(users))
			}
			m::Action::Member(user) => {
				self.user.as_ref().is_none_or(|actor| actor.id != *user)
					|| self.guild_permission(guild, p::MANAGE_ROLES)
			}
			m::Action::Unban { user, .. } => {
				self.guild_permission(guild, p::BAN_MEMBERS)
					&& self.server_admin.bans.as_ref().is_some_and(|page| {
						page.guild == guild && page.items.iter().any(|row| row.user.id == *user)
					})
			}
			m::Action::Change { user, change, .. } => match change {
				m::Change::Kick => self.can_kick_guild_member(guild, *user),
				m::Change::Ban { .. } => self.can_ban_guild_member(guild, *user),
				m::Change::Timeout { until } => {
					self.can_timeout_guild_member(guild, *user)
						&& until.is_none_or(|until| {
							let now = std::time::SystemTime::now()
								.duration_since(std::time::UNIX_EPOCH)
								.unwrap_or_default()
								.as_nanos() as i128;
							until > now && until - now <= 2_419_200 * 1_000_000_000_i128
						})
				}
			},
		}
	}
	pub(super) fn automod_action_allowed(&self, guild: Id, action: &a::Action) -> bool {
		if !self.can_manage_guild(guild) || !action.valid() {
			return false;
		}
		let mut previous = None;
		let settings = match action {
			a::Action::Load => return true,
			a::Action::Create { settings, .. } => settings.as_ref(),
			a::Action::Edit {
				id, before, after, ..
			} => {
				if !self.server_admin.automod.as_ref().is_some_and(|page| {
					page.guild == guild
						&& page
							.rules
							.iter()
							.any(|rule| rule.id == *id && rule.settings == **before)
				}) {
					return false;
				}
				previous = Some(before.as_ref());
				after.as_ref()
			}
			a::Action::Delete { id, .. } => {
				return self.server_admin.automod.as_ref().is_some_and(|page| {
					page.guild == guild && page.rules.iter().any(|rule| rule.id == *id)
				});
			}
		};
		settings.effects.iter().all(|effect| {
			effect.kind != 3
				|| previous.is_some_and(|before| before.effects.contains(effect))
				|| self.guild_permission(guild, p::MODERATE_MEMBERS)
		}) && settings.effects.iter().all(|effect| {
			effect.channel.is_none_or(|id| {
				previous.is_some_and(|before| {
					before
						.effects
						.iter()
						.any(|old| old.kind == effect.kind && old.channel == Some(id))
				}) || self
					.channel(id)
					.is_some_and(|channel| channel.guild == Some(guild) && self.can_view(id))
			})
		}) && settings
			.exempt_channels
			.iter()
			.filter(|id| !previous.is_some_and(|before| before.exempt_channels.contains(id)))
			.all(|id| {
				self.channel(*id)
					.is_some_and(|channel| channel.guild == Some(guild) && self.can_view(*id))
			}) && settings
			.exempt_roles
			.iter()
			.filter(|id| !previous.is_some_and(|before| before.exempt_roles.contains(id)))
			.all(|id| {
				self.permissions
					.guilds
					.get(&guild)
					.and_then(|guild| guild.roles.as_ref())
					.is_some_and(|roles| roles.iter().any(|role| role.id == *id))
			})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command,
		auth::{AuthState, Failure},
		server_admin,
	};
	use model::server_admin::{Action, Member, Result as Outcome};
	fn user(id: u64) -> model::User {
		model::User {
			username: None,
			id: Id(id),
			name: "Synthetic".into(),
			avatar: None,
			discriminator: 0,
			kind: Default::default(),
			webhook: false,
			primary_guild: None,
			cosmetics: None,
		}
	}
	fn role(id: u64, position: i32, bits: u128) -> p::Role {
		p::Role {
			icon: None,
			unicode_emoji: None,
			id: Id(id),
			name: "Synthetic role".into(),
			position,
			bits,
			color: 0,
			secondary_color: None,
			tertiary_color: None,
			hoist: false,
		}
	}
	fn member(id: u64, roles: Vec<Id>) -> Member {
		Member {
			user: user(id),
			nick: None,
			roles,
			joined_at: None,
			join_source: None,
			invite_code: None,
			flags: None,
			unusual_dm_until: None,
			timeout_until: None,
		}
	}
	fn state(bits: u128) -> State {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			user: Some(user(1)),
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
					role(2, 0, 0),
					role(3, 5, bits),
					role(4, 1, 0),
					role(5, 6, p::ADMINISTRATOR),
				]),
				member: Some(p::Member {
					roles: vec![Id(3)],
					timeout_until: None,
				}),
			},
		);
		state.server_admin.guild = Some(Id(2));
		state.server_admin.moderation_target = Some(member(7, vec![Id(4)]));
		state
	}
	#[test]
	fn audit_log_permission_can_read_bans_without_allowing_moderation_writes() {
		let mut state = state(p::VIEW_AUDIT_LOG);
		assert!(state.can_open_moderation(Id(2)) && state.can_view_guild_bans(Id(2)));
		let Some(Command::ServerAdmin { request, .. }) =
			state.request_server_admin(Id(2), Action::Moderation(m::Action::Bans { after: None }))
		else {
			panic!()
		};
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Bans(m::Bans {
					guild: Id(2),
					items: vec![m::Ban {
						user: user(9),
						reason: None,
					}],
					next: Some(Id(9)),
				}))),
			})
			.unwrap();
		assert!(state.server_admin.bans.is_some());
		assert!(state.moderation_action_allowed(Id(2), &m::Action::Bans { after: Some(Id(9)) }));
		assert!(!state.moderation_action_allowed(
			Id(2),
			&m::Action::Unban {
				user: Id(9),
				reason: String::new()
			}
		));
		assert!(!state.can_ban_guild_member(Id(2), Id(7)));
		assert!(!state.can_kick_guild_member(Id(2), Id(7)));
		assert!(!state.can_timeout_guild_member(Id(2), Id(7)));
		state
			.permissions
			.guilds
			.get_mut(&Id(2))
			.unwrap()
			.roles
			.as_mut()
			.unwrap()[1]
			.bits = 0;
		state.permissions.clear_cache();
		assert!(!state.can_open_moderation(Id(2)) && !state.can_view_guild_bans(Id(2)));
	}

	#[test]
	fn moderation_checks_member_moderator_admin_owner_and_hierarchy() {
		let mut state = state(0);
		assert!(!state.can_open_moderation(Id(2)));
		assert!(!state.can_ban_guild_member(Id(2), Id(7)));
		state
			.permissions
			.guilds
			.get_mut(&Id(2))
			.unwrap()
			.roles
			.as_mut()
			.unwrap()[1]
			.bits = p::KICK_MEMBERS | p::BAN_MEMBERS | p::MODERATE_MEMBERS;
		state.permissions.clear_cache();
		assert!(
			state.can_kick_guild_member(Id(2), Id(7))
				&& state.can_ban_guild_member(Id(2), Id(7))
				&& state.can_timeout_guild_member(Id(2), Id(7))
		);
		for roles in [vec![Id(3)], vec![Id(5)], vec![Id(999)]] {
			state.server_admin.moderation_target.as_mut().unwrap().roles = roles;
			assert!(!state.can_ban_guild_member(Id(2), Id(7)));
			assert!(!state.can_timeout_guild_member(Id(2), Id(7)));
		}
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(1));
		state.permissions.clear_cache();
		state.server_admin.moderation_target.as_mut().unwrap().roles = vec![Id(5)];
		assert!(state.can_ban_guild_member(Id(2), Id(7)));
		assert!(
			!state.can_timeout_guild_member(Id(2), Id(7)),
			"administrators cannot be timed out, even by the owner"
		);
		for target in [1, 99] {
			state.server_admin.moderation_target = Some(member(target, vec![Id(4)]));
			if target == 99 {
				state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(99));
			}
			assert!(!state.can_ban_guild_member(Id(2), Id(target)));
		}
	}
	#[test]
	fn moderation_uncertain_writes_and_stale_permission_results_require_refresh() {
		let mut state = state(p::BAN_MEMBERS);
		let ban = || {
			Action::Moderation(m::Action::Change {
				user: Id(7),
				change: m::Change::Ban { delete_seconds: 0 },
				reason: "Reviewed".into(),
			})
		};
		let Command::ServerAdmin { request, .. } =
			state.request_server_admin(Id(2), ban()).unwrap()
		else {
			panic!()
		};
		assert!(state.request_server_admin(Id(2), ban()).is_none());
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request: request + 1,
				result: Err(Failure::Ambiguous),
			})
			.unwrap();
		assert!(state.server_admin.pending);
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Err(Failure::Ambiguous),
			})
			.unwrap();
		assert!(
			state.server_admin.needs_refresh && state.request_server_admin(Id(2), ban()).is_none()
		);
		state.server_admin.needs_refresh = false;
		let Command::ServerAdmin { request, .. } =
			state.request_server_admin(Id(2), ban()).unwrap()
		else {
			panic!()
		};
		state
			.server_admin
			.permissions_changed(&crate::permissions::Event::Owner {
				guild: Id(2),
				owner: model::Patch::Value(Id(99)),
			});
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Changed {
					user: Id(7),
					member: None,
				})),
			})
			.unwrap();
		assert!(state.server_admin.needs_refresh);
	}
	#[test]
	fn moderation_lookup_and_confirmed_changes_retire_only_their_own_cached_state() {
		let mut state = state(p::KICK_MEMBERS | p::BAN_MEMBERS | p::MODERATE_MEMBERS);
		let request = |state: &mut State, action| {
			let Some(Command::ServerAdmin { request, .. }) =
				state.request_server_admin(Id(2), Action::Moderation(action))
			else {
				panic!("moderation command rejected")
			};
			request
		};
		let deliver = |state: &mut State, request, result| {
			state
				.apply_server_admin(server_admin::Event {
					guild: Id(2),
					request,
					result,
				})
				.unwrap()
		};
		let id = request(&mut state, m::Action::Member(Id(8)));
		assert!(state.server_admin.moderation_target.is_none());
		deliver(&mut state, id, Err(Failure::Forbidden));
		assert!(state.server_admin.moderation_target.is_none());
		state.server_admin.moderation_target = Some(member(7, vec![Id(4)]));
		state.server_admin.members = Some(model::server_admin::Members {
			items: vec![member(7, vec![Id(4)]), member(8, vec![Id(4)])],
			total: 2,
			..Default::default()
		});
		let id = request(
			&mut state,
			m::Action::Change {
				user: Id(7),
				change: m::Change::Timeout { until: None },
				reason: String::new(),
			},
		);
		let mut updated = member(7, vec![Id(4)]);
		updated.nick = Some("Updated member".into());
		deliver(
			&mut state,
			id,
			Ok(Outcome::Moderation(m::Outcome::Changed {
				user: Id(7),
				member: Some(Box::new(updated)),
			})),
		);
		assert_eq!(
			state.server_admin.members.as_ref().unwrap().items[0]
				.nick
				.as_deref(),
			Some("Updated member")
		);
		let id = request(
			&mut state,
			m::Action::Change {
				user: Id(7),
				change: m::Change::Kick,
				reason: String::new(),
			},
		);
		deliver(
			&mut state,
			id,
			Ok(Outcome::Moderation(m::Outcome::Changed {
				user: Id(7),
				member: None,
			})),
		);
		assert!(state.server_admin.moderation_target.is_none());
		let page = state.server_admin.members.as_ref().unwrap();
		assert_eq!(page.total, 1);
		assert_eq!(page.items[0].user.id, Id(8));
		state.server_admin.moderation_target = Some(member(8, vec![Id(4)]));
		state.server_admin.bans = Some(m::Bans {
			guild: Id(2),
			items: vec![m::Ban {
				user: user(9),
				reason: None,
			}],
			next: Some(Id(9)),
		});
		let id = request(
			&mut state,
			m::Action::Unban {
				user: Id(9),
				reason: String::new(),
			},
		);
		deliver(
			&mut state,
			id,
			Ok(Outcome::Moderation(m::Outcome::Changed {
				user: Id(9),
				member: None,
			})),
		);
		assert_eq!(
			state
				.server_admin
				.moderation_target
				.as_ref()
				.unwrap()
				.user
				.id,
			Id(8)
		);
		let page = state.server_admin.bans.as_ref().unwrap();
		assert!(page.items.is_empty() && page.valid());
		assert!(state.moderation_action_allowed(Id(2), &m::Action::Bans { after: Some(Id(9)) }));
		assert!(!state.moderation_action_allowed(Id(3), &m::Action::Bans { after: Some(Id(9)) }));
	}
	#[test]
	fn automod_preserves_unchanged_unknown_references_and_checks_new_actions() {
		let mut state = state(p::MANAGE_GUILD);
		let mut before = a::Settings {
			name: "Synthetic rule".into(),
			metadata: a::Metadata {
				keywords: vec!["example".into()],
				..Default::default()
			},
			exempt_roles: vec![Id(999)],
			..Default::default()
		};
		before.effects.push(a::Effect {
			kind: 3,
			duration: 60,
			..Default::default()
		});
		state.server_admin.automod = Some(a::Snapshot {
			guild: Id(2),
			rules: vec![a::Rule {
				id: Id(10),
				guild: Id(2),
				creator: Id(1),
				settings: before.clone(),
			}],
		});
		let mut after = before.clone();
		after.name = "Renamed".into();
		let edit = |after| a::Action::Edit {
			id: Id(10),
			before: Box::new(before.clone()),
			after: Box::new(after),
			reason: String::new(),
		};
		assert!(state.automod_action_allowed(Id(2), &edit(after.clone())));
		after.effects[1].duration = 120;
		assert!(!state.automod_action_allowed(Id(2), &edit(after.clone())));
		after = before.clone();
		after.exempt_roles.push(Id(998));
		assert!(!state.automod_action_allowed(Id(2), &edit(after)));
		// A hidden exemption may be preserved, but must not authorize a new alert destination.
		let mut before = before.clone();
		before.exempt_channels = vec![Id(999)];
		state.server_admin.automod.as_mut().unwrap().rules[0].settings = before.clone();
		let mut after = before.clone();
		after.name = "Renamed".into();
		let edit = |after| a::Action::Edit {
			id: Id(10),
			before: Box::new(before.clone()),
			after: Box::new(after),
			reason: String::new(),
		};
		assert!(state.automod_action_allowed(Id(2), &edit(after.clone())));
		after.effects.push(a::Effect {
			kind: 2,
			channel: Some(Id(999)),
			..Default::default()
		});
		assert!(!state.automod_action_allowed(Id(2), &edit(after)));
	}
	#[test]
	fn bulk_acknowledgement_is_not_completion_and_only_exact_active_targets_are_admitted() {
		let mut state = state(p::BAN_MEMBERS | p::MANAGE_GUILD);
		state.server_admin.members = Some(model::server_admin::Members {
			items: vec![member(7, vec![Id(4)]), member(8, vec![Id(4)])],
			total: 2,
			..Default::default()
		});
		let action = || m::Action::BulkBan {
			users: vec![Id(7), Id(8)],
			delete_seconds: 0,
			reason: "Reviewed".into(),
		};
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(Id(2), Action::Moderation(action()))
			.unwrap()
		else {
			panic!()
		};
		assert!(state.server_admin_command_allowed(Id(2), request, &Action::Moderation(action())));
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Bulk(
					m::BulkReport::pending(&[Id(7), Id(8)]),
				))),
			})
			.unwrap();
		assert!(
			state
				.server_admin
				.bulk_report
				.as_ref()
				.unwrap()
				.users
				.iter()
				.all(|(_, status)| *status == m::BanStatus::Pending)
		);
		assert_eq!(state.server_admin.members.as_ref().unwrap().items.len(), 2);
		assert!(
			state
				.request_server_admin(Id(2), Action::Moderation(action()))
				.is_none()
		);
		let report = |users| m::BulkReport {
			users,
			accepted: true,
		};
		state.apply_bulk_moderation(
			Id(3),
			report(vec![
				(Id(7), m::BanStatus::Banned),
				(Id(8), m::BanStatus::Failed),
			]),
		);
		state.apply_bulk_moderation(
			Id(2),
			report(vec![
				(Id(99), m::BanStatus::Banned),
				(Id(8), m::BanStatus::Failed),
			]),
		);
		assert!(
			state
				.server_admin
				.bulk_report
				.as_ref()
				.unwrap()
				.users
				.iter()
				.all(|(_, status)| *status == m::BanStatus::Pending)
		);
		state.apply_bulk_moderation(
			Id(2),
			report(vec![
				(Id(7), m::BanStatus::Banned),
				(Id(8), m::BanStatus::Failed),
			]),
		);
		assert_eq!(
			state.server_admin.members.as_ref().unwrap().items[0]
				.user
				.id,
			Id(8)
		);
		state.close_server_admin();
		state.apply_bulk_moderation(
			Id(2),
			report(vec![
				(Id(7), m::BanStatus::Banned),
				(Id(8), m::BanStatus::Failed),
			]),
		);
		assert!(state.server_admin.bulk_report.is_none());
	}
	#[test]
	fn gateway_completion_before_http_ack_remains_confirmed() {
		let mut state = state(p::BAN_MEMBERS | p::MANAGE_GUILD);
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(
				Id(2),
				Action::Moderation(m::Action::BulkBan {
					users: vec![Id(7)],
					delete_seconds: 0,
					reason: String::new(),
				}),
			)
			.unwrap()
		else {
			panic!()
		};
		state.apply_bulk_moderation(
			Id(2),
			m::BulkReport {
				users: vec![(Id(7), m::BanStatus::Banned)],
				accepted: true,
			},
		);
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Bulk(
					m::BulkReport::pending(&[Id(7)]),
				))),
			})
			.unwrap();
		assert_eq!(
			state.server_admin.bulk_report.as_ref().unwrap().users[0].1,
			m::BanStatus::Banned
		);
	}
	#[test]
	fn unconfirmed_bulk_deadline_becomes_unknown_instead_of_failed_or_retried() {
		let mut state = state(p::BAN_MEMBERS | p::MANAGE_GUILD);
		state.server_admin.bulk_report = Some(m::BulkReport::pending(&[Id(7)]));
		state.server_admin.bulk_started =
			Some(std::time::Instant::now() - std::time::Duration::from_secs(31));
		state.expire_bulk_moderation();
		assert_eq!(
			state.server_admin.bulk_report.as_ref().unwrap().users[0].1,
			m::BanStatus::Unknown
		);
		assert!(state.server_admin.bulk_started.is_none());
		assert!(
			state
				.request_server_admin(
					Id(2),
					Action::Moderation(m::Action::BulkBan {
						users: vec![Id(7)],
						delete_seconds: 0,
						reason: String::new()
					})
				)
				.is_none()
		);
	}
	#[test]
	fn completed_or_reconciled_batches_ignore_late_duplicate_completion() {
		let mut state = state(p::BAN_MEMBERS | p::MANAGE_GUILD);
		let submit = m::Action::BulkBan {
			users: vec![Id(7)],
			delete_seconds: 0,
			reason: String::new(),
		};
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(Id(2), Action::Moderation(submit))
			.unwrap()
		else {
			panic!()
		};
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Bulk(
					m::BulkReport::pending(&[Id(7)]),
				))),
			})
			.unwrap();
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(
				Id(2),
				Action::Moderation(m::Action::BulkRefresh { users: vec![Id(7)] }),
			)
			.unwrap()
		else {
			panic!()
		};
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Bulk(m::BulkReport {
					users: vec![(Id(7), m::BanStatus::NotBanned)],
					accepted: true,
				}))),
			})
			.unwrap();
		assert!(!state.server_admin.bulk_awaiting_completion);
		state.apply_bulk_moderation(
			Id(2),
			m::BulkReport {
				users: vec![(Id(7), m::BanStatus::Banned)],
				accepted: true,
			},
		);
		assert_eq!(
			state.server_admin.bulk_report.as_ref().unwrap().users[0].1,
			m::BanStatus::NotBanned
		);
		state.dismiss_bulk_moderation();
		assert!(state.server_admin.bulk_report.is_none());
		state.apply_bulk_moderation(
			Id(2),
			m::BulkReport {
				users: vec![(Id(7), m::BanStatus::Banned)],
				accepted: true,
			},
		);
		assert!(state.server_admin.bulk_report.is_none());
	}
	#[test]
	fn timed_out_active_batch_can_finish_once_before_it_is_retired() {
		let mut state = state(p::BAN_MEMBERS | p::MANAGE_GUILD);
		state.server_admin.bulk_report = Some(m::BulkReport::pending(&[Id(7)]));
		state.server_admin.bulk_awaiting_completion = true;
		state.server_admin.bulk_started =
			Some(std::time::Instant::now() - std::time::Duration::from_secs(31));
		state.expire_bulk_moderation();
		assert_eq!(
			state.server_admin.bulk_report.as_ref().unwrap().users[0].1,
			m::BanStatus::Unknown
		);
		state.apply_bulk_moderation(
			Id(2),
			m::BulkReport {
				users: vec![(Id(7), m::BanStatus::Banned)],
				accepted: true,
			},
		);
		assert!(!state.server_admin.bulk_awaiting_completion);
		state.apply_bulk_moderation(
			Id(2),
			m::BulkReport {
				users: vec![(Id(7), m::BanStatus::Failed)],
				accepted: true,
			},
		);
		assert_eq!(
			state.server_admin.bulk_report.as_ref().unwrap().users[0].1,
			m::BanStatus::Banned
		);
	}
	#[test]
	fn member_review_is_scoped_and_never_installs_history_or_marks_it_read() {
		let mut state = state(p::KICK_MEMBERS | p::VIEW_CHANNEL | p::READ_MESSAGE_HISTORY);
		state.channels = vec![model::Channel {
			icon: None,
			last_message: None,
			id: Id(6),
			guild: Some(Id(2)),
			parent_id: None,
			position: 0,
			name: "Synthetic channel".into(),
			kind: 0,
			recipients: vec![],
			member_list_id: None,
			message_count: None,
			tags: None,
		}];
		state.permissions.channels.insert(
			Id(6),
			p::Channel {
				id: Id(6),
				guild: Id(2),
				overwrites: Some(vec![]),
			},
		);
		state.selected = Some(Id(6));
		state.request = 42;
		let review = || m::Review {
			guild: Id(2),
			channel: Id(6),
			member: member(7, vec![Id(4)]),
			roles: vec![],
			messages: vec![
				m::ReviewMessage {
					id: Id(50),
					channel: Id(6),
					excerpt: "Visible excerpt".into(),
				},
				m::ReviewMessage {
					id: Id(51),
					channel: Id(999),
					excerpt: "Must not be admitted".into(),
				},
			],
			audit: vec![],
			audit_window: None,
			messages_error: None,
			audit_error: None,
		};
		let action = || {
			Action::Moderation(m::Action::Review {
				user: Id(7),
				channel: Id(6),
				audit: false,
			})
		};
		let Command::ServerAdmin { request, .. } =
			state.request_server_admin(Id(2), action()).unwrap()
		else {
			panic!()
		};
		let progress = |request| crate::server_admin::ReviewProgress {
			guild: Id(2),
			request,
			review: Box::new(review()),
			pending: crate::server_admin::REVIEW_MESSAGES,
		};
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::MemberReviewProgress(progress(request)),
		});
		assert!(state.server_admin.pending);
		assert_eq!(
			state.server_admin.member_review_loading,
			crate::server_admin::REVIEW_MESSAGES
		);
		assert_eq!(
			state
				.server_admin
				.member_review
				.as_ref()
				.unwrap()
				.messages
				.len(),
			1
		);
		state.close_member_review();
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::MemberReviewProgress(progress(request)),
		});
		assert!(state.server_admin.member_review.is_none());
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Review(Box::new(review())))),
			})
			.unwrap();
		assert!(state.server_admin.member_review.is_none());
		let Command::ServerAdmin { request, .. } =
			state.request_server_admin(Id(2), action()).unwrap()
		else {
			panic!()
		};
		state.apply(crate::Envelope {
			generation: state.generation.wrapping_sub(1),
			event: crate::Event::MemberReviewProgress(progress(request)),
		});
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::MemberReviewProgress(progress(request.wrapping_sub(1))),
		});
		assert!(state.server_admin.member_review.is_none());
		let mut completed_messages = progress(request);
		completed_messages.pending = crate::server_admin::REVIEW_ROLES;
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::MemberReviewProgress(completed_messages),
		});
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::Delete {
				channel: Id(6),
				id: Id(50),
			},
		});
		state
			.apply_server_admin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(Outcome::Moderation(m::Outcome::Review(Box::new(review())))),
			})
			.unwrap();
		assert_eq!(
			state
				.server_admin
				.member_review
				.as_ref()
				.unwrap()
				.messages
				.len(),
			0
		);
		assert_eq!(state.selected, Some(Id(6)));
		assert_eq!(state.request, 42);
		assert!(state.timeline.is_empty());
		assert!(state.search.is_none());
		assert!(!state.history_pending);
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::Delete {
				channel: Id(6),
				id: Id(50),
			},
		});
		assert!(
			state
				.server_admin
				.member_review
				.as_ref()
				.unwrap()
				.messages
				.is_empty()
		);
		state.close_member_review();
		assert!(state.server_admin.member_review.is_none());
	}
}
