//! Bounded session activity; badge counts never use snowflake subtraction.
use crate::{MAX_NAV, State};
use model::{Id, Message};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_OBSERVED: usize = 4096;
const MAX_OBSERVED_BYTES: usize = 128 * 1024;
const MAX_NOTIFICATIONS: usize = 32;
const MAX_NOTIFICATION_BYTES: usize = 16 * 1024;
const MAX_SETTINGS_BYTES: usize = 32 * 1024 * 1024;

pub struct Notification {
	pub channel: Id,
	pub message: Id,
	author: Id,
	pub sender: String,
	pub preview: String,
	pub avatar_key: String,
	direct: bool,
	everyone: bool,
	roles: Vec<Id>,
}
impl Notification {
	fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.roles.capacity() * size_of::<Id>()
			+ self.sender.capacity()
			+ self.preview.capacity()
			+ self.avatar_key.capacity()
	}
}

fn alert_text(text: &str, chars: usize, bytes: usize) -> String {
	let mut output = String::new();
	for character in text.chars().take(chars) {
		let character = if character.is_control() || character.is_whitespace() {
			' '
		} else {
			character
		};
		if output.len() + character.len_utf8() > bytes {
			break;
		}
		output.push(character);
	}
	output.trim().to_owned()
}
#[derive(Default)]
pub(crate) struct Activity {
	counts: BTreeMap<Id, u32>,
	high_water: BTreeMap<Id, Id>,
	observed_counts: BTreeMap<Id, (u32, u32)>,
	observed: VecDeque<(Id, Id, bool)>,
	notifications: VecDeque<Notification>,
}
impl Activity {
	pub(crate) fn clear_notifications(&mut self) {
		self.notifications.clear();
	}
	pub(crate) fn forget(&mut self, channel: Id) {
		self.high_water.remove(&channel);
		self.revoke(channel);
	}
	fn revoke(&mut self, channel: Id) {
		self.counts.remove(&channel);
		self.observed_counts.remove(&channel);
		self.observed.retain(|(c, ..)| *c != channel);
		self.notifications.retain(|n| n.channel != channel);
	}
	pub(crate) fn delete(&mut self, channel: Id, message: Id) {
		self.observed
			.retain(|(c, id, _)| (*c, *id) != (channel, message));
		self.notifications
			.retain(|n| (n.channel, n.message) != (channel, message));
		if self.observed_counts.contains_key(&channel) {
			self.recount(channel);
		}
	}
	pub(crate) fn observe_latest(&mut self, channel: Id, message: Id) {
		let latest = self.high_water.entry(channel).or_insert(message);
		*latest = (*latest).max(message);
	}
	pub(crate) fn set_count(&mut self, channel: Id, count: u32) {
		self.counts.insert(channel, count);
	}
	pub(crate) fn ack(&mut self, channel: Id, message: Option<Id>, count: Option<u32>) {
		self.observed
			.retain(|(c, id, _)| *c != channel || message.is_none_or(|read| *id > read));
		self.notifications
			.retain(|n| n.channel != channel || message.is_none_or(|read| n.message > read));
		// The service count includes messages after this ACK. Avoid counting those twice.
		if let Some(count) = count {
			self.counts.insert(channel, count);
			self.observed.retain(|(c, ..)| *c != channel);
		} else {
			// Without a refreshed service count, only retained observed messages are countable.
			self.counts.remove(&channel);
		}
		self.recount(channel);
	}
	fn recount(&mut self, channel: Id) {
		let mut counts = (0, 0);
		for (c, _, mention) in &self.observed {
			if *c == channel {
				counts.0 += 1;
				counts.1 += u32::from(*mention);
			}
		}
		if counts.0 == 0 {
			self.observed_counts.remove(&channel);
		} else {
			self.observed_counts.insert(channel, counts);
		}
	}
	fn count(&self, channel: Id, mentions_only: bool) -> u32 {
		let counts = self
			.observed_counts
			.get(&channel)
			.copied()
			.unwrap_or_default();
		self.counts
			.get(&channel)
			.copied()
			.unwrap_or(0)
			.saturating_add(if mentions_only { counts.1 } else { counts.0 })
	}
}

#[derive(Clone, Default)]
pub struct Setting {
	/// True only after a validated full settings row or complete snapshot fallback.
	pub overrides_known: bool,
	pub mute_until: Option<i64>,
	pub guild: Option<Id>,
	pub muted: Option<bool>,
	pub level: Option<u8>,
	pub suppress_everyone: Option<bool>,
	pub suppress_roles: Option<bool>,
	pub hide_muted_channels: Option<bool>,
	pub channels: Vec<(Id, Option<bool>, Option<u8>)>,
	pub channel_mute_until: Vec<(Id, i64)>,
}
impl Setting {
	fn muted(&self) -> Option<bool> {
		if self.muted == Some(true)
			&& self
				.mute_until
				.is_some_and(|until| until <= State::permission_time())
		{
			Some(false)
		} else {
			self.muted
		}
	}
	fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.channels.capacity() * size_of::<(Id, Option<bool>, Option<u8>)>()
			+ self.channel_mute_until.capacity() * size_of::<(Id, i64)>()
	}
	fn mute_expired(&self, channel: Id) -> bool {
		self.channel_mute_until
			.iter()
			.any(|(id, until)| *id == channel && *until <= State::permission_time())
	}
}
pub enum Event {
	Settings {
		entries: Vec<Setting>,
		replace: bool,
	},
	Presence(Option<bool>), // Some(true) means at least one session is DND.
	Invalidate,
}
impl Event {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Settings { entries, .. } => {
				entries.capacity() * size_of::<Setting>()
					+ entries
						.iter()
						.map(|e| {
							e.channels.capacity() * size_of::<(Id, Option<bool>, Option<u8>)>()
								+ e.channel_mute_until.capacity() * size_of::<(Id, i64)>()
						})
						.sum::<usize>()
			}
			_ => 0,
		}
	}
}
#[derive(Default)]
pub struct Preferences {
	complete: bool,
	settings: BTreeMap<Option<Id>, Setting>,
	dnd: Option<bool>,
}
impl Preferences {
	// Only the empty synthesized fallback is cloned; resident override vectors stay in place.
	fn initial_guild_setting(&self, guild: Id) -> Setting {
		if self.settings.contains_key(&Some(guild)) {
			Setting::default()
		} else {
			self.setting(Some(guild)).cloned().unwrap_or_default()
		}
	}
	fn setting(&self, guild: Option<Id>) -> Option<&Setting> {
		static DEFAULT_GUILD: Setting = Setting {
			overrides_known: true,
			guild: None,
			muted: Some(false),
			level: Some(3),
			mute_until: None,
			suppress_everyone: Some(false),
			suppress_roles: Some(false),
			hide_muted_channels: Some(false),
			channels: Vec::new(),
			channel_mute_until: Vec::new(),
		};
		self.settings
			.get(&guild)
			.or_else(|| (self.complete && guild.is_some()).then_some(&DEFAULT_GUILD))
	}
}
impl State {
	pub fn server_notification_settings(
		&self,
		guild: Id,
	) -> crate::server_actions::NotificationOptions {
		let Some(setting) = self.notification_preferences.setting(Some(guild)) else {
			return Default::default();
		};
		crate::server_actions::NotificationOptions {
			level: setting.level.filter(|level| *level <= 3),
			muted: setting.muted(),
			mute_until: (setting.muted() == Some(true))
				.then_some(setting.mute_until)
				.flatten(),
			suppress_everyone: setting.suppress_everyone,
			suppress_roles: setting.suppress_roles,
		}
	}
	pub(crate) fn confirm_server_notifications(
		&mut self,
		guild: Id,
		options: crate::server_actions::NotificationOptions,
	) -> Result<(), &'static str> {
		let initial = self.notification_preferences.initial_guild_setting(guild);
		let preferences = &mut self.notification_preferences;
		if !preferences.settings.contains_key(&Some(guild)) && preferences.settings.len() >= MAX_NAV
		{
			return Err("Notification settings exceed safe capacity");
		}
		let setting = preferences.settings.entry(Some(guild)).or_insert(Setting {
			guild: Some(guild),
			..initial
		});
		if let Some(level) = options.level {
			setting.level = Some(level);
		}
		if let Some(muted) = options.muted {
			setting.muted = Some(muted);
			setting.mute_until = options.mute_until;
		}
		if let Some(suppress) = options.suppress_everyone {
			setting.suppress_everyone = Some(suppress);
		}
		if let Some(suppress) = options.suppress_roles {
			setting.suppress_roles = Some(suppress);
		}
		self.read_state.activity.clear_notifications();
		self.check_notification_capacity()
	}
	pub(crate) fn invalidate_startup_preferences(&mut self, settings: bool, sessions: bool) {
		if settings {
			self.notification_preferences.settings.clear();
			self.notification_preferences.complete = false;
		}
		if sessions {
			self.notification_preferences.dnd = None;
		}
		self.read_state.activity.clear_notifications();
	}
	fn check_notification_capacity(&mut self) -> Result<(), &'static str> {
		let settings = &self.notification_preferences.settings;
		if settings.len() > MAX_NAV
			|| settings
				.values()
				.map(|setting| setting.bytes() + 64)
				.sum::<usize>()
				> MAX_SETTINGS_BYTES
		{
			self.notification_preferences.settings.clear();
			self.notification_preferences.complete = false;
			self.startup_warnings.notifications = true;
			self.read_state.activity.clear_notifications();
			return Err("Notification settings exceed safe byte capacity");
		}
		Ok(())
	}
	pub fn lights_guild_rail(&self, channel: &model::Channel) -> bool {
		channel.kind != 2
			&& self.channel_unread(channel) == Some(true)
			&& !self.muted_for_guild_rail(channel)
	}

	pub fn muted_for_guild_rail(&self, channel: &model::Channel) -> bool {
		let Some(guild) = channel.guild else {
			return false;
		};
		if self
			.notification_preferences
			.setting(Some(guild))
			.is_some_and(|setting| setting.muted() == Some(true))
		{
			return true;
		}
		self.guild_channel_effectively_muted(channel.id) == Some(true)
	}
	/// A bounded same-guild ancestry, nearest channel first. Invalid metadata never crosses guilds.
	pub(crate) fn notification_path(&self, channel: Id) -> Option<[Option<Id>; 8]> {
		let guild = self.channel(channel)?.guild;
		let mut path = [None; 8];
		let mut current = Some(channel);
		for index in 0..path.len() {
			let Some(id) = current else { return Some(path) };
			let source = self.channel(id)?;
			if source.guild != guild || path[..index].contains(&Some(id)) {
				return None;
			}
			path[index] = Some(id);
			current = source.parent_id;
		}
		current.is_none().then_some(path)
	}
	/// Category/channel/thread muting used by the sidebar; server mute does not hide its whole tree.
	pub fn guild_channel_effectively_muted(&self, channel: Id) -> Option<bool> {
		self.channel(channel)?.guild?;
		let mut known = true;
		for id in self.notification_path(channel)?.into_iter().flatten() {
			match self.guild_channel_muted(id) {
				Some(true) => return Some(true),
				Some(false) => {}
				None => known = false,
			}
		}
		known.then_some(false)
	}
	pub fn guild_channel_muted(&self, channel: Id) -> Option<bool> {
		let guild = self.channel(channel)?.guild?;
		let setting = self.notification_preferences.setting(Some(guild));
		let muted = setting.and_then(|s| {
			s.channels
				.iter()
				.find(|(id, ..)| *id == channel)
				.and_then(|(_, muted, _)| *muted)
		});
		if muted == Some(true) && setting.is_some_and(|s| s.mute_expired(channel)) {
			return Some(false);
		}
		muted.or_else(|| (self.demo || setting.is_some()).then_some(false))
	}
	pub fn channel_notification_level(&self, channel: Id) -> Option<u8> {
		let guild = self.channel(channel)?.guild?;
		let setting = self.notification_preferences.setting(Some(guild))?;
		setting
			.channels
			.iter()
			.find(|(id, ..)| *id == channel)
			.and_then(|(_, _, level)| *level)
			.or(Some(3))
	}
	pub(crate) fn confirm_channel_mute_timer(
		&mut self,
		channel: Id,
		muted: Option<bool>,
		until: Option<i64>,
	) -> Result<(), &'static str> {
		let Some(guild) = self.channel(channel).and_then(|c| c.guild) else {
			return Ok(());
		};
		let Some(setting) = self.notification_preferences.settings.get_mut(&Some(guild)) else {
			return Ok(());
		};
		setting.channel_mute_until.retain(|(id, _)| *id != channel);
		if muted == Some(true)
			&& let Some(until) = until
		{
			if setting.channel_mute_until.len() >= MAX_NAV {
				return Err("Mute timers exceed safe capacity");
			}
			setting.channel_mute_until.push((channel, until));
		}
		self.check_notification_capacity()
	}
	pub(crate) fn confirm_channel_preferences(
		&mut self,
		guild: Id,
		channel: Id,
		muted: Option<bool>,
		level: Option<u8>,
	) -> Result<(), &'static str> {
		let initial = self.notification_preferences.initial_guild_setting(guild);
		let preferences = &mut self.notification_preferences;
		let existing = preferences.settings.get(&Some(guild));
		if existing.is_none() && preferences.settings.len() >= MAX_NAV {
			return Err("Notification settings exceed safe capacity");
		}
		if !existing.is_some_and(|s| s.channels.iter().any(|(id, ..)| *id == channel))
			&& preferences
				.settings
				.values()
				.map(|s| s.channels.len())
				.sum::<usize>()
				>= MAX_NAV
		{
			return Err("Notification settings exceed safe capacity");
		}
		let setting = preferences
			.settings
			.entry(Some(guild))
			.or_insert_with(|| Setting {
				guild: Some(guild),
				..initial
			});
		if let Some((_, m, l)) = setting.channels.iter_mut().find(|(id, ..)| *id == channel) {
			if muted.is_some() {
				*m = muted;
			}
			if level.is_some() {
				*l = level;
			}
		} else {
			setting.channels.push((
				channel,
				muted.or(setting.overrides_known.then_some(false)),
				level.or(setting.overrides_known.then_some(3)),
			));
		}
		self.read_state.activity.clear_notifications();
		self.check_notification_capacity()
	}

	pub fn hides_muted_channels(&self, guild: Id) -> Option<bool> {
		self.notification_preferences
			.setting(Some(guild))
			.and_then(|setting| setting.hide_muted_channels)
	}

	pub(crate) fn confirm_guild_hides_muted(
		&mut self,
		guild: Id,
		hide: bool,
	) -> Result<(), &'static str> {
		let initial = self.notification_preferences.initial_guild_setting(guild);
		let preferences = &mut self.notification_preferences;
		if !preferences.settings.contains_key(&Some(guild)) && preferences.settings.len() >= MAX_NAV
		{
			return Err("Notification settings exceed safe capacity");
		}
		let setting = preferences
			.settings
			.entry(Some(guild))
			.or_insert_with(|| Setting {
				guild: Some(guild),
				..initial
			});
		setting.hide_muted_channels = Some(hide);
		self.check_notification_capacity()
	}

	/// Time until the next timed mute lapses, so counts that hide muted conversations can be
	/// refreshed by an otherwise idle window.
	pub fn next_mute_expiry(&self) -> Option<std::time::Duration> {
		self.next_mute_deadline()?
			.duration_since(std::time::SystemTime::now())
			.ok()
	}
	pub fn next_mute_deadline(&self) -> Option<std::time::SystemTime> {
		let now = Self::permission_time();
		self.notification_preferences
			.settings
			.values()
			.flat_map(|s| {
				s.mute_until
					.into_iter()
					.chain(s.channel_mute_until.iter().map(|(_, until)| *until))
			})
			.filter(|until| *until > now)
			.min()
			.and_then(|until| {
				std::time::UNIX_EPOCH
					.checked_add(std::time::Duration::from_secs(until as u64))
					.or_else(|| {
						// Windows SystemTime cannot represent every service timestamp. Recheck
						// daily without changing when the mute itself expires.
						std::time::SystemTime::now()
							.checked_add(std::time::Duration::from_secs(24 * 60 * 60))
					})
			})
	}

	/// Per-DM notification override; absent settings remain unknown outside the fixture.
	pub fn dm_muted(&self, channel: Id) -> Option<bool> {
		if let Some(muted) = self.pending_dm_muted(channel) {
			return Some(muted);
		}
		let Some(setting) = self.notification_preferences.settings.get(&None) else {
			return self.demo.then_some(false);
		};
		setting
			.channels
			.iter()
			.find(|(id, ..)| *id == channel)
			.map(|(_, muted, _)| {
				// Expired timed mutes keep `muted: true` on the service.
				if *muted == Some(true) && setting.mute_expired(channel) {
					Some(false)
				} else {
					*muted
				}
			})
			.unwrap_or_else(|| (self.demo || setting.muted.is_some()).then_some(false))
	}
	pub(crate) fn confirm_dm_muted(
		&mut self,
		channel: Id,
		muted: bool,
	) -> Result<(), &'static str> {
		let existing = self.notification_preferences.settings.get(&None);
		if !existing.is_some_and(|s| s.channels.iter().any(|(id, ..)| *id == channel))
			&& self
				.notification_preferences
				.settings
				.values()
				.map(|s| s.channels.len())
				.sum::<usize>()
				>= MAX_NAV
		{
			return Err("Notification settings exceed safe capacity");
		}
		let setting = self
			.notification_preferences
			.settings
			.entry(None)
			.or_default();
		if let Some((_, value, _)) = setting.channels.iter_mut().find(|(id, ..)| *id == channel) {
			*value = Some(muted);
		} else {
			setting.channels.push((channel, Some(muted), None));
		}
		// DM mute writes are untimed, so a stale expiry must not unmute them.
		setting.channel_mute_until.retain(|(id, _)| *id != channel);
		self.read_state.activity.clear_notifications();
		self.check_notification_capacity()
	}
	pub(crate) fn confirm_dm_timed_mute(
		&mut self,
		channel: Id,
		until: i64,
	) -> Result<(), &'static str> {
		self.confirm_dm_muted(channel, true)?;
		let setting = self
			.notification_preferences
			.settings
			.get_mut(&None)
			.unwrap();
		if setting.channel_mute_until.len() >= MAX_NAV {
			return Err("Mute timers exceed safe capacity");
		}
		setting.channel_mute_until.push((channel, until));
		self.check_notification_capacity()
	}
	/// Latest known message activity, retained across deletion for navigation ordering.
	pub fn channel_activity(&self, channel: &model::Channel) -> Id {
		self.read_state
			.activity
			.high_water
			.get(&channel.id)
			.copied()
			.max(channel.last_message)
			.unwrap_or(channel.id)
	}
	pub fn unread_directs(&self, call: Option<Id>) -> Vec<Id> {
		let mut rows: Vec<(Id, Id)> = self
			.channels
			.iter()
			.filter(|channel| {
				channel.guild.is_none()
					&& channel.supports_text()
					&& !self.message_request_pending(channel.id)
					&& !self.spam_direct(channel.id)
					&& (Some(channel.id) == call
						|| self.channel_unread(channel) == Some(true)
						|| self.unread_count(channel.id) > 0)
			})
			.map(|channel| (self.channel_activity(channel), channel.id))
			.collect();
		rows.sort_unstable_by_key(|&(activity, id)| std::cmp::Reverse((activity, id)));
		if let Some(call) = call
			&& let Some(index) = rows.iter().position(|(_, id)| *id == call)
		{
			rows[..=index].rotate_right(1);
		}
		rows.into_iter().take(15).map(|(_, id)| id).collect()
	}
	/// Service badge count plus bounded activity observed since that count. A lower bound
	/// when history or settings are incomplete; this is not an exact total unread count.
	pub fn unread_count(&self, channel: Id) -> u32 {
		if self.can_view(channel) {
			self.read_state.activity.count(channel, false)
		} else {
			0
		}
	}
	pub fn mention_count(&self, channel: Id) -> u32 {
		// Most channels have no mentions; skip the permission lookup for those.
		let count = self.read_state.activity.count(channel, true);
		if count > 0 && self.can_view(channel) {
			count
		} else {
			0
		}
	}
	/// Sidebar counts include a thread's mentions on its immediate parent. Rail totals
	/// must continue summing `mention_count`, so each conversation contributes only once.
	/// Constructed once per navigation revision, never by scanning children per painted row.
	pub fn channel_mention_counts(&self) -> BTreeMap<Id, u32> {
		let mut counts = BTreeMap::new();
		for channel in self.channels.iter().take(MAX_NAV) {
			if channel.guild.is_none() {
				continue;
			}
			let count = self.mention_count(channel.id);
			if count > 0 {
				counts.insert(channel.id, count);
			}
		}
		for channel in self.channels.iter().take(MAX_NAV) {
			if !matches!(channel.kind, 10..=12) {
				continue;
			}
			let count = counts.get(&channel.id).copied().unwrap_or(0);
			if count == 0 {
				continue;
			}
			if let Some(parent) = channel.parent_id.and_then(|id| self.channel(id))
				&& parent.guild.is_some()
				&& parent.guild == channel.guild
				&& matches!(parent.kind, 0 | 5 | 15 | 16)
				&& self.can_view(parent.id)
			{
				let entry = counts.entry(parent.id).or_default();
				*entry = entry.saturating_add(count);
			}
		}
		counts
	}
	pub(crate) fn reconcile_notifications(&mut self) {
		let activity = &self.read_state.activity;
		let revoked: BTreeSet<_> = activity
			.counts
			.keys()
			.chain(activity.observed_counts.keys())
			.copied()
			.chain(
				activity
					.notifications
					.iter()
					.map(|notification| notification.channel),
			)
			.filter(|channel| !self.can_view(*channel))
			.collect();
		for channel in revoked {
			self.read_state.activity.revoke(channel);
		}
	}
	pub fn take_notification(&mut self) -> Option<Notification> {
		while let Some(notification) = self.read_state.activity.notifications.pop_front() {
			let mention = self.mention_matches(
				notification.channel,
				notification.direct,
				notification.everyone,
				&notification.roles,
			);
			if self.user_content_restriction(notification.author).is_none()
				&& self.notification_allowed_for(notification.channel, mention)
			{
				return Some(notification);
			}
		}
		None
	}
	pub fn notification_preferences_known(&self) -> bool {
		self.notification_preferences.dnd.is_some()
			&& (self.notification_preferences.complete
				|| !self.notification_preferences.settings.is_empty())
	}
	pub fn notification_allowed(&self, channel: Id) -> bool {
		self.notification_allowed_for(channel, true)
	}
	fn notification_allowed_for(&self, channel: Id, mention: bool) -> bool {
		if !self.gateway_connected
			|| !self.can_view(channel)
			|| !self.notification_preferences_known()
			|| self.notification_preferences.dnd == Some(true)
		{
			return false;
		}
		let Some(channel) = self.channel(channel).filter(|c| c.supports_text()) else {
			return false;
		};
		if channel.guild.is_some() && self.notification_preferences.dnd.is_none() {
			return false;
		}
		if channel.guild.is_none()
			&& ((channel.kind == 1
				&& channel
					.recipients
					.iter()
					.any(|user| self.user_content_restriction(user.id).is_some()))
				|| self.message_request_pending(channel.id)
				|| self.spam_direct(channel.id)
				|| self.pending_dm_muted(channel.id) == Some(true))
		{
			return false;
		}
		let Some(setting) = self.notification_preferences.setting(channel.guild) else {
			// A missing global DM entry means default delivery, unless a known mute above applies.
			return channel.guild.is_none();
		};
		if channel.guild.is_some() && !setting.overrides_known {
			return false;
		}
		if setting.muted() == Some(true)
			|| (channel.guild.is_some() && setting.muted() != Some(false))
		{
			return false;
		}
		let mut level = setting
			.level
			.or_else(|| channel.guild.is_none().then_some(0));
		if level == Some(3) {
			level = match channel.guild {
				Some(guild) => self
					.guild(guild)
					.and_then(|guild| guild.default_message_notifications),
				None => Some(0),
			};
		}
		let Some(path) = self.notification_path(channel.id) else {
			return false;
		};
		for id in path.into_iter().rev().flatten() {
			if let Some((_, muted, override_level)) =
				setting.channels.iter().find(|(c, ..)| *c == id)
			{
				let pending = if channel.guild.is_none() && id == channel.id {
					self.pending_dm_muted(id)
				} else {
					None
				};
				let muted = pending.or(if setting.mute_expired(id) {
					Some(false)
				} else {
					*muted
				});
				if muted == Some(true) || (channel.guild.is_some() && muted != Some(false)) {
					return false;
				}
				if channel.guild.is_some() && override_level.is_none() {
					return false;
				}
				if override_level.is_some_and(|l| l != 3) {
					level = *override_level;
				}
			}
		}
		level == Some(0) || (mention && level == Some(1))
	}
	pub fn apply_notification_preferences(&mut self, event: Event) -> Result<(), &'static str> {
		self.observe_dm_settings(&event);
		self.read_state.activity.clear_notifications();
		if event.bytes() > MAX_SETTINGS_BYTES {
			self.notification_preferences = Preferences::default();
			return Err("Notification settings exceed safe byte capacity");
		}
		match event {
			Event::Presence(dnd) => {
				self.notification_preferences.dnd = dnd;
				if dnd.is_some() {
					self.startup_warnings.sessions = false;
				}
			}
			Event::Invalidate => self.notification_preferences = Preferences::default(),
			Event::Settings { entries, replace } => {
				let keys: BTreeSet<_> = entries.iter().map(|s| s.guild).collect();
				let old: Vec<_> = self
					.notification_preferences
					.settings
					.iter()
					.filter(|(key, _)| !replace && !keys.contains(key))
					.map(|(_, setting)| setting)
					.collect();
				let overrides = entries.iter().map(|s| s.channels.len()).sum::<usize>();
				let old_overrides: usize = old.iter().map(|s| s.channels.len()).sum();
				if old.len().saturating_add(entries.len()) > MAX_NAV
					|| old
						.iter()
						.map(|setting| setting.bytes() + 64)
						.sum::<usize>() + entries
						.iter()
						.map(|setting| setting.bytes() + 64)
						.sum::<usize>() > MAX_SETTINGS_BYTES
					|| overrides.saturating_add(old_overrides) > MAX_NAV
					|| keys.len() != entries.len()
					|| entries.iter().any(|s| {
						let channels: BTreeMap<_, _> = s
							.channels
							.iter()
							.map(|(id, muted, _)| (*id, *muted))
							.collect();
						if s.channel_mute_until.len() > s.channels.len()
							|| s.channel_mute_until
								.iter()
								.map(|(id, _)| *id)
								.collect::<BTreeSet<_>>()
								.len() != s.channel_mute_until.len()
							|| s.channel_mute_until.iter().any(|(id, until)| {
								*until < 0 || channels.get(id) != Some(&Some(true))
							}) {
							return true;
						}
						channels.len() != s.channels.len()
					}) {
					self.notification_preferences = Preferences::default();
					return Err(
						"Notification settings exceed safe capacity or contain duplicate IDs",
					);
				}
				self.observe_server_notification_settings(&entries);
				if replace {
					self.notification_preferences.settings.clear();
					self.startup_warnings.notifications = false;
					self.notification_preferences.complete = true;
				}
				for mut setting in entries {
					setting.overrides_known = true;
					self.notification_preferences
						.settings
						.insert(setting.guild, setting);
				}
			}
		}
		Ok(())
	}
	pub(crate) fn counts_toward_mention_badge(&self, message: &Message) -> bool {
		// Private recipient-remove rows are not mentions. Discord message type 2.
		if message.kind == 2
			|| self.user_content_restriction(message.author.id).is_some()
			|| !model::valid_mention_roles(&message.mention_roles)
			|| self
				.user
				.as_ref()
				.is_some_and(|owner| message.author.id == owner.id)
		{
			return false;
		}
		let direct = self
			.user
			.as_ref()
			.is_some_and(|owner| message.mentions.iter().any(|user| user.id == owner.id));
		self.mention_matches(
			message.channel,
			direct,
			message.mention_everyone,
			&message.mention_roles,
		)
	}
	fn mention_matches(&self, channel: Id, direct: bool, everyone: bool, roles: &[Id]) -> bool {
		let Some(channel) = self.channel(channel) else {
			return false;
		};
		if direct || channel.guild.is_none() {
			return true;
		}
		let Some(guild) = channel.guild else {
			return false;
		};
		let Some(setting) = self.notification_preferences.setting(Some(guild)) else {
			return false;
		};
		(everyone && setting.suppress_everyone == Some(false))
			|| (setting.suppress_roles == Some(false)
				&& self
					.permissions
					.guilds
					.get(&guild)
					.and_then(|g| g.member.as_ref())
					.is_some_and(|member| roles.iter().any(|role| member.roles.contains(role))))
	}
	fn alert_preview(&self, message: &Message, mut text: &str) -> String {
		let guild = self
			.channel(message.channel)
			.and_then(|channel| channel.guild);
		let mut output = String::new();
		let mut count = 0;
		while !text.is_empty() && count < 160 && output.len() < 512 {
			let (prefix, name, len) = if let Some((id, len)) = model::user_mention_prefix(text) {
				let member = self
					.members
					.as_ref()
					.filter(|list| list.guild == guild)
					.and_then(|list| {
						list.slots
							.iter()
							.flatten()
							.filter_map(|slot| match slot {
								model::MemberSlot::Person(m) => Some(m),
								_ => None,
							})
							.find(|member| member.user.id == id)
					});
				let user = message
					.mentions
					.iter()
					.find(|user| user.id == id)
					.or_else(|| self.user.as_ref().filter(|user| user.id == id))
					.or_else(|| self.friend(id));
				let name = member
					.and_then(|member| member.nick.as_deref().filter(|nick| !nick.is_empty()))
					.or_else(|| user.map(|user| self.user_display_name(user)))
					.or_else(|| member.map(|member| member.user.name.as_str()))
					.unwrap_or("Unknown user");
				("@", name, len)
			} else if let Some((id, len)) = model::role_mention_prefix(text) {
				let name = guild
					.and_then(|guild| self.guild_roles(guild))
					.and_then(|roles| roles.iter().find(|role| role.id == id))
					.map_or("Unknown role", |role| role.name.as_str());
				("@", name, len)
			} else if let Some((id, len)) = model::channel_mention_prefix(text) {
				let name = self
					.channel(id)
					.filter(|_| self.can_view(id))
					.map_or("Unknown channel", |channel| channel.name.as_str());
				("#", name, len)
			} else {
				let len = text.chars().next().unwrap().len_utf8();
				("", &text[..len], len)
			};
			for character in prefix.chars().chain(name.chars()) {
				if count == 160 || output.len() + character.len_utf8() > 512 {
					return alert_text(&output, 160, 512);
				}
				output.push(character);
				count += 1;
			}
			text = &text[len..];
		}
		alert_text(&output, 160, 512)
	}

	pub(crate) fn observe_notification(&mut self, message: &Message) {
		if !model::valid_mention_roles(&message.mention_roles) {
			return;
		}
		if !self
			.channel(message.channel)
			.is_some_and(|channel| channel.supports_text())
		{
			return;
		}
		let activity = &mut self.read_state.activity;
		let previous = activity.high_water.get(&message.channel).copied();
		if previous.is_some_and(|id| message.id <= id) {
			return;
		}
		activity.high_water.insert(message.channel, message.id);
		if !self.can_view(message.channel) {
			return;
		}
		let Some(owner) = self.user.as_ref() else {
			return;
		};
		if message.author.id == owner.id {
			// The unofficial service also implicitly acknowledges our own gateway messages.
			let _ = self.apply_read_state(crate::read_state::Event::Ack {
				channel: message.channel,
				message: Some(message.id),
				manual: false,
				mention_count: Some(0),
				version: None,
			});
			return;
		}
		if self
			.read_marker(message.channel)
			.flatten()
			.is_some_and(|read| message.id <= read)
		{
			return;
		}
		let direct = message.mentions.iter().any(|u| u.id == owner.id);
		let mention = self.counts_toward_mention_badge(message);
		// Silent messages still contribute to badges, but never enqueue an OS alert.
		let allowed = !message.suppress_notifications
			&& self.user_content_restriction(message.author.id).is_none()
			&& self.notification_allowed_for(message.channel, mention);
		let mut evicted_mention = false;
		{
			let activity = &mut self.read_state.activity;
			// ponytail: retain 4096 observed messages; counts become a lower bound after eviction.
			while activity.observed.len() >= MAX_OBSERVED
				|| (activity.observed.len() + 1) * size_of::<(Id, Id, bool)>() > MAX_OBSERVED_BYTES
			{
				if let Some((channel, _, mention)) = activity.observed.pop_front()
					&& let Some(counts) = activity.observed_counts.get_mut(&channel)
				{
					counts.0 -= 1;
					counts.1 -= u32::from(mention);
					evicted_mention |= mention;
					if counts.0 == 0 {
						activity.observed_counts.remove(&channel);
					}
				}
			}
			activity
				.observed
				.push_back((message.channel, message.id, mention));
			let counts = activity.observed_counts.entry(message.channel).or_default();
			counts.0 += 1;
			counts.1 += u32::from(mention);
		}
		if evicted_mention {
			self.note_evicted_mention();
		}
		if !allowed {
			return;
		}
		let sender = alert_text(self.message_author_name(message), 80, 256);
		let sender = if sender.is_empty() {
			"Unknown sender".to_owned()
		} else {
			sender
		};
		let display = message.display_text();
		let preview = if display.trim().is_empty() {
			if !message.attachments.is_empty() {
				"Sent an attachment".to_owned()
			} else if !message.embeds.is_empty() {
				"Sent an embed".to_owned()
			} else {
				"Sent a message".to_owned()
			}
		} else {
			let text = self.alert_preview(message, &display);
			if text.is_empty() {
				"Sent a message".to_owned()
			} else {
				text
			}
		};
		let notification = Notification {
			channel: message.channel,
			message: message.id,
			author: message.author.id,
			sender,
			preview,
			avatar_key: message.author.avatar_key(),
			direct,
			everyone: message.mention_everyone,
			roles: message.mention_roles.clone(),
		};
		let activity = &mut self.read_state.activity;
		while activity.notifications.len() >= MAX_NOTIFICATIONS
			|| activity
                .notifications
                .iter()
                .map(Notification::bytes)
                .sum::<usize>()
                + notification.bytes()
                // Include unused deque slots; capacity never exceeds the 32-item ceiling.
                + (MAX_NOTIFICATIONS - activity.notifications.len() - 1) * size_of::<Notification>()
				> MAX_NOTIFICATION_BYTES
		{
			activity.notifications.pop_front();
		}
		activity.notifications.push_back(notification);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use model::{Channel, Guild, Patch, User, permissions as p};

	#[test]
	fn mentioned_text_traffic_reuses_rows_but_thread_activity_and_own_ack_refresh() {
		let mut state = notification_state();
		let apply = |state: &mut State, message| {
			state.apply(crate::Envelope {
				generation: state.generation,
				event: crate::Event::Message(message),
			});
		};
		apply(&mut state, message(100, 20));
		assert_eq!(state.mention_count(Id(20)), 1);
		let initial = state.channel_list_revision();
		for id in 101..111 {
			let mut incoming = message(id, 20);
			incoming.mentions.clear();
			apply(&mut state, incoming);
			assert_eq!(state.mention_count(Id(20)), 1);
			assert_eq!(
				state.channel_list_revision(),
				initial,
				"ordinary messages do not change a mentioned text channel's row or count"
			);
		}
		let mut own = message(111, 20);
		own.author = state.user.clone().unwrap();
		own.mentions.clear();
		apply(&mut state, own);
		assert_eq!(state.mention_count(Id(20)), 0);
		assert_ne!(
			state.channel_list_revision(),
			initial,
			"own-message ACK retires the badge"
		);
		let mut post = state.channels[0].clone();
		post.id = Id(30);
		post.kind = 11;
		post.parent_id = Some(Id(20));
		state.channels.push(post);
		state.invalidate_navigation();
		apply(&mut state, message(200, 30));
		let before_activity = state.channel_list_revision();
		let mut reply = message(201, 30);
		reply.mentions.clear();
		apply(&mut state, reply);
		assert_eq!(state.mention_count(Id(30)), 1);
		assert_ne!(
			state.channel_list_revision(),
			before_activity,
			"activity in a pinged thread refreshes its position among other pinged threads"
		);
	}

	#[test]
	fn observed_mention_eviction_invalidates_its_other_channel_sidebar_rollup() {
		let mut state = notification_state();
		let mut post = state.channels[0].clone();
		post.id = Id(30);
		post.kind = 11;
		post.parent_id = Some(Id(20));
		state.channels.push(post);
		state.invalidate_navigation();
		let apply = |state: &mut State, message| {
			state.apply(crate::Envelope {
				generation: state.generation,
				event: crate::Event::Message(message),
			});
		};
		let mut ping = message(100, 30);
		ping.suppress_notifications = true;
		apply(&mut state, ping);
		assert_eq!(state.mention_count(Id(30)), 1);
		let initial = state.channel_list_revision();
		let ordinary = |id| {
			let mut incoming = message(id, 20);
			incoming.mentions.clear();
			incoming.suppress_notifications = true;
			incoming
		};
		for index in 0..MAX_OBSERVED - 1 {
			apply(&mut state, ordinary(101 + index as u64));
			assert_eq!(
				state.channel_list_revision(),
				initial,
				"ordinary non-evicting guild traffic reuses sidebar rows"
			);
		}
		assert_eq!(state.channel_mention_counts().get(&Id(20)), Some(&1));
		let catalog = state.catalog_revision();
		let labels = state.channel_labels_revision();
		apply(&mut state, ordinary(101 + MAX_OBSERVED as u64));
		assert_eq!(state.mention_count(Id(30)), 0);
		assert!(state.channel_mention_counts().is_empty());
		assert_ne!(
			state.channel_list_revision(),
			initial,
			"evicting another conversation's mention retires its revealed row and parent badge"
		);
		assert_eq!(state.catalog_revision(), catalog);
		assert_eq!(state.channel_labels_revision(), labels);
		let after_eviction = state.channel_list_revision();
		apply(&mut state, ordinary(102 + MAX_OBSERVED as u64));
		assert_eq!(
			state.channel_list_revision(),
			after_eviction,
			"evicting an ordinary observation does not rebuild navigation"
		);
	}

	#[test]
	fn sidebar_mentions_roll_up_threads_without_changing_direct_counts_or_parent_ack() {
		let mut state = notification_state();
		state.channels[0].kind = 15;
		let mut post = state.channels[0].clone();
		post.id = Id(30);
		post.kind = 11;
		post.parent_id = Some(Id(20));
		state.channels.push(post);
		state.invalidate_navigation();
		state.read_state.activity.set_count(Id(20), 2);
		state.read_state.activity.set_count(Id(30), 3);
		let counts = state.channel_mention_counts();
		assert_eq!(counts.get(&Id(20)), Some(&5));
		assert_eq!(counts.get(&Id(30)), Some(&3));
		assert_eq!(
			state.mention_count(Id(20)) + state.mention_count(Id(30)),
			5,
			"server totals use each conversation once"
		);
		state
			.apply_read_state(crate::read_state::Event::Ack {
				channel: Id(20),
				message: Some(Id(100)),
				manual: false,
				mention_count: Some(0),
				version: None,
			})
			.unwrap();
		assert_eq!(state.mention_count(Id(30)), 3);
		assert_eq!(state.channel_mention_counts().get(&Id(20)), Some(&3));
		state.channels.retain(|channel| channel.id != Id(30));
		state.invalidate_navigation();
		assert!(
			state.channel_mention_counts().is_empty(),
			"removed metadata cannot leave a parent badge"
		);
	}
	fn notification_state() -> State {
		let owner = User {
			username: None,
			primary_guild: None,
			cosmetics: None,
			id: Id(2),
			name: "Synthetic".into(),
			avatar: None,
			webhook: false,
			kind: Default::default(),
			discriminator: 0,
		};
		let mut state = State {
			user: Some(owner.clone()),
			gateway_connected: true,
			auth: crate::auth::AuthState::Authenticated,
			guilds: vec![Guild {
				default_message_notifications: None,
				stickers: None,
				id: Id(1),
				name: "Synthetic".into(),
				icon: None,
				emojis: None,
			}],
			channels: [20, 21]
				.into_iter()
				.map(|id| Channel {
					id: Id(id),
					guild: Some(Id(1)),
					kind: 0,
					name: "Synthetic".into(),
					parent_id: None,
					last_message: Some(Id(95)),
					position: 0,
					recipients: vec![],
					icon: None,
					member_list_id: None,
					tags: None,
					message_count: None,
				})
				.collect(),
			..State::default()
		};
		state
			.permissions
			.replace(p::Snapshot {
				guilds: vec![p::Guild {
					id: Id(1),
					owner: Some(Id(999)),
					roles: Some(vec![
						p::Role {
							icon: None,
							unicode_emoji: None,
							id: Id(1),
							bits: p::VIEW_CHANNEL,
							name: String::new(),
							color: 0,
							secondary_color: None,
							tertiary_color: None,
							position: 0,
							hoist: false,
						},
						p::Role {
							icon: None,
							unicode_emoji: None,
							id: Id(10),
							bits: 0,
							name: String::new(),
							color: 0,
							secondary_color: None,
							tertiary_color: None,
							position: 0,
							hoist: false,
						},
						p::Role {
							icon: None,
							unicode_emoji: None,
							id: Id(11),
							bits: 0,
							name: String::new(),
							color: 0,
							secondary_color: None,
							tertiary_color: None,
							position: 0,
							hoist: false,
						},
					]),
					member: Some(p::Member {
						roles: vec![],
						timeout_until: None,
					}),
				}],
				channels: [20, 21]
					.into_iter()
					.map(|id| p::Channel {
						id: Id(id),
						guild: Id(1),
						overwrites: Some(vec![]),
					})
					.collect(),
			})
			.unwrap();
		state
			.apply_read_state(crate::read_state::Event::Snapshot {
				entries: Some(vec![(Id(20), Some(Id(90)), 0), (Id(21), Some(Id(90)), 0)]),
				version: Some(1),
				partial: false,
			})
			.unwrap();
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![Setting {
					overrides_known: true,
					guild: Some(Id(1)),
					mute_until: None,
					muted: Some(false),
					level: Some(0),
					suppress_everyone: Some(false),
					suppress_roles: Some(false),
					hide_muted_channels: None,
					channel_mute_until: vec![],
					channels: vec![],
				}],
				replace: true,
			})
			.unwrap();
		state
			.apply_notification_preferences(Event::Presence(Some(false)))
			.unwrap();
		state
	}
	fn category_fixture() -> State {
		let mut state = notification_state();
		state.channels[0].parent_id = Some(Id(30));
		state.channels[1].parent_id = Some(Id(30));
		let mut category = state.channels[0].clone();
		category.id = Id(30);
		category.kind = 4;
		category.parent_id = None;
		category.last_message = None;
		let mut thread = state.channels[0].clone();
		thread.id = Id(40);
		thread.kind = 11;
		thread.parent_id = Some(Id(20));
		state.channels.extend([category, thread]);
		for id in [30, 40] {
			state.permissions.channels.insert(
				Id(id),
				p::Channel {
					id: Id(id),
					guild: Id(1),
					overwrites: Some(vec![]),
				},
			);
		}
		for role in state
			.permissions
			.guilds
			.get_mut(&Id(1))
			.unwrap()
			.roles
			.iter_mut()
			.flatten()
		{
			role.bits |= p::READ_MESSAGE_HISTORY;
		}
		state.permissions = state.permissions.clone();
		state.invalidate_navigation();
		state
			.apply_read_state(crate::read_state::Event::Snapshot {
				entries: Some(vec![
					(Id(20), Some(Id(90)), 0),
					(Id(21), Some(Id(90)), 0),
					(Id(40), Some(Id(90)), 0),
				]),
				version: Some(2),
				partial: false,
			})
			.unwrap();
		state
	}
	#[test]
	fn category_mute_and_notification_overrides_reach_threads_without_admin_access() {
		let mut state = category_fixture();
		assert!(!state.can_manage_channel(Id(30)));
		assert!(state.notification_allowed(Id(40)));
		state
			.confirm_channel_preferences(Id(1), Id(30), Some(false), Some(2))
			.unwrap();
		assert!(
			!state.notification_allowed(Id(40)),
			"thread inherits the category's notification level"
		);
		state
			.confirm_channel_preferences(Id(1), Id(20), Some(false), Some(0))
			.unwrap();
		assert!(
			state.notification_allowed(Id(40)),
			"explicit child level overrides category default"
		);
		state
			.confirm_channel_preferences(Id(1), Id(30), Some(true), Some(2))
			.unwrap();
		for id in [20, 21, 40] {
			assert_eq!(state.guild_channel_muted(Id(id)), Some(false));
			assert_eq!(state.guild_channel_effectively_muted(Id(id)), Some(true));
			assert!(state.channel_access(Id(id)).muted());
			assert!(!state.notification_allowed(Id(id)));
		}
		state
			.confirm_channel_mute_timer(Id(30), Some(true), Some(State::permission_time() - 1))
			.unwrap();
		assert_eq!(state.guild_channel_effectively_muted(Id(40)), Some(false));
		assert!(state.notification_allowed(Id(40)));
		assert_eq!(
			state.channel_notification_level(Id(20)),
			Some(0),
			"expiry preserves child overrides"
		);
		state
			.channels
			.iter_mut()
			.find(|channel| channel.id == Id(30))
			.unwrap()
			.parent_id = Some(Id(40));
		state.invalidate_navigation();
		assert!(state.notification_path(Id(40)).is_none());
		assert!(
			!state.notification_allowed(Id(40)),
			"cycles are bounded and fail closed"
		);
		state
			.channels
			.iter_mut()
			.find(|channel| channel.id == Id(30))
			.unwrap()
			.parent_id = None;
		state
			.channels
			.iter_mut()
			.find(|channel| channel.id == Id(30))
			.unwrap()
			.guild = Some(Id(99));
		state.invalidate_navigation();
		assert!(
			!state.notification_allowed(Id(40)),
			"settings never cross guild boundaries"
		);
	}
	#[test]
	fn scoped_category_and_folder_read_actions_preserve_newer_marks_and_other_conversations() {
		use crate::{Command, read_state::Event as Read};
		let mut state = category_fixture();
		let mut outside = state.channels[0].clone();
		outside.id = Id(22);
		outside.parent_id = None;
		state.channels.push(outside);
		state.permissions.channels.insert(
			Id(22),
			p::Channel {
				id: Id(22),
				guild: Id(1),
				overwrites: Some(vec![]),
			},
		);
		state.invalidate_navigation();
		state
			.apply_read_state(Read::Snapshot {
				entries: Some(vec![
					(Id(20), Some(Id(90)), 0),
					(Id(21), Some(Id(90)), 0),
					(Id(22), Some(Id(90)), 0),
					(Id(40), Some(Id(90)), 0),
				]),
				version: Some(3),
				partial: false,
			})
			.unwrap();
		state.drafts.insert(Id(20), "Keep this draft".into());
		let selected = state.selected;
		assert!(state.can_mark_channel_read(Id(30)));
		let Command::MarkAllRead { request, channels } =
			state.prepare_mark_channel_read(Id(30)).unwrap()
		else {
			panic!("category uses bounded bulk acknowledgement")
		};
		assert_eq!(
			channels,
			[(Id(20), Id(95)), (Id(21), Id(95)), (Id(40), Id(95))]
		);
		state
			.apply_read_state(Read::Ack {
				channel: Id(20),
				message: Some(Id(80)),
				manual: true,
				mention_count: None,
				version: Some(4),
			})
			.unwrap();
		state
			.apply_read_state(Read::AllAck {
				request,
				acked: channels.len(),
				result: Ok(()),
			})
			.unwrap();
		assert_eq!(
			state.read_marker(Id(20)),
			Some(Some(Id(80))),
			"newer cross-client mark wins"
		);
		assert_eq!(state.unread(Id(21)), Some(false));
		assert_eq!(state.unread(Id(40)), Some(false));
		assert_eq!(state.unread(Id(22)), Some(true));
		assert_eq!(state.selected, selected);
		assert_eq!(state.drafts[&Id(20)], "Keep this draft");
		let Command::MarkAllRead { request, channels } =
			state.prepare_mark_guilds_read(&[Id(1)]).unwrap()
		else {
			panic!()
		};
		assert_eq!(channels, [(Id(20), Id(95)), (Id(22), Id(95))]);
		state
			.apply_read_state(Read::AllAck {
				request,
				acked: 1,
				result: Err(crate::auth::Failure::RateLimited),
			})
			.unwrap();
		assert_eq!(state.unread(Id(20)), Some(false));
		assert_eq!(state.unread(Id(22)), Some(true));
		assert!(state.continue_mark_all_read().is_none());
		assert!(state.prepare_mark_guilds_read(&[Id(0)]).is_none());
		state.gateway_connected = false;
		assert!(state.prepare_mark_channel_read(Id(30)).is_none());
	}
	#[test]
	fn deleting_a_channel_during_a_partial_bulk_ack_cannot_acknowledge_its_successor() {
		use crate::{Command, read_state::Event as Read};
		let mut state = category_fixture();
		let Command::MarkAllRead { request, .. } = state.prepare_mark_channel_read(Id(30)).unwrap()
		else {
			panic!()
		};
		state.remove_channels(&std::collections::BTreeSet::from([Id(20)]));
		state
			.apply_read_state(Read::AllAck {
				request,
				acked: 1,
				result: Err(crate::auth::Failure::RateLimited),
			})
			.unwrap();
		assert_eq!(
			state.unread(Id(21)),
			Some(true),
			"the confirmed prefix referred to the deleted channel"
		);
		assert_eq!(state.read_marker(Id(20)), None);
		assert!(state.continue_mark_all_read().is_none());
	}
	#[test]
	fn category_read_continuation_keeps_its_scope_after_a_full_batch() {
		use crate::{Command, read_state::Event as Read};
		let mut state = category_fixture();
		for id in 1000..1000 + crate::MAX_BULK_ACK as u64 {
			let mut channel = state.channels[0].clone();
			channel.id = Id(id);
			state.channels.push(channel);
			state.permissions.channels.insert(
				Id(id),
				p::Channel {
					id: Id(id),
					guild: Id(1),
					overwrites: Some(vec![]),
				},
			);
		}
		let mut outside = state.channels[0].clone();
		outside.id = Id(99);
		outside.parent_id = None;
		state.channels.push(outside);
		state.permissions.channels.insert(
			Id(99),
			p::Channel {
				id: Id(99),
				guild: Id(1),
				overwrites: Some(vec![]),
			},
		);
		state.invalidate_navigation();
		state
			.apply_read_state(Read::Snapshot {
				entries: Some(
					state
						.channels
						.iter()
						.filter(|c| c.kind != 4)
						.map(|c| (c.id, Some(Id(90)), 0))
						.collect(),
				),
				version: Some(3),
				partial: false,
			})
			.unwrap();
		let Command::MarkAllRead { request, channels } =
			state.prepare_mark_channel_read(Id(30)).unwrap()
		else {
			panic!()
		};
		assert_eq!(channels.len(), crate::MAX_BULK_ACK);
		assert!(!channels.iter().any(|(id, _)| *id == Id(99)));
		state
			.apply_read_state(Read::AllAck {
				request,
				acked: channels.len(),
				result: Ok(()),
			})
			.unwrap();
		let Command::MarkAllRead { request, channels } = state.continue_mark_all_read().unwrap()
		else {
			panic!()
		};
		assert_eq!(channels.len(), 3);
		assert!(!channels.iter().any(|(id, _)| *id == Id(99)));
		state
			.apply_read_state(Read::AllAck {
				request,
				acked: channels.len(),
				result: Ok(()),
			})
			.unwrap();
		assert!(state.continue_mark_all_read().is_none());
		assert_eq!(state.unread(Id(99)), Some(true));
	}
	#[test]
	fn timed_mute_deadlines_select_the_earliest_future_expiry() {
		use std::time::{Duration, UNIX_EPOCH};
		let mut state = notification_state();
		let now = State::permission_time();
		let setting = state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap();
		setting.muted = Some(true);
		setting.mute_until = Some(now + 3600);
		setting.channels = vec![(Id(20), Some(true), None), (Id(21), Some(true), None)];
		setting.channel_mute_until = vec![(Id(20), now + 900), (Id(21), now - 1)];
		assert_eq!(
			state.next_mute_deadline(),
			UNIX_EPOCH.checked_add(Duration::from_secs((now + 900) as u64))
		);
		assert_eq!(state.guild_channel_muted(Id(20)), Some(true));
		assert_eq!(state.guild_channel_muted(Id(21)), Some(false));
		let setting = state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap();
		setting.channel_mute_until.clear();
		assert_eq!(
			state.next_mute_deadline(),
			UNIX_EPOCH.checked_add(Duration::from_secs((now + 3600) as u64))
		);
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.mute_until = Some(now - 1);
		assert!(state.next_mute_deadline().is_none());
		assert_eq!(state.server_notification_settings(Id(1)).muted, Some(false));
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.mute_until = None;
		assert!(state.next_mute_deadline().is_none());
		assert_eq!(state.server_notification_settings(Id(1)).muted, Some(true));
	}

	#[test]
	fn far_future_mutes_keep_their_value_and_a_recheck_on_every_platform() {
		use std::time::{Duration, SystemTime, UNIX_EPOCH};
		for (guild, whole_guild) in [(None, false), (Some(Id(1)), false), (Some(Id(1)), true)] {
			let mut state = notification_state();
			if guild.is_none() {
				state.channels[0].guild = None;
				state.channels[0].kind = 1;
			}
			let setting = state
				.notification_preferences
				.settings
				.entry(guild)
				.or_default();
			if whole_guild {
				setting.muted = Some(true);
				setting.mute_until = Some(i64::MAX);
			} else {
				setting.channels = vec![(Id(20), Some(true), None)];
				setting.channel_mute_until = vec![(Id(20), i64::MAX)];
			}
			let before = SystemTime::now();
			let deadline = state
				.next_mute_deadline()
				.expect("future mute needs a recheck");
			let after = SystemTime::now();
			if let Some(expected) = UNIX_EPOCH.checked_add(Duration::from_secs(i64::MAX as u64)) {
				assert_eq!(deadline, expected);
			} else {
				// Exercise the actual Windows SystemTime limit, preserving a bounded wakeup.
				let recheck = Duration::from_secs(24 * 60 * 60);
				assert!(deadline >= before.checked_add(recheck).unwrap());
				assert!(deadline <= after.checked_add(recheck).unwrap());
			}
			assert!(
				state
					.next_mute_expiry()
					.is_some_and(|delay| !delay.is_zero())
			);
			let setting = &state.notification_preferences.settings[&guild];
			if whole_guild {
				assert_eq!(setting.mute_until, Some(i64::MAX));
				assert_eq!(state.server_notification_settings(Id(1)).muted, Some(true));
			} else {
				assert_eq!(setting.channel_mute_until, vec![(Id(20), i64::MAX)]);
				assert_eq!(
					if guild.is_some() {
						state.guild_channel_muted(Id(20))
					} else {
						state.dm_muted(Id(20))
					},
					Some(true)
				);
			}
		}
	}

	#[test]
	fn timed_dm_mute_confirms_expiry_and_rejects_old_account_results() {
		use crate::{
			Command,
			user_actions::{Action, Event as UserEvent},
		};
		let mut state = notification_state();
		state.channels[0].guild = None;
		state.channels[0].kind = 1;
		state.channels[0].recipients = vec![message(100, 20).author];
		state.invalidate_navigation();
		assert!(
			state
				.set_dm_mute(Id(20), crate::channel_actions::Mute::For(1))
				.is_none()
		);
		let Command::UserAction {
			action: Action::MuteFor {
				channel,
				seconds,
				until,
			},
			request,
			..
		} = state
			.set_dm_mute(Id(20), crate::channel_actions::Mute::For(900))
			.unwrap()
		else {
			panic!()
		};
		assert_eq!(seconds, 900);
		assert!(until > State::permission_time());
		assert_eq!(state.dm_muted(Id(20)), Some(true));
		let action = Action::MuteFor {
			channel,
			seconds,
			until,
		};
		state
			.apply_user_action(UserEvent::Written {
				action: action.clone(),
				request,
				result: Ok(()),
			})
			.unwrap();
		assert_eq!(
			state.notification_preferences.settings[&None].channel_mute_until,
			[(Id(20), until)]
		);
		state
			.confirm_dm_timed_mute(Id(20), State::permission_time() - 1)
			.unwrap();
		assert_eq!(state.dm_muted(Id(20)), Some(false));
		state.confirm_dm_muted(Id(20), true).unwrap();
		assert!(
			state.notification_preferences.settings[&None]
				.channel_mute_until
				.is_empty()
		);
		assert_eq!(state.dm_muted(Id(20)), Some(true));
		let generation = state.generation;
		state.logout();
		state.apply(crate::Envelope {
			generation,
			event: crate::Event::UserAction(UserEvent::Written {
				action,
				request,
				result: Ok(()),
			}),
		});
		assert!(state.notification_preferences.settings.is_empty());
	}
	#[test]
	fn unread_navigation_skips_muted_channels_but_keeps_mentions_and_preserves_drafts() {
		let mut state = category_fixture();
		state.selected = Some(Id(20));
		state.drafts.insert(Id(20), "Keep draft".into());
		assert_eq!(state.next_unread_channel(false), Some(Id(21)));
		assert_eq!(state.next_unread_channel(true), Some(Id(40)));
		state
			.confirm_channel_preferences(Id(1), Id(30), Some(true), Some(3))
			.unwrap();
		assert!(state.next_unread_channel(false).is_none());
		state
			.apply_read_state(crate::read_state::Event::Ack {
				channel: Id(40),
				message: Some(Id(90)),
				manual: true,
				mention_count: Some(1),
				version: Some(3),
			})
			.unwrap();
		assert_eq!(state.next_unread_channel(false), Some(Id(40)));
		assert_eq!(state.selected, Some(Id(20)));
		assert_eq!(state.drafts[&Id(20)], "Keep draft");
	}
	#[test]
	fn restrictions_recheck_queued_authors_and_do_not_silence_other_group_members() {
		for ignored in [false, true] {
			let mut state = notification_state();
			let incoming = message(100, 20);
			state.observe_notification(&incoming);
			if ignored {
				state
					.apply_user_action(crate::user_actions::Event::Restriction {
						user: incoming.author.id,
						ignored: Some(true),
						profile: Some((incoming.author.clone(), "synthetic".into())),
					})
					.unwrap();
			} else {
				state
					.apply_user_action(crate::user_actions::Event::Relationship {
						user: incoming.author.id,
						blocked: true,
					})
					.unwrap();
			}
			assert!(
				state.take_notification().is_none(),
				"queued guild previews must be discarded after a relationship change"
			);
			assert!(!state.counts_toward_mention_badge(&incoming));
			state.channels[0].guild = None;
			state.channels[0].kind = 3;
			state.channels[0].recipients = vec![incoming.author.clone()];
			let mut other = message(101, 20);
			other.author.id = Id(4);
			state.observe_notification(&other);
			assert!(
				state.take_notification().is_some(),
				"one restricted recipient must not mute an entire group"
			);
			let mut hidden = incoming;
			hidden.id = Id(102);
			state.observe_notification(&hidden);
			assert!(state.take_notification().is_none());
		}
	}
	#[test]
	fn confirmed_dm_mute_and_block_suppress_notifications() {
		{
			let mut state = notification_state();
			let mut incoming = message(100, 20);
			incoming.author.id = Id(3);
			state.channels[0].guild = None;
			state.channels[0].kind = 1;
			state.channels[0].recipients = vec![incoming.author.clone()];
			state
				.apply_notification_preferences(Event::Settings {
					entries: vec![Setting {
						muted: Some(false),
						level: Some(0),
						..Default::default()
					}],
					replace: false,
				})
				.unwrap();
			state.observe_notification(&incoming);
			assert!(state.take_notification().is_some());
			let mute = state.set_dm_muted(Id(20), true).unwrap();
			assert_eq!(state.dm_muted(Id(20)), Some(true));
			assert!(!state.notification_allowed(Id(20)));
			state.command_rejected(mute);
			assert_eq!(state.dm_muted(Id(20)), Some(false));
			assert!(state.notification_allowed(Id(20)));
			state.confirm_dm_muted(Id(20), true).unwrap();
			let unmute = state.set_dm_muted(Id(20), false).unwrap();
			assert_eq!(state.dm_muted(Id(20)), Some(false));
			assert!(state.notification_allowed(Id(20)));
			state.command_rejected(unmute);
			assert!(!state.notification_allowed(Id(20)));
			incoming.id = Id(101);
			state.observe_notification(&incoming);
			assert!(state.take_notification().is_none());
			state.confirm_dm_muted(Id(20), false).unwrap();
			incoming.id = Id(102);
			state.observe_notification(&incoming);
			assert!(state.take_notification().is_some());
			state
				.apply_user_action(crate::user_actions::Event::Relationship {
					user: Id(3),
					blocked: true,
				})
				.unwrap();
			incoming.id = Id(103);
			state.observe_notification(&incoming);
			assert!(state.take_notification().is_none());
		}
		{
			let mut state = notification_state();
			let mut incoming = message(100, 20);
			incoming.author.name = "A sender\nname".into();
			incoming.author.avatar = Some("0123456789abcdef0123456789abcdef".into());
			incoming.content = "Hello\nfrom the call 🌍".repeat(100);
			state.observe_notification(&incoming);
			let alert = state.take_notification().expect("admitted incoming alert");
			assert_eq!(alert.sender, "A sender name");
			assert!(alert.preview.starts_with("Hello from the call 🌍"));
			assert!(alert.preview.len() <= 512);
			assert_eq!(alert.avatar_key, incoming.author.avatar_key());
			assert!(alert.bytes() <= MAX_NOTIFICATION_BYTES);
		}
	}
	#[test]
	fn guild_defaults_controls_timers_and_unknown_preferences() {
		let mut state = notification_state();
		state.guilds[0].default_message_notifications = Some(0);
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.level = Some(3);
		assert!(state.notification_allowed_for(Id(20), false));
		state.guilds[0].default_message_notifications = Some(1);
		assert!(!state.notification_allowed_for(Id(20), false));
		assert!(state.notification_allowed_for(Id(20), true));
		state.guilds[0].default_message_notifications = None;
		assert!(!state.notification_allowed_for(Id(20), true));
		state
			.confirm_server_notifications(
				Id(1),
				crate::server_actions::NotificationOptions {
					level: Some(0),
					..Default::default()
				},
			)
			.unwrap();
		assert!(state.notification_allowed_for(Id(20), false));
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.channels = vec![(Id(20), Some(false), Some(2))];
		assert!(!state.notification_allowed_for(Id(20), true));
		state
			.confirm_server_notifications(
				Id(1),
				crate::server_actions::NotificationOptions {
					suppress_roles: Some(true),
					..Default::default()
				},
			)
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(20)), Some(2));
		assert_eq!(
			state.server_notification_settings(Id(1)).suppress_roles,
			Some(true)
		);
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.channels
			.clear();
		state
			.confirm_server_notifications(
				Id(1),
				crate::server_actions::NotificationOptions {
					muted: Some(true),
					..Default::default()
				},
			)
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
		state
			.notification_preferences
			.settings
			.get_mut(&Some(Id(1)))
			.unwrap()
			.mute_until = Some(1);
		assert_eq!(state.server_notification_settings(Id(1)).muted, Some(false));
		assert!(state.notification_allowed_for(Id(20), false));
		assert!(!state.muted_for_guild_rail(&state.channels[0]));
		state
			.confirm_server_notifications(
				Id(1),
				crate::server_actions::NotificationOptions {
					muted: Some(true),
					..Default::default()
				},
			)
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
		assert_eq!(
			state.notification_preferences.settings[&Some(Id(1))].mute_until,
			None
		);
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![],
				replace: true,
			})
			.unwrap();
		state.guilds[0].default_message_notifications = Some(0);
		assert_eq!(state.server_notification_settings(Id(1)).level, Some(3));
		assert!(state.notification_allowed_for(Id(20), false));
		state
			.apply_notification_preferences(Event::Presence(Some(true)))
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
		state
			.apply_notification_preferences(Event::Presence(Some(false)))
			.unwrap();
		state
			.apply_notification_preferences(Event::Invalidate)
			.unwrap();
		state
			.apply_notification_preferences(Event::Presence(Some(false)))
			.unwrap();
		assert_eq!(state.server_notification_settings(Id(1)).level, None);
		assert!(!state.notification_allowed_for(Id(20), false));
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![],
				replace: false,
			})
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
	}

	#[test]
	fn inherited_dm_preferences_keep_default_delivery_and_mute() {
		let mut state = notification_state();
		state.channels[0].guild = None;
		state.channels[0].kind = 1;
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![Setting {
					muted: Some(false),
					level: Some(3),
					..Default::default()
				}],
				replace: true,
			})
			.unwrap();
		assert!(state.notification_allowed_for(Id(20), false));
		state.confirm_dm_muted(Id(20), true).unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
	}

	#[test]
	fn existing_channel_controls_preserve_complete_server_defaults() {
		for hide in [false, true] {
			let mut state = notification_state();
			state.guilds[0].default_message_notifications = Some(0);
			state
				.apply_notification_preferences(Event::Settings {
					entries: vec![],
					replace: true,
				})
				.unwrap();
			if hide {
				state.confirm_guild_hides_muted(Id(1), true).unwrap();
			} else {
				state
					.confirm_channel_preferences(Id(1), Id(20), Some(false), Some(3))
					.unwrap();
			}
			assert_eq!(state.server_notification_settings(Id(1)).level, Some(3));
			assert!(state.notification_allowed_for(Id(20), false));
		}
	}

	#[test]
	fn sparse_local_writes_cannot_claim_unknown_overrides_are_empty() {
		let mut state = notification_state();
		state
			.apply_notification_preferences(Event::Invalidate)
			.unwrap();
		state
			.apply_notification_preferences(Event::Presence(Some(false)))
			.unwrap();
		state
			.confirm_server_notifications(
				Id(1),
				crate::server_actions::NotificationOptions {
					muted: Some(false),
					level: Some(0),
					..Default::default()
				},
			)
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), false));
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![Setting {
					guild: Some(Id(1)),
					muted: Some(false),
					level: Some(0),
					channels: vec![(Id(20), Some(false), None)],
					..Default::default()
				}],
				replace: false,
			})
			.unwrap();
		assert!(!state.notification_allowed_for(Id(20), true));
		state
			.apply_notification_preferences(Event::Settings {
				entries: vec![Setting {
					guild: Some(Id(1)),
					muted: Some(false),
					level: Some(0),
					..Default::default()
				}],
				replace: false,
			})
			.unwrap();
		state
			.confirm_channel_preferences(Id(1), Id(20), Some(false), None)
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(20)), Some(3));
		assert!(state.notification_allowed_for(Id(20), false));
	}

	fn message(id: u64, channel: u64) -> Message {
		let owner = User {
			username: None,
			primary_guild: None,
			cosmetics: None,
			id: Id(2),
			name: "Synthetic".into(),
			avatar: None,
			webhook: false,
			kind: Default::default(),
			discriminator: 0,
		};
		Message {
			reply_preview: None,
			poll: None,
			sticker_items: Vec::new(),
			kind: 0,
			id: Id(id),
			channel: Id(channel),
			author: User {
				id: Id(3),
				..owner.clone()
			},
			content: "Synthetic".into(),
			mentions: vec![owner.clone()],
			author_nick: None,
			author_roles: vec![],
			mention_roles: vec![],
			mention_everyone: false,
			suppress_notifications: false,
			reactions: Some(vec![]),
			edited: false,
			edited_at: None,
			revision: 0,
			nonce: None,
			reply_to: None,
			reply_deleted: false,
			interaction: None,
			forwarded: false,
			unsupported: false,
			components: vec![],
			application_id: None,
			flags: 0,
			ephemeral: false,
			extra_content: Default::default(),
			embeds: vec![],
			embeds_suppressed: false,
			attachments: vec![],
		}
	}

	#[test]
	fn supplied_group_mentions_respect_membership_suppression_and_count_once() {
		{
			for (direct, everyone, role, suppress_everyone, suppress_roles, expected) in [
				(false, false, true, Some(false), Some(false), true),
				(false, true, false, Some(false), Some(false), true),
				(true, true, true, Some(false), Some(false), true),
				(false, false, true, Some(false), Some(true), false),
				(false, true, false, Some(true), Some(false), false),
				(false, false, true, Some(false), None, false),
				(false, true, false, None, Some(false), false),
				(true, true, true, Some(true), Some(true), true),
			] {
				let mut state = notification_state();
				state
					.permissions
					.guilds
					.get_mut(&Id(1))
					.unwrap()
					.member
					.as_mut()
					.unwrap()
					.roles = vec![Id(10)];
				let setting = state
					.notification_preferences
					.settings
					.get_mut(&Some(Id(1)))
					.unwrap();
				setting.level = Some(1);
				setting.suppress_everyone = suppress_everyone;
				setting.suppress_roles = suppress_roles;
				let mut message = message(100, 20);
				if !direct {
					message.mentions.clear();
				}
				message.mention_everyone = everyone;
				if role {
					message.mention_roles = vec![Id(10), Id(11)];
				}
				state.observe_notification(&message);
				state.observe_notification(&message); // Replays cannot double-count or alert again.
				assert_eq!(state.unread_count(Id(20)), 1);
				assert_eq!(state.mention_count(Id(20)), u32::from(expected));
				assert_eq!(state.take_notification().is_some(), expected);
				assert!(state.take_notification().is_none());
			}
			for member_roles in [None, Some(vec![]), Some(vec![Id(11)])] {
				let mut state = notification_state();
				// Owner/admin permissions never imply membership in every role.
				let guild = state.permissions.guilds.get_mut(&Id(1)).unwrap();
				guild.owner = Some(Id(2));
				guild.member = member_roles.map(|roles| p::Member {
					roles,
					timeout_until: None,
				});
				let mut message = message(100, 20);
				message.mentions.clear();
				message.mention_roles = vec![Id(10)];
				state.observe_notification(&message);
				assert_eq!(state.mention_count(Id(20)), 0);
			}
		}
		{
			for level in [0, 1] {
				let mut state = notification_state();
				state
					.notification_preferences
					.settings
					.get_mut(&Some(Id(1)))
					.unwrap()
					.level = Some(level);
				let mut message = message(100, 20);
				message.suppress_notifications = true;
				state.observe_notification(&message);
				assert_eq!(state.unread_count(Id(20)), 1);
				assert_eq!(state.mention_count(Id(20)), 1);
				assert!(state.take_notification().is_none());
				// Suppressed group pings are still ordinary messages at all-messages level.
				let setting = state
					.notification_preferences
					.settings
					.get_mut(&Some(Id(1)))
					.unwrap();
				setting.suppress_everyone = Some(true);
				message.id = Id(101);
				message.suppress_notifications = false;
				message.mentions.clear();
				message.mention_everyone = true;
				state.observe_notification(&message);
				assert_eq!(state.mention_count(Id(20)), 1);
				assert_eq!(state.take_notification().is_some(), level == 0);
			}
		}
	}
	#[test]
	fn queued_role_alerts_recheck_membership_and_payload_capacity_at_delivery() {
		{
			let mut state = notification_state();
			state
				.notification_preferences
				.settings
				.get_mut(&Some(Id(1)))
				.unwrap()
				.level = Some(1);
			state
				.permissions
				.guilds
				.get_mut(&Id(1))
				.unwrap()
				.member
				.as_mut()
				.unwrap()
				.roles = vec![Id(10), Id(11)];
			let mut message = message(100, 20);
			message.mentions.clear();
			message.mention_roles = vec![Id(10), Id(11)];
			state.observe_notification(&message);
			state
				.permissions
				.update(crate::permissions::Event::Member {
					guild: Id(1),
					roles: Patch::Value(vec![Id(11)]),
					timeout_until: Patch::Absent,
				})
				.unwrap();
			assert!(
				state.take_notification().is_some(),
				"A second matching role still qualifies"
			);
			message.id = Id(101);
			state.observe_notification(&message);
			state
				.permissions
				.update(crate::permissions::Event::Member {
					guild: Id(1),
					roles: Patch::Value(vec![]),
					timeout_until: Patch::Absent,
				})
				.unwrap();
			assert!(state.take_notification().is_none());
			assert_eq!(
				state.mention_count(Id(20)),
				2,
				"Delivery revalidation does not rewrite observed history"
			);
			state
				.notification_preferences
				.settings
				.get_mut(&Some(Id(1)))
				.unwrap()
				.level = Some(0);
			message.mention_roles = (10..110).map(Id).collect();
			for id in 102..202 {
				message.id = Id(id);
				state.observe_notification(&message);
			}
			let queue = &state.read_state.activity.notifications;
			assert!(
				queue.len() < MAX_NOTIFICATIONS,
				"Payload budget evicts before item ceiling"
			);
			assert!(
				queue.iter().map(Notification::bytes).sum::<usize>()
					+ (queue.capacity() - queue.len()) * size_of::<Notification>()
					<= MAX_NOTIFICATION_BYTES
			);
			state
				.apply_notification_preferences(Event::Invalidate)
				.unwrap();
			assert!(state.take_notification().is_none());
		}
		{
			let mut state = notification_state();
			let access = |view| crate::permissions::Event::Channel {
				channel: Id(20),
				guild: Some(Id(1)),
				overwrites: Patch::Value(vec![p::Overwrite {
					id: Id(1),
					kind: 0,
					allow: 0,
					deny: if view { 0 } else { p::VIEW_CHANNEL },
				}]),
			};
			state.observe_notification(&message(100, 20));
			state.observe_notification(&message(101, 21));
			assert!(!state.can_read_history(Id(20)));
			assert_eq!(
				state.unread_count(Id(20)),
				1,
				"Live activity requires VIEW, not history access"
			);
			assert_eq!(state.mention_count(Id(20)), 1);
			assert_eq!(state.unread(Id(20)), Some(true));
			assert_eq!(state.channel_unread(&state.channels[0]), Some(true));
			state.apply(crate::Envelope {
				generation: state.generation,
				event: crate::Event::Permissions(access(false)),
			});
			assert_eq!(state.unread_count(Id(20)), 0);
			assert_eq!(state.mention_count(Id(20)), 0);
			assert_eq!(state.unread(Id(20)), None);
			assert_eq!(state.channel_unread(&state.channels[0]), None);
			assert_eq!(state.unread_count(Id(21)), 1);
			assert_eq!(
				state.read_state.activity.notifications.len(),
				1,
				"Revocation removes queued alerts before permission can return"
			);
			state.observe_notification(&message(102, 20));
			state.apply(crate::Envelope {
				generation: state.generation,
				event: crate::Event::Permissions(access(true)),
			});
			assert_eq!(state.take_notification().unwrap().channel, Id(21));
			state.observe_notification(&message(100, 20));
			state.observe_notification(&message(102, 20));
			assert_eq!(state.unread_count(Id(20)), 0);
			assert!(state.take_notification().is_none());
			state.observe_notification(&message(103, 20));
			assert_eq!(state.mention_count(Id(20)), 1);
			state.permissions.update(access(false)).unwrap();
			assert!(
				state.take_notification().is_none(),
				"Delivery independently checks current VIEW access"
			);
			state.permissions.update(access(true)).unwrap();
			state
				.apply_read_state(crate::read_state::Event::Ack {
					channel: Id(20),
					message: Some(Id(103)),
					manual: false,
					mention_count: Some(0),
					version: None,
				})
				.unwrap();
			state.observe_notification(&message(103, 20));
			assert_eq!(state.mention_count(Id(20)), 0);
			assert!(state.take_notification().is_none());
		}
	}
	#[test]
	fn unknown_deletes_and_empty_recounts_never_admit_channel_state() {
		let mut activity = Activity::default();
		for id in 1..=10_000 {
			activity.delete(Id(id), Id(id));
			activity.recount(Id(id));
		}
		assert!(activity.observed_counts.is_empty());
		assert!(activity.high_water.is_empty());
		assert!(activity.counts.is_empty());
		activity.observed.push_back((Id(1), Id(2), true));
		activity.recount(Id(1));
		assert_eq!(activity.count(Id(1), true), 1);
		activity.delete(Id(1), Id(2));
		assert!(activity.observed_counts.is_empty());
		assert!(activity.observed.is_empty());
	}
	#[test]
	fn marking_unread_badges_guild_mentions_inside_the_new_range() {
		let mut state = notification_state();
		state.freshness = model::Freshness::Fresh;
		state.selected = Some(Id(20));
		state
			.channels
			.iter_mut()
			.find(|channel| channel.id == Id(20))
			.unwrap()
			.last_message = Some(Id(100));
		state
			.apply_read_state(crate::read_state::Event::Ack {
				channel: Id(20),
				message: Some(Id(100)),
				manual: false,
				mention_count: Some(0),
				version: Some(2),
			})
			.unwrap();
		let mut plain = message(96, 20);
		plain.mentions.clear();
		let ping = message(97, 20);
		let mut mine = message(98, 20);
		mine.author.id = Id(2);
		mine.mentions.clear();
		let mut everyone = message(99, 20);
		everyone.mentions.clear();
		everyone.mention_everyone = true;
		let mut later = message(100, 20);
		later.mentions.clear();
		for row in [plain, ping, mine, everyone, later] {
			state.timeline.insert(row, false, false).unwrap();
		}
		assert_eq!(state.mention_count(Id(20)), 0);
		assert_eq!(state.unread(Id(20)), Some(false));
		let crate::Command::MarkRead {
			channel,
			message,
			request,
			manual: true,
			mention_count,
		} = state.prepare_mark_unread(Id(96)).unwrap()
		else {
			panic!("mark unread");
		};
		assert_eq!(message, Id(95));
		assert_eq!(mention_count, Some(2));
		state
			.apply_read_state(crate::read_state::Event::Result {
				channel,
				message,
				request,
				result: Ok(()),
			})
			.unwrap();
		assert_eq!(state.unread(Id(20)), Some(true));
		assert_eq!(state.mention_count(Id(20)), 2);
		state
			.apply_read_state(crate::read_state::Event::Ack {
				channel,
				message: Some(message),
				manual: true,
				mention_count: None,
				version: Some(3),
			})
			.unwrap();
		assert_eq!(state.mention_count(Id(20)), 2);
		assert_eq!(state.unread(Id(20)), Some(true));
	}
}
