//! Authoritative refresh admission and display reconciliation; disk hints never grant access.
use crate::{Envelope, Event, State, auth::AuthState, force_sync_read::ReadResult};
use model::{Id, Member, MemberIdentity, Patch, permissions as p};

/// Opaque cache; only checked observations may enter its generation-scoped map.
#[derive(Default)]
pub struct ServerBanners {
	entries: std::collections::BTreeMap<Id, (u64, Option<String>)>,
	bytes: usize,
}
impl ServerBanners {
	pub(crate) fn clear(&mut self) {
		self.entries.clear();
		self.bytes = 0;
	}
	pub(crate) fn remove(&mut self, guild: &Id) {
		if let Some((_, value)) = self.entries.remove(guild) {
			self.bytes = self
				.bytes
				.saturating_sub(128 + value.as_ref().map_or(0, String::capacity));
		}
	}
}

/// Issued by current authenticated permission state; callers retire it on scope changes.
pub struct RosterPermit {
	guild: Id,
}

impl RosterPermit {
	pub fn guild(&self) -> Id {
		self.guild
	}
}

impl State {
	/// A service-observed server banner; explicit removal remains distinct from unknown.
	/// This is separate from a member's own server-profile banner.
	pub fn synced_server_banner(&self, guild: Id) -> Option<Option<&str>> {
		if self.auth != AuthState::Authenticated
			|| !self.gateway_connected
			|| self.guild(guild).is_none()
			|| !self
				.channels
				.iter()
				.any(|channel| channel.guild == Some(guild) && self.can_view(channel.id))
		{
			return None;
		}
		let (generation, hash) = self.force_sync_banners.entries.get(&guild)?;
		(*generation == self.generation).then_some(hash.as_deref())
	}
	pub(crate) fn force_sync_banner_bytes(&self) -> usize {
		self.force_sync_banners.bytes
	}
	fn observe_force_sync_banner(&mut self, guild: Id, banner: Patch<String>) -> bool {
		let value = match banner {
			Patch::Absent => return true,
			Patch::Null => None,
			Patch::Value(hash) if model::valid_avatar_hash(&hash) => Some(hash),
			Patch::Value(_) => return false,
		};
		let previous = self
			.force_sync_banners
			.entries
			.get(&guild)
			.map_or(0, |(_, value)| {
				128 + value.as_ref().map_or(0, String::capacity)
			});
		if self.force_sync_banners.entries.len() >= 4096 && previous == 0
			|| self
				.navigation_bytes()
				.saturating_sub(previous)
				.saturating_add(128 + value.as_ref().map_or(0, String::capacity))
				.saturating_add(self.permissions.bytes())
				> model::account::MAX_BYTES
		{
			return false;
		}
		self.force_sync_banners.bytes = self.force_sync_banners.bytes.saturating_sub(previous)
			+ 128 + value.as_ref().map_or(0, String::capacity);
		self.force_sync_banners
			.entries
			.insert(guild, (self.generation, value));
		self.navigation_index.bytes.set(None);
		true
	}
	/// Caller fences REST observation order before applying this live result.
	/// History pages remain owned by the storage worker and cannot replace the timeline here.
	pub fn apply_force_sync_result(&mut self, result: ReadResult) -> bool {
		if self.auth != AuthState::Authenticated
			|| !self.gateway_connected
			|| self
				.navigation_bytes()
				.saturating_add(self.permissions.bytes())
				.saturating_add(result.bytes())
				> model::account::MAX_BYTES
		{
			return false;
		}
		let generation = self.generation;
		let apply = |state: &mut State, event| state.apply(Envelope { generation, event });
		match result {
			ReadResult::Guild(value) => {
				let guild = value.display.id;
				if self.guild(guild).is_none()
					|| !value.roles.valid()
					|| value.permissions.guilds.len() != 1
					|| value
						.permissions
						.guilds
						.iter()
						.any(|entry| entry.id != guild)
					|| self
						.permissions
						.bytes()
						.saturating_add(value.permissions.bytes())
						.saturating_add(512 * 1024)
						> crate::permissions::MAX_BYTES
				{
					return false;
				}
				if !self.observe_force_sync_banner(guild, value.banner) {
					return false;
				}
				apply(self, Event::GuildChanged(value.display));
				for mut metadata in value.permissions.guilds {
					// A failed optional own-member read is not a permission revocation.
					// Keep the live mirror unchanged as a unit: fresh role bits or ownership
					// must never be combined with older member assignments.
					if matches!(&value.self_member, Err(failure) if *failure != crate::auth::Failure::Forbidden)
					{
						continue;
					}
					if value.self_member.is_err() {
						metadata.member = None;
						if metadata.owner == self.user.as_ref().map(|user| user.id) {
							metadata.owner = None;
						}
					}
					if let Some(old) = self.permissions.guilds.get(&guild) {
						if metadata.member.is_none() && value.self_member.is_ok() {
							metadata.member.clone_from(&old.member);
						}
						if metadata.roles.is_none() {
							metadata.roles.clone_from(&old.roles);
						}
						if metadata.owner.is_none() && value.self_member.is_ok() {
							metadata.owner = old.owner;
						}
					}
					apply(
						self,
						Event::Permissions(crate::permissions::Event::Guild(metadata)),
					);
				}
				if let Ok(member) = value.self_member {
					let unknown = [member.user.id];
					self.apply_force_sync_members_with_unknown_nicks(
						guild,
						&[member],
						if value.self_member_nick_known {
							&[]
						} else {
							&unknown
						},
					);
				}
				if self.server_admin.guild == Some(guild) && self.server_admin.roles.is_some() {
					self.server_admin.roles = Some(value.roles);
				}
			}
			ReadResult::Channels {
				guild,
				channels,
				permissions,
			} => {
				if self.guild(guild).is_none()
					|| channels.iter().any(|channel| {
						channel.id.0 == 0
							|| channel.guild != Some(guild)
							|| matches!(channel.kind, 10..=12)
							|| self.channel(channel.id).is_some_and(|old| {
								old.guild != Some(guild) || matches!(old.kind, 10..=12)
							})
					}) || permissions.iter().any(|channel| channel.guild != guild)
					|| self
						.permissions
						.bytes()
						.saturating_add(permissions.iter().map(p::Channel::bytes).sum::<usize>())
						.saturating_add(512 * 1024)
						> crate::permissions::MAX_BYTES
					|| self
						.channels
						.len()
						.saturating_add(self.guilds.len())
						.saturating_add(channels.len())
						> crate::MAX_NAV
				{
					return false;
				}
				let ids: std::collections::BTreeSet<_> =
					channels.iter().map(|channel| channel.id).collect();
				let metadata_ids: std::collections::BTreeSet<_> =
					permissions.iter().map(|channel| channel.id).collect();
				if ids.len() != channels.len()
					|| metadata_ids.len() != permissions.len()
					|| permissions.iter().any(|channel| {
						!ids.contains(&channel.id)
							|| channel
								.overwrites
								.as_ref()
								.is_some_and(|items| items.len() > p::MAX_OVERWRITES)
					}) {
					return false;
				}
				let removed: Vec<_> = self
					.channels
					.iter()
					.filter(|channel| {
						channel.guild == Some(guild)
							&& !matches!(channel.kind, 10..=12)
							&& !ids.contains(&channel.id)
					})
					.map(|channel| channel.id)
					.collect();
				for channel in removed {
					apply(self, Event::Unavailable(channel));
				}
				for metadata in permissions {
					apply(
						self,
						Event::Permissions(crate::permissions::Event::Channel {
							channel: metadata.id,
							guild: Some(guild),
							overwrites: metadata.overwrites.map_or(Patch::Absent, Patch::Value),
						}),
					);
				}
				for channel in channels {
					apply(self, Event::ChannelCreated(channel));
				}
			}
			ReadResult::Emojis { guild, items } => {
				if self.guild(guild).is_none() || !model::valid_custom_emojis(&items) {
					return false;
				}
				apply(
					self,
					Event::GuildEmojis {
						guild,
						emojis: items,
					},
				);
			}
			ReadResult::Stickers { guild, items } => {
				if self.guild(guild).is_none()
					|| !model::valid_stickers(&items, model::MAX_GUILD_STICKERS)
					|| items.iter().any(|item| item.guild_id != Some(guild))
				{
					return false;
				}
				apply(
					self,
					Event::GuildStickers {
						guild,
						stickers: items,
					},
				);
			}
			ReadResult::Sounds { guild, items } => {
				if !model::soundboard::valid_sounds(&items)
					|| items.iter().any(|item| item.guild != Some(guild))
				{
					return false;
				}
				if let Some((current, catalog)) = &mut self.soundboard.guild
					&& *current == guild
				{
					catalog.sounds = items;
					catalog.loaded = true;
					catalog.loading = false;
					catalog.stale = false;
					catalog.error = None;
					self.revision = self.revision.wrapping_add(1);
				}
			}
			ReadResult::ActiveThreads {
				guild,
				parent,
				threads,
				next,
			} => {
				let new_threads = threads
					.iter()
					.filter(|thread| self.channel(thread.id).is_none())
					.count();
				if self.guild(guild).is_none()
					|| self
						.channel(parent)
						.is_none_or(|channel| channel.guild != Some(guild))
					|| !self.can_archive(parent, model::archives::Kind::Public)
					|| threads.len() > crate::force_sync_read::THREAD_PAGE_SIZE
					|| next.is_some_and(|id| id.0 == 0)
					|| self
						.channels
						.len()
						.saturating_add(self.guilds.len())
						.saturating_add(new_threads)
						> crate::MAX_NAV
					|| threads.iter().enumerate().any(|(index, thread)| {
						thread.id.0 == 0
							|| thread.id == parent
							|| thread.guild != Some(guild)
							|| thread.parent_id != Some(parent)
							|| !self.force_sync_thread_read_allowed(thread)
							|| threads[..index].iter().any(|other| other.id == thread.id)
							|| self.channel(thread.id).is_some_and(|old| {
								old.guild != Some(guild)
									|| old.parent_id != Some(parent)
									|| !matches!(old.kind, 10..=12)
							})
					}) {
					return false;
				}
				// A thread absent from the active endpoint may have been archived, not deleted.
				// Explicit removals remain with Gateway; this read must not reset an open forum page.
				for thread in threads {
					apply(self, Event::ChannelCreated(thread));
				}
			}
			ReadResult::Members {
				guild,
				items,
				missing,
				nick_unknown,
			} => {
				let returned: std::collections::BTreeSet<_> =
					items.iter().map(|member| member.user.id).collect();
				if self.guild(guild).is_none()
					|| !self
						.channels
						.iter()
						.any(|channel| channel.guild == Some(guild) && self.can_view(channel.id))
					|| items.len().saturating_add(missing.len())
						> crate::force_sync_read::MAX_MEMBER_IDS
					|| returned.len() != items.len()
					|| items.iter().any(|member| !member.valid())
					|| missing.iter().any(|id| {
						id.0 == 0
							|| returned.contains(id)
							|| self.user.as_ref().is_some_and(|own| own.id == *id)
					}) || !missing.windows(2).all(|ids| ids[0] < ids[1])
					|| !nick_unknown.windows(2).all(|ids| ids[0] < ids[1])
					|| nick_unknown.iter().any(|id| !returned.contains(id))
				{
					return false;
				}
				self.apply_force_sync_members_with_unknown_nicks(guild, &items, &nick_unknown);
				for user in missing {
					self.retire_force_sync_member(guild, user);
				}
			}
			ReadResult::History(_)
			| ReadResult::Archives(_)
			| ReadResult::ActiveThreadsIndexing { .. } => return false,
		}
		self.auth == AuthState::Authenticated
	}
	/// Authorizes a live service-returned thread descriptor without exposing archived posts
	/// as active sidebar entries. The exact history request still requires service acceptance.
	pub fn force_sync_thread_read_allowed(&self, thread: &model::Channel) -> bool {
		self.auth == AuthState::Authenticated
			&& self.gateway_connected
			&& matches!(thread.kind, 10..=12)
			&& thread
				.guild
				.is_some_and(|guild| self.guild(guild).is_some())
			&& thread.parent_id.is_some_and(|parent| {
				self.channel(parent)
					.is_some_and(|channel| channel.guild == thread.guild)
					&& self.can_read_history(parent)
			})
	}
	pub fn force_sync_roster_permit(&self, guild: Id) -> Option<RosterPermit> {
		if self.auth != AuthState::Authenticated
			|| !self.gateway_connected
			|| self.guild(guild).is_none()
		{
			return None;
		}
		[p::MANAGE_ROLES, p::KICK_MEMBERS, p::BAN_MEMBERS]
			.into_iter()
			.any(|bits| self.guild_permission(guild, bits))
			.then_some(RosterPermit { guild })
	}
	/// Only a complete, current, authorized service roster may prove a member's absence.
	/// The caller fences the account/job and intervening live observations before this step.
	pub fn reconcile_force_sync_roster(&mut self, guild: Id, users: &[Id]) -> bool {
		if self.force_sync_roster_permit(guild).is_none()
			|| users.len() > 250_000
			|| users.iter().any(|user| user.0 == 0)
			|| !users.windows(2).all(|pair| pair[0] < pair[1])
			|| self
				.user
				.as_ref()
				.is_none_or(|own| users.binary_search(&own.id).is_err())
		{
			return false;
		}
		for user in self.force_sync_known_member_ids(guild) {
			if users.binary_search(&user).is_err() {
				self.retire_force_sync_member(guild, user);
			}
		}
		true
	}
	/// Applies only live member observations. Authorization remains with the account permission mirror.
	pub fn apply_force_sync_members(&mut self, guild: Id, members: &[Member]) {
		self.apply_force_sync_members_with_unknown_nicks(guild, members, &[]);
	}
	/// Applies current roles while leaving omitted nickname fields unchanged.
	pub fn apply_force_sync_members_with_unknown_nicks(
		&mut self,
		guild: Id,
		members: &[Member],
		nick_unknown: &[Id],
	) {
		if self.auth != AuthState::Authenticated
			|| !self.gateway_connected
			|| self.guild(guild).is_none()
			|| !self
				.channels
				.iter()
				.any(|channel| channel.guild == Some(guild) && self.can_view(channel.id))
			|| members.len() > 1000
			|| members.iter().any(|member| !member.valid())
			|| !nick_unknown.windows(2).all(|ids| ids[0] < ids[1])
			|| nick_unknown
				.iter()
				.any(|id| !members.iter().any(|member| member.user.id == *id))
			|| members.iter().map(Member::bytes).sum::<usize>() > 4 * 1024 * 1024
		{
			return;
		}
		for member in members.iter().filter(|member| member.valid()) {
			let mut roles = member.roles.clone();
			roles.sort_unstable();
			roles.dedup();
			self.apply_force_sync_identity(MemberIdentity {
				global_name: model::GlobalNameUpdate::Set,
				avatar: model::Patch::Absent,
				guild,
				user: member.user.id,
				profile: Some(member.user.clone()),
				nick: if nick_unknown.binary_search(&member.user.id).is_ok() {
					Patch::Absent
				} else {
					member.nick.clone().map_or(Patch::Null, Patch::Value)
				},
				roles: Patch::Value(roles),
				departed: false,
			});
		}
		if nick_unknown.is_empty() {
			self.member_labels.remember(guild, members);
		} else {
			let complete: Vec<_> = members
				.iter()
				.filter(|member| nick_unknown.binary_search(&member.user.id).is_err())
				.cloned()
				.collect();
			self.member_labels.remember(guild, &complete);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::{Channel, Guild, User};
	fn user(id: u64) -> User {
		User {
			member_avatar: None,
			id: Id(id),
			name: "Synthetic".into(),
			username: Some("synthetic".into()),
			avatar: None,
			discriminator: 0,
			primary_guild: None,
			cosmetics: None,
			webhook: false,
			kind: Default::default(),
		}
	}
	fn channel(id: u64, parent: Option<Id>, kind: u8) -> Channel {
		Channel {
			id: Id(id),
			guild: Some(Id(10)),
			parent_id: parent,
			position: 0,
			name: "Synthetic channel".into(),
			kind,
			recipients: vec![],
			member_list_id: None,
			message_count: None,
			tags: None,
			icon: None,
			last_message: None,
		}
	}
	fn fixture(bits: u128) -> State {
		let mut state = State::default();
		let role = model::server_roles::Role {
			id: Id(10),
			name: "everyone".into(),
			permissions: bits | p::VIEW_CHANNEL | p::READ_MESSAGE_HISTORY,
			..Default::default()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Ready {
				user: user(2),
				guilds: vec![Guild {
					id: Id(10),
					name: "Synthetic server".into(),
					icon: None,
					emojis: None,
					stickers: None,
					default_message_notifications: None,
				}],
				channels: vec![channel(20, None, 0)],
				permissions: p::Snapshot {
					guilds: vec![p::Guild {
						id: Id(10),
						owner: Some(Id(1)),
						roles: Some(vec![role.permission_role()]),
						member: Some(p::Member {
							roles: vec![],
							timeout_until: None,
						}),
					}],
					channels: vec![p::Channel {
						id: Id(20),
						guild: Id(10),
						overwrites: Some(vec![]),
					}],
				},
			},
		});
		state.select(Id(20));
		state.drafts.insert(Id(20), "Retained draft".into());
		state
	}
	#[test]
	fn roster_permits_require_current_authenticated_moderator_permissions() {
		let mut state = fixture(0);
		assert!(state.force_sync_roster_permit(Id(10)).is_none());
		for bits in [
			p::MANAGE_ROLES,
			p::KICK_MEMBERS,
			p::BAN_MEMBERS,
			p::ADMINISTRATOR,
		] {
			state = fixture(bits);
			assert_eq!(
				state.force_sync_roster_permit(Id(10)).unwrap().guild(),
				Id(10)
			);
			state.gateway_connected = false;
			assert!(state.force_sync_roster_permit(Id(10)).is_none());
			state.gateway_connected = true;
			state.auth = AuthState::Unauthenticated;
			assert!(state.force_sync_roster_permit(Id(10)).is_none());
		}
	}
	#[test]
	fn member_refresh_reconciles_nickname_and_roles_without_changing_navigation() {
		let mut state = fixture(0);
		let member = Member {
			user: user(7),
			nick: Some("Current nickname".into()),
			roles: vec![],
			status: None,
			custom_status: None,
			activities: vec![],
			clients: Default::default(),
		};
		let selected = state.selected;
		let history = state.request;
		state.apply_force_sync_members(Id(10), std::slice::from_ref(&member));
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Current nickname"))
		);
		assert_eq!(state.selected, selected);
		assert_eq!(state.request, history);
		assert_eq!(
			state.drafts.get(&Id(20)).map(String::as_str),
			Some("Retained draft")
		);
		state.apply_force_sync_members(
			Id(10),
			&[Member {
				nick: None,
				..member
			}],
		);
		assert_eq!(state.member_nickname(Id(20), Id(7)), Some(None));
	}
	#[test]
	fn archive_descriptors_authorize_reads_without_becoming_active_sidebar_threads() {
		let mut state = fixture(0);
		let archived = channel(30, Some(Id(20)), 11);
		assert!(state.force_sync_thread_read_allowed(&archived));
		assert!(state.channel(archived.id).is_none());
		assert!(
			!state.apply_force_sync_result(ReadResult::Archives(model::archives::Page {
				threads: vec![archived.clone()],
				next: None
			}))
		);
		assert!(state.channel(archived.id).is_none());
		assert!(state.apply_force_sync_result(ReadResult::ActiveThreads {
			guild: Id(10),
			parent: Id(20),
			threads: vec![archived],
			next: None,
		}));
		assert!(state.can_read_history(Id(30)));
		assert_eq!(state.selected, Some(Id(20)));
		assert!(!state.force_sync_thread_read_allowed(&channel(31, Some(Id(999)), 11)));
	}
	#[test]
	fn active_thread_results_preserve_navigation_and_require_current_parent_access() {
		let mut state = fixture(0);
		let make_page = |thread| ReadResult::ActiveThreads {
			guild: Id(10),
			parent: Id(20),
			threads: vec![thread],
			next: None,
		};
		assert!(!state.apply_force_sync_result(make_page(channel(31, Some(Id(999)), 11))));
		assert!(state.apply_force_sync_result(make_page(channel(32, Some(Id(20)), 12))));
		assert_eq!(state.selected, Some(Id(20)));
		assert_eq!(
			state.drafts.get(&Id(20)).map(String::as_str),
			Some("Retained draft")
		);
		state.gateway_connected = false;
		assert!(!state.apply_force_sync_result(make_page(channel(33, Some(Id(20)), 11))));
		assert!(state.channel(Id(33)).is_none());
	}
	#[test]
	fn ordinary_member_refresh_updates_sidebar_composer_and_typing_names() {
		let mut state = fixture(0);
		state.freshness = model::Freshness::Fresh;
		state.history_pending = false;
		let old = Member {
			user: user(7),
			nick: Some("Old nickname".into()),
			roles: vec![],
			status: Some("online".into()),
			custom_status: None,
			activities: vec![],
			clients: Default::default(),
		};
		state.request_members().unwrap();
		let mut list = state.members.clone().unwrap();
		list.slots = vec![Some(model::MemberSlot::Person(old.clone()))];
		list.total = 1;
		list.freshness = model::Freshness::Fresh;
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Members(list),
		});
		let Some(crate::Command::MemberSearch(request)) = state.search_members(Id(20), "Old", 0)
		else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::MemberSearch {
				request,
				result: Ok(vec![old.clone()]),
			},
		});
		let now = std::time::Instant::now();
		let wall = std::time::UNIX_EPOCH + std::time::Duration::from_secs(100);
		state.observe_typing_at(
			crate::typing::Signal {
				channel: Id(20),
				user: Id(7),
				timestamp: 100,
				member: None,
			},
			wall,
			now,
		);
		let next = Member {
			nick: Some("Current nickname".into()),
			roles: vec![Id(11)],
			..old
		};
		state.apply_force_sync_members(Id(10), &[next]);
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Current nickname"))
		);
		assert_eq!(
			state.member_search[0].rows[0].nick.as_deref(),
			Some("Current nickname")
		);
		assert_eq!(state.member_search[0].rows[0].roles, vec![Id(11)]);
		let Some(model::MemberSlot::Person(sidebar)) = state.member_slot(0) else {
			panic!()
		};
		assert_eq!(sidebar.nick.as_deref(), Some("Current nickname"));
		assert_eq!(sidebar.roles, vec![Id(11)]);
		assert_eq!(sidebar.status.as_deref(), Some("online"));
		assert_eq!(state.typing_users(now).collect::<Vec<_>>(), vec![Id(7)]);
		assert!(state.force_sync_roster_permit(Id(10)).is_none());
	}
	#[test]
	fn guild_refresh_keeps_other_servers_and_only_complete_catalogs_remove_items() {
		let mut state = fixture(0);
		let mut other = state.guilds[0].clone();
		other.id = Id(11);
		other.name = "Other server".into();
		state.guilds.push(other);
		let mut other_permissions = state.permissions.guilds[&Id(10)].clone();
		other_permissions.id = Id(11);
		other_permissions.roles.as_mut().unwrap()[0].id = Id(11);
		state
			.update_permissions(crate::permissions::Event::Guild(other_permissions.clone()))
			.unwrap();
		let emoji = model::CustomEmoji {
			id: Id(50),
			name: "party".into(),
			animated: false,
			available: true,
			managed: false,
			roles: Some(vec![]),
		};
		assert!(state.apply_force_sync_result(ReadResult::Emojis {
			guild: Id(10),
			items: vec![emoji]
		}));
		let role = model::server_roles::Role {
			id: Id(10),
			name: "everyone".into(),
			permissions: p::VIEW_CHANNEL | p::READ_MESSAGE_HISTORY,
			colors: model::server_roles::Colors {
				primary: 0x123456,
				secondary: Some(0x654321),
				tertiary: None,
			},
			..Default::default()
		};
		let roles = model::server_roles::Catalog {
			guild: Id(10),
			items: vec![role.clone()],
			features: vec![],
		};
		let metadata = p::Guild {
			id: Id(10),
			owner: None,
			roles: Some(vec![role.permission_role()]),
			member: None,
		};
		assert!(state.apply_force_sync_result(ReadResult::Guild(Box::new(
			crate::force_sync_read::GuildRead {
				display: model::GuildPatch {
					id: Id(10),
					name: Patch::Value("Renamed".into()),
					icon: Patch::Absent,
					default_message_notifications: Patch::Absent
				},
				permissions: p::Snapshot {
					guilds: vec![metadata],
					channels: vec![]
				},
				roles,
				self_member: Err(crate::auth::Failure::Forbidden),
				self_member_nick_known: false,
				banner: Patch::Value("0123456789abcdef0123456789abcdef".into()),
				member_count: None
			}
		))));
		assert_eq!(
			state.guild_roles(Id(10)).unwrap()[0].secondary_color,
			Some(0x654321)
		);
		assert_eq!(state.permissions.guilds[&Id(11)], other_permissions);
		assert_eq!(
			state.synced_server_banner(Id(10)),
			None,
			"fresh banner remains unavailable while own membership is unknown"
		);
		assert!(
			state.permissions.guilds[&Id(10)].member.is_none(),
			"unknown own membership cannot reuse old assignments with fresh role bits"
		);
		assert_eq!(
			state.guild(Id(10)).unwrap().emojis.as_ref().unwrap().len(),
			1,
			"unrelated guild fields preserve catalogs"
		);
		assert!(state.apply_force_sync_result(ReadResult::Emojis {
			guild: Id(10),
			items: vec![]
		}));
		assert!(
			state
				.guild(Id(10))
				.unwrap()
				.emojis
				.as_ref()
				.unwrap()
				.is_empty()
		);
		assert!(!state.apply_force_sync_result(ReadResult::Emojis {
			guild: Id(10),
			items: vec![model::CustomEmoji {
				id: Id(0),
				name: "invalid".into(),
				animated: false,
				available: true,
				managed: false,
				roles: None
			}]
		}));
		assert_eq!(
			state.auth,
			AuthState::Authenticated,
			"invalid optional data stays a scoped rejection"
		);
	}
	fn guild_with_incomplete_own_member(failure: crate::auth::Failure) -> ReadResult {
		let role = model::server_roles::Role {
			id: Id(10),
			name: "Fresh role".into(),
			permissions: p::ADMINISTRATOR,
			colors: model::server_roles::Colors {
				primary: 0x123456,
				..Default::default()
			},
			..Default::default()
		};
		ReadResult::Guild(Box::new(crate::force_sync_read::GuildRead {
			display: model::GuildPatch {
				id: Id(10),
				name: Patch::Value("Fresh server name".into()),
				icon: Patch::Absent,
				default_message_notifications: Patch::Absent,
			},
			permissions: p::Snapshot {
				guilds: vec![p::Guild {
					id: Id(10),
					owner: Some(Id(1)),
					roles: Some(vec![role.permission_role()]),
					member: None,
				}],
				channels: vec![],
			},
			roles: model::server_roles::Catalog {
				guild: Id(10),
				items: vec![role],
				features: vec![],
			},
			self_member: Err(failure),
			self_member_nick_known: false,
			banner: Patch::Absent,
			member_count: None,
		}))
	}
	#[test]
	fn own_member_refresh_with_omitted_nickname_keeps_current_name() {
		let mut state = fixture(0);
		let original = cached_person(2, "Own nickname");
		state.apply_force_sync_members(Id(10), std::slice::from_ref(&original));
		let mut response = guild_with_incomplete_own_member(crate::auth::Failure::Network);
		let ReadResult::Guild(value) = &mut response else {
			panic!("guild response")
		};
		value.self_member = Ok(Member {
			nick: None,
			..original
		});
		value.self_member_nick_known = false;
		assert!(state.apply_force_sync_result(response));
		assert_eq!(
			state.member_nickname(Id(20), Id(2)),
			Some(Some("Own nickname"))
		);
	}
	#[test]
	fn transient_own_member_failures_preserve_live_permission_mirror_and_call_as_a_unit() {
		for failure in [
			crate::auth::Failure::Network,
			crate::auth::Failure::Busy,
			crate::auth::Failure::RateLimited,
			crate::auth::Failure::Protocol,
			crate::auth::Failure::ResponseLimit,
		] {
			let mut state = fixture(p::CONNECT | p::SPEAK);
			state.apply(Envelope {
				generation: state.generation,
				event: Event::ChannelCreated(channel(21, None, 2)),
			});
			state.apply(Envelope {
				generation: state.generation,
				event: Event::Permissions(crate::permissions::Event::Channel {
					channel: Id(21),
					guild: Some(Id(10)),
					overwrites: Patch::Value(vec![]),
				}),
			});
			assert!(state.start_call(Id(21), false).is_some());
			let call_request = state.voice.active.as_ref().unwrap().request;
			let mirror = state.permissions.guilds[&Id(10)].clone();
			assert!(state.can_read_history(Id(20)));
			assert!(state.apply_force_sync_result(guild_with_incomplete_own_member(failure)));
			assert!(
				state.can_read_history(Id(20)),
				"an incomplete refresh must not revoke current conversation access: {failure}"
			);
			assert_eq!(
				state.permissions.guilds[&Id(10)],
				mirror,
				"fresh role bits/owner must not combine with old member assignments"
			);
			assert_eq!(state.permission(Id(20), p::ADMINISTRATOR), Some(false));
			assert_eq!(
				state.voice.active.as_ref().map(|call| call.request),
				Some(call_request)
			);
			assert_eq!(state.selected, Some(Id(20)));
			assert_eq!(state.guild(Id(10)).unwrap().name, "Fresh server name");
		}
	}
	#[test]
	fn incomplete_own_member_refresh_cannot_turn_unknown_permissions_into_access() {
		let mut state = fixture(0);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::Guild(p::Guild {
				id: Id(10),
				owner: Some(Id(1)),
				roles: None,
				member: None,
			})),
		});
		let mirror = state.permissions.guilds[&Id(10)].clone();
		assert!(!state.can_read_history(Id(20)));
		assert!(
			state.apply_force_sync_result(guild_with_incomplete_own_member(
				crate::auth::Failure::Network
			))
		);
		assert_eq!(state.permissions.guilds[&Id(10)], mirror);
		assert!(!state.can_read_history(Id(20)));
	}
	#[test]
	fn forbidden_own_member_refresh_fails_closed_even_when_fresh_owner_matches_self() {
		let mut state = fixture(0);
		assert!(state.can_read_history(Id(20)));
		let ReadResult::Guild(mut value) =
			guild_with_incomplete_own_member(crate::auth::Failure::Forbidden)
		else {
			panic!()
		};
		value.permissions.guilds[0].owner = Some(Id(2));
		assert!(state.apply_force_sync_result(ReadResult::Guild(value)));
		assert!(!state.can_read_history(Id(20)));
		assert!(state.force_sync_roster_permit(Id(10)).is_none());
		assert_eq!(state.auth, AuthState::Authenticated);
	}

	fn cached_person(id: u64, nick: &str) -> Member {
		Member {
			user: user(id),
			nick: Some(nick.into()),
			roles: vec![Id(11)],
			status: None,
			custom_status: None,
			activities: vec![],
			clients: Default::default(),
		}
	}
	#[test]
	fn confirmed_missing_member_clears_nickname_roles_and_suggestions_without_revoking_self() {
		let mut state = fixture(0);
		state.freshness = model::Freshness::Fresh;
		state.history_pending = false;
		let member = cached_person(7, "Departed nickname");
		state.apply_force_sync_members(Id(10), std::slice::from_ref(&member));
		state.request_members().unwrap();
		let mut list = state.members.clone().unwrap();
		list.slots = vec![Some(model::MemberSlot::Person(member.clone()))];
		list.total = 1;
		list.freshness = model::Freshness::Fresh;
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Members(list),
		});
		let Some(crate::Command::MemberSearch(request)) =
			state.search_members(Id(20), "Departed", 0)
		else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::MemberSearch {
				request,
				result: Ok(vec![member]),
			},
		});
		let now = std::time::Instant::now();
		state.observe_typing_at(
			crate::typing::Signal {
				channel: Id(20),
				user: Id(7),
				timestamp: 100,
				member: None,
			},
			std::time::UNIX_EPOCH + std::time::Duration::from_secs(100),
			now,
		);
		let mirror = state.permissions.guilds[&Id(10)].clone();
		assert!(state.apply_force_sync_result(ReadResult::Members {
			guild: Id(10),
			items: vec![],
			missing: vec![Id(7)],
			nick_unknown: vec![]
		}));
		assert_eq!(state.member_nickname(Id(20), Id(7)), Some(None));
		let identity = state.member_identity(Id(20), Id(7)).unwrap();
		assert!(identity.departed);
		assert!(matches!(&identity.roles,Patch::Value(roles) if roles.is_empty()));
		assert!(state.member_search[0].rows.is_empty());
		assert!(state.member_slot(0).is_none());
		assert!(state.display_member(Id(20), Id(7)).is_none());
		assert!(state.typing_users(now).next().is_none());
		assert_eq!(state.permissions.guilds[&Id(10)], mirror);
		assert!(state.can_read_history(Id(20)));
		assert_eq!(state.selected, Some(Id(20)));
	}
	#[test]
	fn incomplete_member_results_never_prove_departures_and_invalid_missing_ids_are_atomic() {
		let mut state = fixture(0);
		let member = cached_person(7, "Still a member");
		state.apply_force_sync_members(Id(10), std::slice::from_ref(&member));
		assert!(state.apply_force_sync_result(ReadResult::Members {
			guild: Id(10),
			items: vec![],
			missing: vec![],
			nick_unknown: vec![]
		}));
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Still a member"))
		);
		for missing in [
			vec![Id(7), Id(7)],
			vec![Id(0)],
			vec![Id(2)],
			vec![Id(8), Id(7)],
		] {
			assert!(!state.apply_force_sync_result(ReadResult::Members {
				guild: Id(10),
				items: vec![],
				missing,
				nick_unknown: vec![]
			}));
			assert_eq!(
				state.member_nickname(Id(20), Id(7)),
				Some(Some("Still a member"))
			);
		}
		assert!(!state.apply_force_sync_result(ReadResult::Members {
			guild: Id(10),
			items: vec![Member {
				nick: Some("Conflicting update".into()),
				..member
			}],
			missing: vec![Id(7)],
			nick_unknown: vec![]
		}));
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Still a member"))
		);
	}
	#[test]
	fn member_read_with_omitted_nickname_keeps_it_while_refreshing_roles() {
		let mut state = fixture(0);
		let original = cached_person(7, "Existing nickname");
		state.apply_force_sync_members(Id(10), std::slice::from_ref(&original));
		let mut updated = original.clone();
		updated.nick = None;
		updated.roles = vec![Id(42)];
		assert!(state.apply_force_sync_result(ReadResult::Members {
			guild: Id(10),
			items: vec![updated.clone()],
			missing: vec![],
			nick_unknown: vec![Id(7)],
		}));
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Existing nickname"))
		);
		assert!(
			matches!(&state.member_identity(Id(20), Id(7)).unwrap().roles, Patch::Value(roles) if roles == &[Id(42)])
		);
		assert!(state.apply_force_sync_result(ReadResult::Members {
			guild: Id(10),
			items: vec![updated],
			missing: vec![],
			nick_unknown: vec![],
		}));
		assert_eq!(state.member_nickname(Id(20), Id(7)), Some(None));
	}
	#[test]
	fn complete_roster_removes_absent_identities_and_requires_own_presence_and_current_permission()
	{
		let mut state = fixture(p::KICK_MEMBERS);
		state.apply_force_sync_members(
			Id(10),
			&[cached_person(7, "Departing"), cached_person(8, "Remaining")],
		);
		let mirror = state.permissions.guilds[&Id(10)].clone();
		assert!(!state.reconcile_force_sync_roster(Id(10), &[Id(7), Id(8)]));
		assert!(!state.reconcile_force_sync_roster(Id(10), &[Id(2), Id(8), Id(8)]));
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Departing"))
		);
		assert!(state.reconcile_force_sync_roster(Id(10), &[Id(2), Id(8)]));
		assert!(state.display_member(Id(20), Id(7)).is_none());
		assert_eq!(state.member_nickname(Id(20), Id(7)), Some(None));
		assert_eq!(
			state.member_nickname(Id(20), Id(8)),
			Some(Some("Remaining"))
		);
		assert_eq!(state.permissions.guilds[&Id(10)], mirror);
		assert_eq!(state.selected, Some(Id(20)));
		let mut ordinary = fixture(0);
		ordinary.apply_force_sync_members(Id(10), &[cached_person(7, "Ordinary member")]);
		assert!(!ordinary.reconcile_force_sync_roster(Id(10), &[Id(2)]));
		assert_eq!(
			ordinary.member_nickname(Id(20), Id(7)),
			Some(Some("Ordinary member"))
		);
	}
	#[test]
	fn off_server_roster_departure_preserves_current_server_identity_and_selection() {
		let mut state = fixture(p::KICK_MEMBERS);
		let mut other = state.guilds[0].clone();
		other.id = Id(11);
		state.guilds.push(other);
		let mut permissions = state.permissions.guilds[&Id(10)].clone();
		permissions.id = Id(11);
		permissions.roles.as_mut().unwrap()[0].id = Id(11);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::Guild(permissions)),
		});
		let mut other_channel = channel(30, None, 0);
		other_channel.guild = Some(Id(11));
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::Channel {
				channel: Id(30),
				guild: Some(Id(11)),
				overwrites: Patch::Value(vec![]),
			}),
		});
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelCreated(other_channel),
		});
		state.apply_force_sync_members(Id(10), &[cached_person(7, "Current server nickname")]);
		state.apply_force_sync_members(Id(11), &[cached_person(7, "Other server nickname")]);
		state
			.apply_force_sync_members(Id(11), &[cached_person(7, "Updated other server nickname")]);
		assert_eq!(
			state.member_nickname(Id(30), Id(7)),
			Some(Some("Updated other server nickname"))
		);
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Current server nickname"))
		);
		assert!(state.display_member(Id(30), Id(7)).is_some());
		let mirror = state.permissions.guilds[&Id(10)].clone();
		assert!(state.reconcile_force_sync_roster(Id(11), &[Id(2)]));
		assert!(state.display_member(Id(30), Id(7)).is_none());
		assert_eq!(
			state.member_nickname(Id(20), Id(7)),
			Some(Some("Current server nickname"))
		);
		assert_eq!(state.selected, Some(Id(20)));
		assert_eq!(state.permissions.guilds[&Id(10)], mirror);
	}

	#[test]
	fn server_banner_clears_and_current_access_do_not_touch_member_profile_banners() {
		let mut state = fixture(0);
		assert!(state.observe_force_sync_banner(
			Id(10),
			Patch::Value("0123456789abcdef0123456789abcdef".into())
		));
		assert!(state.observe_force_sync_banner(Id(10), Patch::Absent));
		assert_eq!(
			state.synced_server_banner(Id(10)),
			Some(Some("0123456789abcdef0123456789abcdef"))
		);
		assert!(state.observe_force_sync_banner(Id(10), Patch::Null));
		assert_eq!(state.synced_server_banner(Id(10)), Some(None));
		assert_eq!(state.force_sync_banner_bytes(), 128);
		state.generation = state.generation.wrapping_add(1);
		assert_eq!(state.synced_server_banner(Id(10)), None);
		state.logout();
		assert_eq!(state.force_sync_banner_bytes(), 0);
	}
}
