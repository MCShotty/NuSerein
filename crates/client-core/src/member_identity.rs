//! Small live identity overlay shared by the sidebar, composer, messages and typing.
//! No permissions or complete directory are derived from these display observations.
use crate::{State, member_search};
use model::{Id, Member, MemberDisplay, MemberIdentity, MemberSlot, Patch, User};
use std::{
	collections::VecDeque,
	time::{Duration, Instant},
};

const LIMIT: usize = 128;
const MAX_BYTES: usize = 128 * 1024;
const MEMBER_LIMIT: usize = 512;
const MEMBER_BYTES: usize = 384 * 1024;

#[derive(Default)]
pub struct Labels {
	entries: VecDeque<(u64, MemberIdentity)>,
	// The last flag distinguishes complete query/sync observations from sidebar rows,
	// which may omit nickname and role fields on their first appearance.
	members: VecDeque<(Id, Member, Instant, bool)>,
	member_bytes: usize,
	visible_scope: Option<(Id, Id)>,
	visible_users: Vec<Id>,
	watch_revision: u64,
	revision: u64,
	display_revision: u64,
	bytes: usize,
}

impl Labels {
	pub(crate) fn clear(&mut self) {
		let display_revision = self.display_revision.wrapping_add(1);
		*self = Self::default();
		self.display_revision = display_revision;
	}
	pub(crate) fn revision(&self) -> u64 {
		self.revision
	}
	pub fn bytes(&self) -> usize {
		self.bytes
			+ self.entries.capacity() * size_of::<(u64, MemberIdentity)>()
			+ self.member_bytes
			+ self.members.capacity() * size_of::<(Id, Member, Instant, bool)>()
			+ self.visible_users.capacity() * size_of::<Id>()
	}
	pub(crate) fn watch_visible(&mut self, guild: Id, channel: Id, users: &[Id]) {
		if self.visible_scope == Some((guild, channel))
			&& self.visible_users.iter().copied().eq(users
				.iter()
				.copied()
				.filter(|id| id.0 != 0)
				.take(member_search::LIMIT))
		{
			return;
		}
		self.visible_scope = Some((guild, channel));
		self.visible_users.clear();
		self.visible_users.extend(
			users
				.iter()
				.copied()
				.filter(|id| id.0 != 0)
				.take(member_search::LIMIT),
		);
		self.watch_revision = self.watch_revision.wrapping_add(1);
	}
	fn overlay_bytes(&self) -> usize {
		self.bytes + self.entries.capacity() * size_of::<(u64, MemberIdentity)>()
	}
	pub(crate) fn member(&self, guild: Id, user: Id) -> Option<&Member> {
		self.members
			.iter()
			.rev()
			.find(|(g, m, _, _)| *g == guild && m.user.id == user)
			.map(|(_, m, _, _)| m)
	}
	pub(crate) fn verified_member(&self, guild: Id, user: Id) -> Option<&Member> {
		self.members
			.iter()
			.rev()
			.find_map(|(g, member, _, verified)| {
				(*g == guild && member.user.id == user && *verified).then_some(member)
			})
	}
	pub(crate) fn member_fresh(&self, guild: Id, user: Id) -> bool {
		self.members.iter().any(|(g, m, at, verified)| {
			*g == guild
				&& m.user.id == user
				&& *verified && at.elapsed() < Duration::from_secs(5 * 60)
		})
	}
	pub(crate) fn remember(&mut self, guild: Id, rows: &[Member]) {
		self.remember_with_source(guild, rows, true);
	}
	pub(crate) fn remember_with_source(&mut self, guild: Id, rows: &[Member], verified: bool) {
		let mut changed = false;
		let mut display_changed = false;
		for member in rows {
			let mut prior_verified = None;
			if let Some(index) = self
				.members
				.iter()
				.position(|(g, m, _, _)| *g == guild && m.user.id == member.user.id)
			{
				let (_, old, observed, was_verified) = &mut self.members[index];
				display_changed |= old.user.name != member.user.name
					|| old.user.username != member.user.username
					|| old.nick != member.nick;
				if old.user == member.user && old.roles == member.roles && old.nick == member.nick {
					if verified {
						*observed = Instant::now();
						*was_verified = true;
					}
					continue;
				}
				let (_, old, observed, was_verified) = self.members.remove(index).unwrap();
				if !verified && was_verified {
					prior_verified = Some(observed);
				}
				self.member_bytes -= old.bytes();
			} else {
				display_changed = true;
			}
			// Presence remains owned by the current subscription, with no transient allocations copied.
			let member = Member {
				user: member.user.clone(),
				roles: member.roles.clone(),
				nick: member.nick.clone(),
				status: None,
				custom_status: None,
				activities: vec![],
				clients: Default::default(),
			};
			self.member_bytes += member.bytes();
			self.members.push_back((
				guild,
				member,
				prior_verified.unwrap_or_else(Instant::now),
				verified || prior_verified.is_some(),
			));
			changed = true;
		}
		if changed {
			self.revision = self.revision.wrapping_add(1);
		}
		if display_changed {
			self.display_revision = self.display_revision.wrapping_add(1);
		}
		if verified {
			for (revision, entry) in &mut self.entries {
				if *revision == 0
					&& entry.guild == guild
					&& rows.iter().any(|row| row.user.id == entry.user)
				{
					*revision = self.revision;
				}
			}
		}
		self.trim_members();
	}

	fn trim_members(&mut self) {
		while self.members.len() > MEMBER_LIMIT
			|| self.member_bytes
				+ self.members.capacity() * size_of::<(Id, Member, Instant, bool)>()
				> MEMBER_BYTES
		{
			let Some((_, old, _, _)) = self.members.pop_front() else {
				break;
			};
			self.member_bytes -= old.bytes();
		}
	}
	fn get(&self, guild: Id, user: Id) -> Option<&MemberIdentity> {
		self.entries
			.iter()
			.rev()
			.find(|(_, row)| row.guild == guild && row.user == user)
			.map(|(_, row)| row)
	}
	/// Disk hints predate every live observation, including queries already in flight.
	fn insert_hint(&mut self, update: MemberIdentity) {
		self.bytes += update.bytes();
		self.entries.push_back((0, update));
	}
	fn insert(&mut self, mut update: MemberIdentity, display_changed: bool) {
		if let Some(index) = self
			.entries
			.iter()
			.position(|(_, old)| old.guild == update.guild && old.user == update.user)
		{
			let (revision, old) = self.entries.remove(index).unwrap();
			self.bytes -= old.bytes();
			// A partial live update must not promote missing fields from an older disk hint.
			if revision != 0 && !update.departed && !old.departed {
				if update.profile.is_none() {
					update.profile = old.profile;
				}
				if matches!(update.nick, Patch::Absent) {
					update.nick = old.nick;
				}
				if matches!(update.roles, Patch::Absent) {
					update.roles = old.roles;
				}
				if matches!(update.avatar, Patch::Absent) {
					update.avatar = old.avatar;
				}
			}
		}
		if let Some(index) = self
			.members
			.iter()
			.position(|(g, m, _, _)| *g == update.guild && m.user.id == update.user)
		{
			let (guild, mut member, observed, verified) = self.members.remove(index).unwrap();
			self.member_bytes -= member.bytes();
			if !update.departed {
				update.apply(&mut member);
				self.member_bytes += member.bytes();
				self.members.push_back((guild, member, observed, verified));
			}
		}
		self.trim_members();
		self.revision = self.revision.wrapping_add(1);
		if display_changed {
			self.display_revision = self.display_revision.wrapping_add(1);
		}
		if self.entries.len() == LIMIT
			&& let Some((_, old)) = self.entries.pop_front()
		{
			self.bytes -= old.bytes();
		}
		self.bytes += update.bytes();
		self.entries.push_back((self.revision, update));
		while self.entries.len() > LIMIT || self.overlay_bytes() > MAX_BYTES {
			if let Some((_, old)) = self.entries.pop_front() {
				self.bytes -= old.bytes();
			} else {
				break;
			}
		}
	}
	/// A query started before a live update cannot restore its old nickname/roles.
	pub(crate) fn reconcile(&mut self, guild: Id, rows: &mut Vec<Member>, started: u64) {
		self.reconcile_with_missing(guild, rows, started, &[]);
	}
	pub(crate) fn reconcile_with_missing(
		&mut self,
		guild: Id,
		rows: &mut Vec<Member>,
		started: u64,
		missing: &[model::MissingMemberFields],
	) {
		if self.entries.is_empty() && missing.is_empty() {
			return;
		}
		let mut display_changed = false;
		rows.retain_mut(|member| {
			let fields = missing.iter().find(|fields| fields.user == member.user.id);
			let fallback = fields
				.and_then(|_| self.verified_member(guild, member.user.id))
				.map(|known| (known.nick.clone(), known.roles.clone()));
			if let Some(fields) = fields
				&& let Some((nick, roles)) = fallback
			{
				if fields.nick {
					member.nick = nick;
				}
				if fields.roles {
					member.roles = roles;
				}
			}
			if let Some((revision, update)) = self
				.entries
				.iter_mut()
				.find(|(_, update)| update.guild == guild && update.user == member.user.id)
			{
				if *revision > started {
					if update.departed {
						return false;
					}
					update.apply(member);
				} else {
					if let Some(fields) = fields {
						if fields.nick {
							match &update.nick {
								Patch::Value(nick) => {
									member.nick = (!nick.is_empty()).then(|| nick.clone())
								}
								Patch::Null => member.nick = None,
								Patch::Absent => {}
							}
						}
						if fields.roles
							&& let Patch::Value(roles) = &update.roles
						{
							member.roles.clone_from(roles);
						}
					}
					let nick = match &update.nick {
						Patch::Value(nick) if !nick.is_empty() => Some(nick.as_str()),
						_ => None,
					};
					display_changed |= update.departed
						|| update.profile.as_ref().is_none_or(|profile| {
							profile.name != member.user.name
								|| profile.username != member.user.username
						}) || (fields.is_none_or(|fields| !fields.nick)
						&& (matches!(update.nick, Patch::Absent)
							|| nick != member.nick.as_deref().filter(|nick| !nick.is_empty())));
					self.bytes -= update.bytes();
					update.profile = Some(member.user.clone());
					if fields.is_none_or(|fields| !fields.nick) {
						update.nick = member.nick.clone().map_or(Patch::Null, Patch::Value);
					}
					if fields.is_none_or(|fields| !fields.roles) {
						update.roles = Patch::Value(member.roles.clone());
					}
					update.avatar = member
						.user
						.member_avatar
						.as_ref()
						.filter(|avatar| avatar.guild == guild)
						.map_or(Patch::Absent, |avatar| {
							avatar.hash.clone().map_or(Patch::Null, Patch::Value)
						});
					update.departed = false;
					self.bytes += update.bytes();
				}
			}
			true
		});
		if display_changed {
			self.display_revision = self.display_revision.wrapping_add(1);
		}
		while self.overlay_bytes() > MAX_BYTES {
			if let Some((_, old)) = self.entries.pop_front() {
				self.bytes -= old.bytes();
			} else {
				break;
			}
		}
	}
	pub(crate) fn refresh(&mut self, list: &model::MemberList) {
		let Some(guild) = list.guild else { return };
		let mut rows: Vec<_> = list
			.slots
			.iter()
			.flatten()
			.filter_map(|slot| match slot {
				MemberSlot::Person(member) => Some(member.clone()),
				_ => None,
			})
			.collect();
		let partial: Vec<_> = rows
			.extract_if(.., |member| {
				list.missing_identity
					.iter()
					.any(|fields| fields.user == member.user.id)
			})
			.collect();
		for member in &partial {
			let fields = list
				.missing_identity
				.iter()
				.find(|fields| fields.user == member.user.id)
				.expect("partial row has field metadata");
			if fields.nick && fields.roles {
				continue;
			}
			let nick = if fields.nick {
				Patch::Absent
			} else {
				member.nick.clone().map_or(Patch::Null, Patch::Value)
			};
			let display_changed = !fields.nick
				&& self
					.get(guild, member.user.id)
					.is_none_or(|old| old.nick != nick);
			self.insert(
				MemberIdentity {
					guild,
					user: member.user.id,
					profile: None,
					avatar: Patch::Absent,
					nick,
					roles: if fields.roles {
						Patch::Absent
					} else {
						Patch::Value(member.roles.clone())
					},
					departed: false,
				},
				display_changed,
			);
		}
		self.reconcile(guild, &mut rows, self.revision);
		self.remember_with_source(guild, &rows, true);
		self.remember_with_source(guild, &partial, false);
		for (revision, entry) in &mut self.entries {
			if entry.guild == guild && rows.iter().any(|row| row.user.id == entry.user) {
				*revision = self.revision;
			}
		}
	}
}

impl State {
	/// The visible conversation's bounded live identity watch, independent of op-8 lookup admission.
	pub fn watched_member_users(&self) -> Option<(Id, Id, Vec<Id>)> {
		let channel = self.selected?;
		let guild = self.channel(channel)?.guild?;
		if self.auth != crate::auth::AuthState::Authenticated
			|| !self.gateway_connected
			|| !self.can_view(channel)
		{
			return None;
		}
		let mut users = Vec::with_capacity(128);
		for user in self
			.member_labels
			.visible_users
			.iter()
			.copied()
			.filter(|_| self.member_labels.visible_scope == Some((guild, channel)))
			.chain(self.timeline.iter().rev().map(|m| m.author.id))
			.chain(
				self.member_search
					.iter()
					.flat_map(|v| v.rows.iter().chain(&v.retained))
					.map(|m| m.user.id),
			)
			.chain(self.observed_member_users(channel).map(|u| u.id))
		{
			if user.0 != 0
				&& !users.contains(&user)
				&& !self
					.member_labels
					.get(guild, user)
					.is_some_and(|entry| entry.departed)
			{
				users.push(user);
			}
			if users.len() == 128 {
				break;
			}
		}
		users.sort_unstable();
		Some((guild, channel, users))
	}
	pub fn member_watch_revision(&self) -> u64 {
		self.member_labels.watch_revision
	}
	/// Revision of bounded display observations, for deduplicated asynchronous disk work.
	pub fn member_display_revision(&self) -> u64 {
		self.member_labels.revision()
	}
	/// Revision of the persisted name/username/nickname projection, independent of
	/// observation ordering used to reject stale lookups and disk reads.
	pub fn member_display_cache_revision(&self) -> u64 {
		self.member_labels.display_revision
	}
	/// Guilds with retained member observations, bounded by the existing label caps.
	pub fn cached_member_guilds(&self) -> Vec<Id> {
		self.member_labels
			.members
			.iter()
			.map(|(guild, _, _, _)| *guild)
			.chain(
				self.member_labels
					.entries
					.iter()
					.map(|(_, entry)| entry.guild),
			)
			.collect::<std::collections::BTreeSet<_>>()
			.into_iter()
			.collect()
	}
	pub fn cached_member_displays(&self, guild: Id) -> Vec<MemberDisplay> {
		let mut rows = std::collections::BTreeMap::new();
		for (_, member, _, _) in self
			.member_labels
			.members
			.iter()
			.filter(|(g, _, _, verified)| *g == guild && *verified)
		{
			let row = MemberDisplay {
				guild,
				user: member.user.id,
				name: member.user.name.clone(),
				username: member.user.username.clone(),
				nick: member.nick.clone(),
			};
			if row.valid() {
				rows.insert(row.user, row);
			}
		}
		for (_, entry) in self
			.member_labels
			.entries
			.iter()
			.filter(|(_, e)| e.guild == guild)
		{
			if entry.departed {
				rows.remove(&entry.user);
				continue;
			}
			if let Some(profile) = &entry.profile
				&& !matches!(entry.nick, Patch::Absent)
			{
				let row = MemberDisplay {
					guild,
					user: entry.user,
					name: profile.name.clone(),
					username: profile.username.clone(),
					nick: match &entry.nick {
						Patch::Value(nick) if !nick.is_empty() => Some(nick.clone()),
						_ => None,
					},
				};
				if row.valid() {
					rows.insert(row.user, row);
				}
			}
		}
		rows.into_values().collect()
	}
	/// Whether a single current observation can be written as a complete display
	/// row. This does not confer membership, permissions or profile freshness.
	pub fn has_cached_member_display(&self, guild: Id, user: Id) -> bool {
		let valid = |profile: &User, nick: Option<&str>| {
			MemberDisplay {
				guild,
				user,
				name: profile.name.clone(),
				username: profile.username.clone(),
				nick: nick.map(str::to_owned),
			}
			.valid()
		};
		if let Some(update) = self.member_labels.get(guild, user) {
			if update.departed {
				return false;
			}
			if let Some(profile) = &update.profile {
				match &update.nick {
					Patch::Value(nick)
						if valid(profile, (!nick.is_empty()).then_some(nick.as_str())) =>
					{
						return true;
					}
					Patch::Null if valid(profile, None) => return true,
					_ => {}
				}
			}
		}
		self.member_labels
			.member(guild, user)
			.is_some_and(|member| valid(&member.user, member.nick.as_deref()))
	}
	/// Targeted local suggestions remain older than every live query or observation.
	/// They fill presentation hints without completing a Gateway request or inventing roles.
	pub fn hydrate_member_search_displays(
		&mut self,
		request: &member_search::Request,
		started_revision: u64,
		rows: Vec<MemberDisplay>,
	) -> bool {
		if !request.valid()
			|| self.selected != Some(request.channel)
			|| self
				.channel(request.channel)
				.and_then(|channel| channel.guild)
				!= Some(request.guild)
			|| !self.can_view(request.channel)
			|| !self.gateway_connected
			|| self.auth != crate::auth::AuthState::Authenticated
			|| self.member_labels.revision() != started_revision
			|| self.member_search[request.slot].request.as_ref() != Some(request)
			|| (self.member_search[request.slot].finished
				&& self.member_search[request.slot].error.is_none())
			|| rows.len() > member_search::LIMIT
			|| rows.iter().map(MemberDisplay::bytes).sum::<usize>() > member_search::MAX_BYTES
		{
			return false;
		}
		let query = request.query.to_lowercase();
		let exact = request.query.parse::<Id>().ok();
		if rows.iter().any(|row| {
			row.guild != request.guild
				|| !row.valid()
				|| if request.slot == 1 {
					!request.users.contains(&row.user)
				} else if let Some(exact) = exact {
					row.user != exact
				} else {
					![
						Some(row.name.as_str()),
						row.username.as_deref(),
						row.nick.as_deref(),
					]
					.into_iter()
					.flatten()
					.any(|name| name.to_lowercase().contains(&query))
				}
		}) {
			return false;
		}
		let needed = rows
			.iter()
			.filter(|row| {
				self.member_labels.get(request.guild, row.user).is_none()
					&& self.member_labels.member(request.guild, row.user).is_none()
			})
			.count();
		let mut removed = false;
		while self.member_labels.entries.len().saturating_add(needed) > LIMIT {
			let Some(index) = self
				.member_labels
				.entries
				.iter()
				.position(|(revision, entry)| {
					*revision == 0
						&& !rows
							.iter()
							.any(|row| row.guild == entry.guild && row.user == entry.user)
				})
			else {
				break;
			};
			let (_, old) = self
				.member_labels
				.entries
				.remove(index)
				.expect("hint index checked");
			self.member_labels.bytes -= old.bytes();
			removed = true;
		}
		if removed {
			self.revision = self.revision.wrapping_add(1);
			self.skip_identity_navigation();
		}
		self.hydrate_member_displays(request.guild, started_revision, rows);
		true
	}
	/// Older disk observations never override a live result, removal or account transition.
	pub fn hydrate_member_displays(
		&mut self,
		guild: Id,
		started_revision: u64,
		rows: Vec<MemberDisplay>,
	) {
		if self.member_labels.revision() != started_revision
			|| !self.gateway_connected
			|| self.auth != crate::auth::AuthState::Authenticated
			|| !self.selected.is_some_and(|channel| {
				self.channel(channel)
					.is_some_and(|c| c.guild == Some(guild))
					&& self.can_view(channel)
			}) || rows.len() > 4096
			|| rows.iter().any(|row| row.guild != guild || !row.valid())
			|| rows.iter().map(MemberDisplay::bytes).sum::<usize>() > 2 * 1024 * 1024
		{
			return;
		}
		let mut changed = false;
		for row in rows {
			if self.member_labels.get(guild, row.user).is_some()
				|| self.member_labels.member(guild, row.user).is_some()
			{
				continue;
			}
			// Never evict a known live identity or departure to install an older disk name.
			if self.member_labels.entries.len() >= LIMIT {
				break;
			}
			let update = MemberIdentity {
				guild,
				user: row.user,
				profile: Some(User {
					member_avatar: None,
					id: row.user,
					name: row.name,
					username: row.username,
					avatar: None,
					discriminator: 0,
					primary_guild: None,
					cosmetics: None,
					webhook: false,
					kind: Default::default(),
				}),
				avatar: Patch::Absent,
				nick: row.nick.map_or(Patch::Null, Patch::Value),
				roles: Patch::Absent,
				departed: false,
			};
			let capacity = self.member_labels.entries.capacity();
			let reserve = if self.member_labels.entries.len() == capacity {
				capacity.max(2) * size_of::<(u64, MemberIdentity)>()
			} else {
				0
			};
			if self.member_labels.overlay_bytes() + reserve + update.bytes() > MAX_BYTES {
				break;
			}
			self.member_labels.insert_hint(update);
			changed = true;
		}
		if !changed {
			return;
		}

		self.revision = self.revision.wrapping_add(1);
		self.skip_identity_navigation();
		self.enforce_resident_budget();
	}
	pub fn member_identity(&self, channel: Id, user: Id) -> Option<&MemberIdentity> {
		if !self.can_view(channel) {
			return None;
		}
		self.member_labels.get(self.channel(channel)?.guild?, user)
	}
	/// Outer None means unknown; Some(None) is an authoritative nickname removal.
	pub fn member_nickname(&self, channel: Id, user: Id) -> Option<Option<&str>> {
		let guild = self.channel(channel)?.guild?;
		if !self.can_view(channel) {
			return None;
		}
		if let Some(update) = self.member_identity(channel, user) {
			match &update.nick {
				Patch::Value(nick) => return Some((!nick.is_empty()).then_some(nick.as_str())),
				Patch::Null => return Some(None),
				Patch::Absent => {}
			}
		}
		if let Some(list) = self
			.members
			.as_ref()
			.filter(|list| list.guild == Some(guild) && list.channel == channel)
			&& !list
				.missing_identity
				.iter()
				.any(|fields| fields.user == user && fields.nick)
			&& let Some(member) = list.slots.iter().flatten().find_map(|slot| match slot {
				MemberSlot::Person(member) if member.user.id == user => Some(member),
				_ => None,
			}) {
			return Some(member.nick.as_deref());
		}
		self.member_labels
			.verified_member(guild, user)
			.map(|member| member.nick.as_deref())
			.or_else(|| {
				self.timeline
					.iter()
					.rev()
					.filter(|message| {
						message.channel == channel
							&& message.author.id == user
							&& !message.author.webhook
					})
					.find_map(|message| message.author_nick.as_deref().map(Some))
			})
	}
	/// An incomplete list row cannot assert that its absent role array is empty.
	pub(crate) fn presentation_member_roles(&self, channel: Id, user: Id) -> Option<&[Id]> {
		let guild = self.channel(channel)?.guild?;
		if !self.can_view(channel) {
			return None;
		}
		if let Some(list) = self
			.members
			.as_ref()
			.filter(|list| list.guild == Some(guild) && list.channel == channel)
			&& !list
				.missing_identity
				.iter()
				.any(|fields| fields.user == user && fields.roles)
			&& let Some(member) = list.slots.iter().flatten().find_map(|slot| match slot {
				MemberSlot::Person(member) if member.user.id == user => Some(member),
				_ => None,
			}) {
			return Some(member.roles.as_slice());
		}
		self.member_labels
			.verified_member(guild, user)
			.map(|member| member.roles.as_slice())
	}
	pub fn observed_member_users(
		&self,
		channel: Id,
	) -> impl Iterator<Item = std::borrow::Cow<'_, User>> {
		let guild = self
			.channel(channel)
			.filter(|_| self.can_view(channel))
			.and_then(|c| c.guild);
		self.member_labels
			.entries
			.iter()
			.rev()
			.filter_map(move |(_, update)| {
				if update.departed || Some(update.guild) != guild {
					return None;
				}
				if let Some(user) = &update.profile {
					return Some(std::borrow::Cow::Borrowed(user));
				}
				let Patch::Value(nick) = &update.nick else {
					return None;
				};
				if nick.is_empty() {
					return None;
				}
				Some(std::borrow::Cow::Owned(User {
					member_avatar: None,
					id: update.user,
					name: nick.clone(),
					username: None,
					avatar: None,
					discriminator: 0,
					primary_guild: None,
					cosmetics: None,
					webhook: false,
					kind: Default::default(),
				}))
			})
			.chain(
				self.member_labels
					.members
					.iter()
					.rev()
					.filter_map(move |(g, member, _, _)| {
						(Some(*g) == guild
							&& !self
								.member_labels
								.get(*g, member.user.id)
								.is_some_and(|e| e.departed))
						.then_some(std::borrow::Cow::Borrowed(&member.user))
					}),
			)
	}
	/// Bounded live/presentation identities retained for one server, without changing selection.
	pub fn force_sync_known_member_ids(&self, guild: Id) -> Vec<Id> {
		let mut ids: std::collections::BTreeSet<_> = self
			.member_labels
			.members
			.iter()
			.filter(|(scope, _, _, _)| *scope == guild)
			.map(|(_, member, _, _)| member.user.id)
			.chain(
				self.member_labels
					.entries
					.iter()
					.filter(|(_, entry)| entry.guild == guild && !entry.departed)
					.map(|(_, entry)| entry.user),
			)
			.collect();
		let people = |slots: &[Option<MemberSlot>]| {
			slots
				.iter()
				.filter_map(|slot| match slot {
					Some(MemberSlot::Person(member)) => Some(member.user.id),
					_ => None,
				})
				.collect::<Vec<_>>()
		};
		if let Some(list) = &self.members
			&& list.guild == Some(guild)
		{
			ids.extend(people(&list.slots));
			for chunk in self.member_chunks.chunks.values() {
				ids.extend(people(chunk));
			}
		}
		if let Some(parked) = &self.parked_members
			&& parked.list.guild == Some(guild)
		{
			ids.extend(people(&parked.list.slots));
			for chunk in parked.chunks.chunks.values() {
				ids.extend(people(chunk));
			}
		}
		for view in &self.member_search {
			if view
				.request
				.as_ref()
				.is_some_and(|request| request.guild == guild)
			{
				ids.extend(
					view.rows
						.iter()
						.chain(&view.retained)
						.map(|member| member.user.id),
				);
			}
		}
		if self
			.selected
			.and_then(|channel| self.channel(channel))
			.and_then(|channel| channel.guild)
			== Some(guild)
		{
			ids.extend(
				self.timeline
					.iter()
					.filter(|message| !message.author.webhook)
					.map(|message| message.author.id),
			);
		}
		if let Some(profile) = &self.profile
			&& profile.guild == Some(guild)
		{
			ids.insert(profile.user);
		}
		ids.into_iter().collect()
	}
	/// Confirmed service absence retires only this server's identity and scoped consumers.
	pub(crate) fn retire_force_sync_member(&mut self, guild: Id, user: Id) -> bool {
		if self.auth != crate::auth::AuthState::Authenticated
			|| !self.gateway_connected
			|| self.guild(guild).is_none()
			|| user.0 == 0
			|| self.user.as_ref().is_none_or(|own| own.id == user)
		{
			return false;
		}
		self.apply_member_identity(MemberIdentity {
			guild,
			user,
			profile: None,
			avatar: Patch::Null,
			nick: Patch::Null,
			roles: Patch::Value(vec![]),
			departed: true,
		});
		true
	}
	pub(crate) fn apply_member_identity(&mut self, update: MemberIdentity) {
		self.apply_member_identity_inner(update, false);
	}
	/// A correlated Force Sync read can update the requested guild without changing selection.
	pub(crate) fn apply_force_sync_identity(&mut self, update: MemberIdentity) {
		self.apply_member_identity_inner(update, true);
	}
	fn apply_member_identity_inner(&mut self, mut update: MemberIdentity, background: bool) {
		let channel = self.selected.filter(|channel| self.can_view(*channel));
		let selected_scope = channel
			.and_then(|channel| self.channel(channel))
			.and_then(|channel| channel.guild)
			== Some(update.guild);
		// A valid RESUME replays dispatches before its terminal RESUMED event. Account
		// generation and permission scope still gate these observations while reconnecting.
		if self.auth != crate::auth::AuthState::Authenticated
			|| !update.valid()
			|| self.guild(update.guild).is_none()
			|| (!selected_scope && !update.departed && !background)
		{
			return;
		}
		if matches!(update.avatar, Patch::Absent)
			&& let Some(avatar) = update
				.profile
				.as_ref()
				.and_then(|user| user.member_avatar.as_ref())
			&& avatar.guild == update.guild
			&& avatar.valid()
		{
			update.avatar = avatar.hash.clone().map_or(Patch::Null, Patch::Value);
		}
		if let Some(channel) = channel.filter(|_| selected_scope)
			&& let Some(profile) = &mut update.profile
		{
			let known = self
				.display_member(channel, update.user)
				.map(|member| &member.user)
				.or_else(|| {
					self.member_labels
						.get(update.guild, update.user)
						.and_then(|old| old.profile.as_ref())
				})
				.or_else(|| {
					self.timeline
						.iter()
						.rev()
						.find(|message| {
							message.channel == channel
								&& message.author.id == update.user
								&& !message.author.webhook
						})
						.map(|message| &message.author)
				});
			if let Some(known) = known {
				let mut merged = known.clone();
				merged.name.clone_from(&profile.name);
				merged.username.clone_from(&profile.username);
				if let Some(avatar) = &profile.member_avatar
					&& avatar.guild == update.guild
					&& avatar.valid()
				{
					merged.member_avatar = Some(avatar.clone());
				}
				*profile = merged;
			}
		}
		if !update.departed
			&& update.profile.is_none()
			&& matches!(update.nick, Patch::Absent)
			&& matches!(update.roles, Patch::Absent)
			&& matches!(update.avatar, Patch::Absent)
		{
			return;
		}
		// A later live observation can fill fields omitted by the current sidebar
		// snapshot, even when the value itself is unchanged.
		fn resolve_list_fields(list: &mut model::MemberList, update: &MemberIdentity) {
			if list.guild != Some(update.guild) {
				return;
			}
			list.missing_identity.retain_mut(|fields| {
				if fields.user != update.user {
					return true;
				}
				if update.departed {
					return false;
				}
				fields.nick &= matches!(update.nick, Patch::Absent);
				fields.roles &= matches!(update.roles, Patch::Absent);
				fields.nick || fields.roles
			});
		}
		if let Some(list) = &mut self.members {
			resolve_list_fields(list, &update);
		}
		if let Some(parked) = &mut self.parked_members {
			resolve_list_fields(&mut parked.list, &update);
		}
		if self
			.member_labels
			.get(update.guild, update.user)
			.is_some_and(|old| {
				old.departed == update.departed
					&& (update.profile.is_none() || update.profile == old.profile)
					&& (matches!(update.nick, Patch::Absent) || update.nick == old.nick)
					&& (matches!(update.roles, Patch::Absent) || update.roles == old.roles)
					&& (matches!(update.avatar, Patch::Absent) || update.avatar == old.avatar)
			}) {
			// Still advance observation ordering to fence in-flight search replies, without
			// relaying unchanged typing metadata through every cached row and layout.
			self.member_labels.insert(update, false);
			return;
		}
		let previous = self.member_labels.get(update.guild, update.user);
		let display_changed = update.departed
			|| previous.is_some_and(|old| old.departed)
			|| update.profile.as_ref().is_some_and(|profile| {
				previous
					.and_then(|old| old.profile.as_ref())
					.is_none_or(|old| old.name != profile.name || old.username != profile.username)
			}) || (!matches!(update.nick, Patch::Absent)
			&& previous.is_none_or(|old| old.nick != update.nick));
		fn patch_slots(slots: &mut [Option<MemberSlot>], update: &MemberIdentity) {
			for slot in slots {
				if let Some(MemberSlot::Person(member)) = slot
					&& member.user.id == update.user
				{
					if update.departed {
						*slot = None;
					} else {
						update.apply(member);
					}
				}
			}
		}
		if let Some(list) = &mut self.members
			&& list.guild == Some(update.guild)
		{
			patch_slots(&mut list.slots, &update);
			let mut bytes = list.slot_bytes();
			for slot in list.slots.iter_mut().rev() {
				if bytes <= 256 * 1024 {
					break;
				}
				if let Some(old) = slot.take() {
					bytes -= old.bytes();
				}
			}
			for slots in self.member_chunks.chunks.values_mut() {
				patch_slots(slots, &update);
			}
			self.member_chunks.evict(&list.ranges);
			// Identity growth must respect the cap even if every remaining chunk is visible.
			while self.member_chunks.bytes() > crate::MEMBER_CACHE_BYTES {
				let Some(key) = self
					.member_chunks
					.chunks
					.keys()
					.copied()
					.max_by_key(|start| start.abs_diff(self.member_chunks.viewport))
				else {
					break;
				};
				self.member_chunks.chunks.remove(&key);
			}
		}
		if let Some(parked) = &mut self.parked_members
			&& parked.list.guild == Some(update.guild)
		{
			patch_slots(&mut parked.list.slots, &update);
			for slots in parked.chunks.chunks.values_mut() {
				patch_slots(slots, &update);
			}
			let mut bytes = parked.list.slot_bytes();
			for slot in parked.list.slots.iter_mut().rev() {
				if bytes <= 256 * 1024 {
					break;
				}
				if let Some(old) = slot.take() {
					bytes -= old.bytes();
				}
			}
			parked.chunks.evict(&parked.list.ranges);
			while parked.chunks.bytes() > crate::MEMBER_CACHE_BYTES {
				let Some(key) = parked.chunks.chunks.keys().next_back().copied() else {
					break;
				};
				parked.chunks.chunks.remove(&key);
			}
		}
		for view in &mut self.member_search {
			if view
				.request
				.as_ref()
				.is_some_and(|r| r.guild == update.guild)
			{
				for rows in [&mut view.rows, &mut view.retained] {
					rows.retain_mut(|member| {
						if member.user.id != update.user {
							return true;
						}
						if update.departed {
							return false;
						}
						update.apply(member);
						true
					});
					let mut bytes = rows.iter().map(Member::bytes).sum::<usize>();
					while bytes > member_search::MAX_BYTES {
						bytes -= rows.remove(0).bytes();
					}
				}
			}
		}
		if selected_scope {
			self.timeline.apply_author_identity(&update);
		}
		// Only guild-scoped profiles may inherit a member avatar; global profile views
		// keep the account artwork. Cached cards are updated alongside the open card.
		let refresh_profile = |profile: &mut model::UserProfile| {
			if update.departed {
				if profile
					.guild
					.as_ref()
					.is_some_and(|guild| guild.guild == update.guild)
				{
					profile.guild = None;
				}
				return;
			}
			let Some(guild) = &mut profile.guild else {
				return;
			};
			if guild.guild != update.guild || profile.user.id != update.user {
				return;
			}
			let avatar = match &update.avatar {
				Patch::Value(hash) => Some(Some(hash.clone())),
				Patch::Null => Some(None),
				Patch::Absent => update
					.profile
					.as_ref()
					.and_then(|user| user.member_avatar.as_ref())
					.filter(|avatar| avatar.guild == update.guild)
					.map(|avatar| avatar.hash.clone()),
			};
			if let Some(avatar) = avatar {
				guild.avatar = avatar;
			}
		};
		if let Some(profile) = &mut self.profile
			&& profile.user == update.user
			&& profile.guild == Some(update.guild)
			&& let Some(data) = &mut profile.data
		{
			refresh_profile(data);
		}
		self.profile_cache.update(update.user, refresh_profile);
		if update.departed && selected_scope {
			self.typing.remove(update.user);
		}
		self.member_labels.insert(update, display_changed);
		self.revision = self.revision.wrapping_add(1);
		self.skip_identity_navigation();
		self.enforce_resident_budget();
	}
}

#[cfg(test)]
mod cache_search_tests {
	use super::*;

	fn state() -> State {
		let mut state = State {
			auth: crate::auth::AuthState::Authenticated,
			gateway_connected: true,
			user: Some(crate::tests::message(1).author),
			selected: Some(Id(1)),
			guilds: vec![model::Guild {
				default_message_notifications: None,
				stickers: None,
				emojis: None,
				id: Id(2),
				name: "Synthetic server".into(),
				icon: None,
			}],
			channels: vec![model::Channel {
				icon: None,
				last_message: None,
				id: Id(1),
				guild: Some(Id(2)),
				parent_id: None,
				position: 0,
				name: "synthetic".into(),
				kind: 0,
				recipients: vec![],
				member_list_id: None,
				message_count: None,
				tags: None,
			}],
			..State::default()
		};
		crate::tests::grant_permissions(&mut state);
		state
	}

	fn row(user: u64, nick: &str) -> MemberDisplay {
		MemberDisplay {
			guild: Id(2),
			user: Id(user),
			name: "Synthetic person".into(),
			username: Some("synthetic_account".into()),
			nick: Some(nick.into()),
		}
	}

	#[test]
	fn targeted_disk_search_supplies_nickname_without_completing_the_live_query() {
		let mut state = state();
		let Some(crate::Command::MemberSearch(request)) =
			state.search_members(Id(1), "FarAwayNick", 0)
		else {
			panic!("member query");
		};
		let revision = state.member_display_revision();
		assert!(state.hydrate_member_search_displays(
			&request,
			revision,
			vec![row(9000, "FarAwayNick")]
		));
		assert_eq!(
			state.member_nickname(Id(1), Id(9000)),
			Some(Some("FarAwayNick"))
		);
		assert!(!state.member_search[0].finished);
		assert!(state.member_search[0].rows.is_empty());
		assert!(matches!(
			state.member_identity(Id(1), Id(9000)).unwrap().roles,
			Patch::Absent
		));
		assert!(matches!(
			state.member_identity(Id(1), Id(9000)).unwrap().avatar,
			Patch::Absent
		));
	}

	#[test]
	fn live_results_and_changed_queries_fence_delayed_disk_searches() {
		let mut state = state();
		let Some(crate::Command::MemberSearch(old)) = state.search_members(Id(1), "OldNick", 0)
		else {
			panic!("old query");
		};
		let revision = state.member_display_revision();
		let Some(crate::Command::MemberSearch(current)) = state.search_members(Id(1), "NewNick", 0)
		else {
			panic!("new query");
		};
		assert!(!state.hydrate_member_search_displays(&old, revision, vec![row(9000, "OldNick")]));
		let mut user = crate::tests::message(1).author;
		user.id = Id(9000);
		state.searched_members(
			current.clone(),
			Ok(vec![Member {
				roles: vec![],
				user,
				nick: Some("NewNick".into()),
				status: None,
				custom_status: None,
				activities: vec![],
				clients: Default::default(),
			}]),
		);
		assert!(!state.hydrate_member_search_displays(
			&current,
			revision,
			vec![row(9000, "NewNickFromDisk")]
		));
		assert_eq!(
			state.member_nickname(Id(1), Id(9000)),
			Some(Some("NewNick"))
		);
	}

	#[test]
	fn targeted_search_replaces_old_hints_within_existing_limits() {
		let mut state = state();
		let revision = state.member_display_revision();
		state.hydrate_member_displays(
			Id(2),
			revision,
			(1000..1000 + LIMIT as u64)
				.map(|user| row(user, "Old cached nickname"))
				.collect(),
		);
		assert_eq!(state.member_labels.entries.len(), LIMIT);
		let Some(crate::Command::MemberSearch(request)) =
			state.search_members(Id(1), "TargetNickname", 0)
		else {
			panic!("target query");
		};
		assert!(state.hydrate_member_search_displays(
			&request,
			state.member_display_revision(),
			vec![row(9000, "TargetNickname")]
		));
		assert_eq!(
			state.member_nickname(Id(1), Id(9000)),
			Some(Some("TargetNickname"))
		);
		assert!(state.member_labels.entries.len() <= LIMIT);
		assert!(state.member_labels.overlay_bytes() <= MAX_BYTES);
		assert!(!state.member_search[0].finished);
	}
}
