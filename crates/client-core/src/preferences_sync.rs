//! One bounded, account-scoped lane for personal guild preferences.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
	channel_actions::{Action, Mute},
};
use model::{
	Id, Patch,
	guild_preferences::{Fields, Row, Update, Write},
};
use std::{
	collections::BTreeMap,
	time::{Duration, Instant},
};

pub const NOT_DISPATCHED: &str =
	"Channel preferences changed or became unavailable before the save was sent";

const DEBOUNCE: Duration = Duration::from_millis(250);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
	Saving,
	Failed(&'static str),
	Unconfirmed,
	Conflict,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
	Flags,
	Collapsed,
	Muted,
	Level,
	Until,
	Everyone,
	Roles,
	HideMuted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
	guild: Id,
	channel: Option<Id>,
	field: Field,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
	Flags(u64),
	Bool(bool),
	Level(u8),
	Until(Option<i64>),
}
#[derive(Clone)]
struct Edit {
	before: Value,
	desired: Value,
	mask: u64,
	sequence: u64,
	due: Instant,
	status: Status,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
	pub generation: u64,
	pub guild: Id,
	pub request: u64,
	pub patch: Write,
}
pub struct Finished {
	pub generation: u64,
	pub guild: Id,
	pub request: u64,
	pub result: Result<Update, Failure>,
}
struct Flight {
	request: Request,
	fields: Vec<(Key, Edit)>,
	refresh: bool,
	observed_fields: u16,
	observed_channels: BTreeMap<Id, u16>,
	all_channels: bool,
	unversioned_observation: bool,
}
#[derive(Default)]
pub struct Queue {
	edits: BTreeMap<Key, Edit>,
	active: Option<Flight>,
	sequence: u64,
	versions: BTreeMap<Option<Id>, u64>,
	bytes: usize,
	pending_refresh: Option<Id>,
	refresh_status: Option<(Id, Status)>,
}
impl Queue {
	pub(super) fn retain_versions(&mut self, keys: &std::collections::BTreeSet<Option<Id>>) {
		self.versions.retain(|key, _| keys.contains(key));
	}
	pub(crate) fn bytes(&self) -> usize {
		self.bytes
			+ self.active.as_ref().map_or(0, |f| {
				f.fields.capacity() * size_of::<(Key, Edit)>()
					+ 2 * f.request.patch.bytes()
					+ f.observed_channels.len() * (size_of::<(Id, u16)>() + 48)
			}) + self.versions.len() * 48
	}
	fn recount(&mut self) {
		self.bytes = self.edits.len() * (size_of::<(Key, Edit)>() + 48);
	}
	fn applying(edit: &Edit) -> bool {
		matches!(edit.status, Status::Saving | Status::Unconfirmed)
	}
	fn overlay(&self, key: Key, base: Option<Value>) -> Option<Value> {
		let Some(edit) = self.edits.get(&key).filter(|edit| Self::applying(edit)) else {
			return base;
		};
		Some(match (base, edit.desired) {
			(Some(Value::Flags(base)), Value::Flags(desired)) => {
				Value::Flags((base & !edit.mask) | (desired & edit.mask))
			}
			_ => edit.desired,
		})
	}
}
fn assign(fields: &mut Fields, field: Field, value: Value) {
	match (field, value) {
		(Field::Flags, Value::Flags(v)) => fields.flags = Patch::Value(v),
		(Field::Collapsed, Value::Bool(v)) => fields.collapsed = Patch::Value(v),
		(Field::Muted, Value::Bool(v)) => fields.muted = Patch::Value(v),
		(Field::Level, Value::Level(v)) => fields.level = Patch::Value(v),
		(Field::Until, Value::Until(v)) => fields.mute_until = v.map_or(Patch::Null, Patch::Value),
		(Field::Everyone, Value::Bool(v)) => fields.suppress_everyone = Patch::Value(v),
		(Field::Roles, Value::Bool(v)) => fields.suppress_roles = Patch::Value(v),
		(Field::HideMuted, Value::Bool(v)) => fields.hide_muted = Patch::Value(v),
		_ => unreachable!("typed preference field"),
	}
}
fn same(a: Value, b: Value, mask: u64) -> bool {
	match (a, b) {
		(Value::Flags(a), Value::Flags(b)) => (a ^ b) & mask == 0,
		_ => a == b,
	}
}
impl State {
	fn confirmed_preference(&self, key: Key) -> Option<Value> {
		let setting = self.notification_preferences.setting(Some(key.guild));
		let demo_default = self.demo && setting.is_none();
		let known = demo_default || setting.is_some_and(|s| s.overrides_known);
		match (key.channel, key.field) {
			(None, Field::Flags) => setting
				.and_then(|s| s.flags)
				.or(demo_default.then_some(0))
				.map(Value::Flags),
			(None, Field::Muted) => setting
				.and_then(|s| s.muted)
				.or(demo_default.then_some(false))
				.map(Value::Bool),
			(None, Field::Level) => setting
				.and_then(|s| s.level)
				.or(demo_default.then_some(3))
				.map(Value::Level),
			(None, Field::Until) => setting
				.map(|s| Value::Until(s.mute_until))
				.or(demo_default.then_some(Value::Until(None))),
			(None, Field::Everyone) => setting
				.and_then(|s| s.suppress_everyone)
				.or(demo_default.then_some(false))
				.map(Value::Bool),
			(None, Field::Roles) => setting
				.and_then(|s| s.suppress_roles)
				.or(demo_default.then_some(false))
				.map(Value::Bool),
			(None, Field::HideMuted) => setting
				.and_then(|s| s.hide_muted_channels)
				.or(demo_default.then_some(false))
				.map(Value::Bool),
			(Some(id), Field::Flags | Field::Collapsed) => {
				let row = setting
					.and_then(|s| s.channel_flags.iter().find(|(channel, ..)| *channel == id));
				match key.field {
					Field::Flags => row
						.and_then(|(_, flags, _)| *flags)
						.or_else(|| (row.is_none() && known).then_some(0))
						.map(Value::Flags),
					_ => row
						.and_then(|(_, _, collapsed)| *collapsed)
						.or_else(|| (row.is_none() && known).then_some(false))
						.map(Value::Bool),
				}
			}
			(Some(id), Field::Muted | Field::Level) => {
				let row =
					setting.and_then(|s| s.channels.iter().find(|(channel, ..)| *channel == id));
				match key.field {
					Field::Muted => row
						.and_then(|(_, muted, _)| *muted)
						.or_else(|| (row.is_none() && known).then_some(false))
						.map(Value::Bool),
					_ => row
						.and_then(|(_, _, level)| *level)
						.or_else(|| (row.is_none() && known).then_some(3))
						.map(Value::Level),
				}
			}
			(Some(id), Field::Until) => known.then(|| {
				Value::Until(setting.and_then(|s| {
					s.channel_mute_until
						.iter()
						.find(|(channel, _)| *channel == id)
						.map(|(_, until)| *until)
				}))
			}),
			_ => None,
		}
	}
	fn preference_value(&self, guild: Id, channel: Option<Id>, field: Field) -> Option<Value> {
		let key = Key {
			guild,
			channel,
			field,
		};
		self.preference_queue
			.overlay(key, self.confirmed_preference(key))
	}
	pub(crate) fn preference_flags(&self, guild: Id, channel: Option<Id>) -> Option<u64> {
		match self.preference_value(guild, channel, Field::Flags)? {
			Value::Flags(v) => Some(v),
			_ => None,
		}
	}
	pub(crate) fn preference_bool(
		&self,
		guild: Id,
		channel: Option<Id>,
		name: &str,
	) -> Option<bool> {
		let field = match name {
			"muted" => Field::Muted,
			"collapsed" => Field::Collapsed,
			"hide_muted" => Field::HideMuted,
			"everyone" => Field::Everyone,
			"roles" => Field::Roles,
			_ => return None,
		};
		match self.preference_value(guild, channel, field)? {
			Value::Bool(v) => Some(v),
			_ => None,
		}
	}
	pub(crate) fn preference_level(&self, guild: Id, channel: Option<Id>) -> Option<u8> {
		match self.preference_value(guild, channel, Field::Level)? {
			Value::Level(v) => Some(v),
			_ => None,
		}
	}
	pub(crate) fn preference_until(&self, guild: Id, channel: Option<Id>) -> Option<i64> {
		match self.preference_value(guild, channel, Field::Until)? {
			Value::Until(v) => v,
			_ => None,
		}
	}
	pub fn channel_preferences_known(&self, guild: Id) -> bool {
		self.preference_flags(guild, None).is_some()
			&& (self
				.notification_preferences
				.setting(Some(guild))
				.is_some_and(|setting| setting.overrides_known)
				|| self.demo && self.notification_preferences.setting(Some(guild)).is_none())
	}
	pub fn channel_preference_status(&self, guild: Id, channel: Option<Id>) -> Option<Status> {
		self.preference_queue
			.edits
			.range(
				Key {
					guild,
					channel,
					field: Field::Flags,
				}..=Key {
					guild,
					channel,
					field: Field::HideMuted,
				},
			)
			.map(|(_, edit)| edit.status)
			.max_by_key(|status| match status {
				Status::Failed(_) => 4,
				Status::Conflict => 3,
				Status::Unconfirmed => 2,
				Status::Saving => 1,
			})
			.or_else(|| {
				(channel.is_none() && self.preference_queue.pending_refresh == Some(guild))
					.then_some(Status::Saving)
			})
			.or_else(|| {
				self.preference_queue
					.active
					.as_ref()
					.filter(|flight| {
						flight.request.guild == guild && flight.refresh && channel.is_none()
					})
					.map(|_| Status::Saving)
			})
			.or_else(|| {
				self.preference_queue
					.refresh_status
					.filter(|(target, _)| *target == guild && channel.is_none())
					.map(|(_, status)| status)
			})
	}
	pub(crate) fn pending_mute_deadline(&self, now: i64) -> Option<i64> {
		self.preference_queue
			.edits
			.iter()
			.filter_map(|(key, edit)| {
				if key.field != Field::Until || !Queue::applying(edit) {
					return None;
				}
				let Value::Until(Some(until)) = edit.desired else {
					return None;
				};
				(until > now && self.preference_bool(key.guild, key.channel, "muted") == Some(true))
					.then_some(until)
			})
			.min()
	}
	pub fn next_channel_preference_deadline(&self) -> Option<Instant> {
		if self.preference_queue.active.is_some() {
			return None;
		}
		if self.preference_queue.pending_refresh.is_some() {
			return Some(Instant::now());
		}
		self.preference_queue
			.edits
			.values()
			.filter(|edit| edit.status == Status::Saving)
			.map(|edit| edit.due)
			.min()
	}
	pub fn flush_channel_preferences(&mut self) {
		let now = Instant::now();
		for edit in self
			.preference_queue
			.edits
			.values_mut()
			.filter(|edit| edit.status == Status::Saving)
		{
			edit.due = now;
		}
	}
	fn queue_preference_fields(
		&mut self,
		guild: Id,
		fields: Vec<(Option<Id>, Field, Value, u64)>,
		now: Instant,
	) -> bool {
		if self.guild(guild).is_none()
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return false;
		}
		if fields.len() > model::guild_preferences::MAX_EDITS {
			return false;
		}
		if fields.iter().any(|(channel, field, _, _)| {
			channel.is_some_and(|id| {
				self.channel(id).is_none_or(|c| c.guild != Some(guild))
					|| !self.can_personally_manage_channel(id)
			}) || self
				.confirmed_preference(Key {
					guild,
					channel: *channel,
					field: *field,
				})
				.is_none()
		}) {
			return false;
		}
		let mut edits = self.preference_queue.edits.clone();
		for (channel, field, value, mask) in fields {
			let key = Key {
				guild,
				channel,
				field,
			};
			let before = self.confirmed_preference(key).expect("known preference");
			self.preference_queue.sequence = self.preference_queue.sequence.wrapping_add(1);
			let sequence = self.preference_queue.sequence;
			let in_flight = self
				.preference_queue
				.active
				.as_ref()
				.is_some_and(|flight| flight.fields.iter().any(|(active, _)| *active == key));
			let old = edits.remove(&key);
			let combined = old.as_ref().map_or(mask, |old| old.mask | mask);
			let desired = match (old.as_ref().map(|old| old.desired), value) {
				(Some(Value::Flags(old)), Value::Flags(value)) => {
					Value::Flags((old & !mask) | (value & mask))
				}
				(_, value) => value,
			};
			if !in_flight && same(before, desired, combined) {
				continue;
			}
			edits.insert(
				key,
				Edit {
					before,
					desired,
					mask: combined,
					sequence,
					due: now + DEBOUNCE,
					status: Status::Saving,
				},
			);
		}
		let bytes = edits.len() * (2 * size_of::<(Key, Edit)>() + size_of::<Row>() + 48)
			+ self.preference_queue.active.as_ref().map_or(0, |f| {
				f.fields.capacity() * size_of::<(Key, Edit)>() + 2 * f.request.patch.bytes()
			});
		if edits.len() > model::guild_preferences::MAX_EDITS
			|| bytes > model::guild_preferences::MAX_PENDING_BYTES
			|| bytes
				+ self
					.notification_preferences
					.settings
					.values()
					.map(|setting| setting.bytes() + 64)
					.sum::<usize>()
				> 32 * 1024 * 1024
		{
			self.status = "Too many unsaved channel preferences";
			return false;
		}
		self.preference_queue.edits = edits;
		self.preference_queue.recount();
		self.revision = self.revision.wrapping_add(1);
		true
	}
	pub(crate) fn queue_channel_preference_action(&mut self, channel: Id, action: &Action) -> bool {
		let Some(guild) = self.channel(channel).and_then(|c| c.guild) else {
			return false;
		};
		let mut fields = Vec::new();
		match action {
			Action::Personal(change) => {
				if !self.personal_change_allowed(guild, change, false) {
					return false;
				}
				if let Some((_, after)) = change.guild_flags {
					fields.push((
						None,
						Field::Flags,
						Value::Flags(after),
						model::channel_visibility::GUILD_MASK,
					));
				}
				for row in &change.channels {
					if let Some((before, after)) = row.flags {
						let mut mask = before ^ after;
						for group in [
							model::channel_visibility::UNREADS_ONLY_MENTIONS
								| model::channel_visibility::UNREADS_ALL_MESSAGES,
							model::channel_visibility::NEW_FORUM_THREADS_OFF
								| model::channel_visibility::NEW_FORUM_THREADS_ON,
						] {
							if mask & group != 0 {
								mask |= group;
							}
						}
						fields.push((Some(row.channel), Field::Flags, Value::Flags(after), mask));
					}
					if let Some((_, after)) = row.collapsed {
						fields.push((Some(row.channel), Field::Collapsed, Value::Bool(after), 0));
					}
				}
			}
			Action::HideMuted(value) => {
				fields.push((None, Field::HideMuted, Value::Bool(*value), 0))
			}
			Action::Mute(value) => {
				if !value.valid() {
					return false;
				}
				fields.push((
					Some(channel),
					Field::Muted,
					Value::Bool(*value != Mute::Unmute),
					0,
				));
				fields.push((
					Some(channel),
					Field::Until,
					Value::Until(value.expires_at(Self::permission_time())),
					0,
				));
			}
			Action::Notifications(value) => {
				if *value > 3 {
					return false;
				}
				fields.push((Some(channel), Field::Level, Value::Level(*value), 0));
			}
			_ => return false,
		}
		self.queue_preference_fields(guild, fields, Instant::now())
	}
	pub(crate) fn queue_server_preference_action(
		&mut self,
		guild: Id,
		options: crate::server_actions::NotificationOptions,
	) -> bool {
		if !options.valid() {
			return false;
		}
		let mut fields = Vec::new();
		for (field, value) in [
			(Field::Muted, options.muted),
			(Field::Everyone, options.suppress_everyone),
			(Field::Roles, options.suppress_roles),
		] {
			if let Some(value) = value {
				fields.push((None, field, Value::Bool(value), 0));
			}
		}
		if let Some(value) = options.level {
			fields.push((None, Field::Level, Value::Level(value), 0));
		}
		if options.muted.is_some() {
			fields.push((None, Field::Until, Value::Until(options.mute_until), 0));
		}
		self.queue_preference_fields(guild, fields, Instant::now())
	}
	pub fn retry_channel_preference(&mut self, guild: Id, channel: Option<Id>) -> bool {
		let values: Vec<_> = self
			.preference_queue
			.edits
			.iter()
			.filter(|(key, edit)| {
				key.guild == guild
					&& key.channel == channel
					&& matches!(edit.status, Status::Failed(_) | Status::Conflict)
			})
			.map(|(key, edit)| (key.channel, key.field, edit.desired, edit.mask))
			.collect();
		!values.is_empty() && self.queue_preference_fields(guild, values, Instant::now())
	}
	pub fn refresh_channel_preferences(&mut self, guild: Id) -> Option<Command> {
		if self.guild(guild).is_none()
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return None;
		}
		if let Some(active) = &self.preference_queue.active {
			if active.request.guild != guild || !active.refresh {
				self.preference_queue.pending_refresh = Some(guild);
			}
			return None;
		}
		self.preference_queue.refresh_status = None;
		self.preference_queue.sequence = self.preference_queue.sequence.wrapping_add(1);
		let request = Request {
			generation: self.generation,
			guild,
			request: self.preference_queue.sequence,
			patch: Write::default(),
		};
		self.preference_queue.active = Some(Flight {
			request: request.clone(),
			fields: Vec::new(),
			refresh: true,
			observed_fields: 0,
			observed_channels: BTreeMap::new(),
			all_channels: false,
			unversioned_observation: false,
		});
		Some(Command::ChannelPreferences(Box::new(request)))
	}
	pub fn poll_channel_preferences(&mut self, now: Instant) -> Option<Command> {
		if self.preference_queue.active.is_some()
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return None;
		}
		if let Some(guild) = self.preference_queue.pending_refresh.take() {
			return self.refresh_channel_preferences(guild);
		}
		let guild = self
			.preference_queue
			.edits
			.iter()
			.filter(|(_, e)| e.status == Status::Saving && e.due <= now)
			.min_by_key(|(_, e)| e.due)
			.map(|(key, _)| key.guild)?;
		let keys: Vec<_> = self
			.preference_queue
			.edits
			.iter()
			.filter(|(key, e)| key.guild == guild && e.status == Status::Saving)
			.map(|(key, _)| *key)
			.collect();
		let mut fields = Vec::new();
		let mut patch = Write::default();
		let mut status_changed = false;
		for key in keys {
			if self.guild(guild).is_none()
				|| key.channel.is_some_and(|id| {
					self.channel(id).is_none_or(|c| c.guild != Some(guild))
						|| !self.can_personally_manage_channel(id)
				}) {
				if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
					edit.status = Status::Failed("Channel access changed");
					status_changed = true;
				}
				continue;
			}
			let edit = self.preference_queue.edits.get(&key).expect("queued field");
			let Some(base) = self.confirmed_preference(key) else {
				if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
					edit.status = Status::Conflict;
					status_changed = true;
				}
				continue;
			};
			let value = match (base, edit.desired) {
				(Value::Flags(base), Value::Flags(desired)) => {
					Value::Flags((base & !edit.mask) | (desired & edit.mask))
				}
				(_, desired) => desired,
			};
			if let Some(channel) = key.channel {
				let index = patch
					.channels
					.iter()
					.position(|row| row.channel == channel)
					.unwrap_or_else(|| {
						patch.channels.push(Row {
							channel,
							fields: Fields::default(),
						});
						patch.channels.len() - 1
					});
				assign(&mut patch.channels[index].fields, key.field, value);
			} else {
				assign(&mut patch.fields, key.field, value);
			}
			let mut edit = edit.clone();
			edit.before = base;
			fields.push((key, edit));
		}
		if status_changed {
			self.revision = self.revision.wrapping_add(1);
		}
		if fields.is_empty() {
			return None;
		}
		patch.channels.shrink_to_fit();
		fields.shrink_to_fit();
		if !patch.valid()
			|| self.preference_queue.bytes()
				+ 2 * patch.bytes()
				+ fields.capacity() * size_of::<(Key, Edit)>()
				> model::guild_preferences::MAX_PENDING_BYTES
		{
			for (key, _) in fields {
				if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
					edit.status = Status::Failed("Channel preference save exceeds safe capacity");
				}
			}
			self.revision = self.revision.wrapping_add(1);
			return None;
		}
		self.preference_queue.sequence = self.preference_queue.sequence.wrapping_add(1);
		let request = Request {
			generation: self.generation,
			guild,
			request: self.preference_queue.sequence,
			patch,
		};
		self.preference_queue.active = Some(Flight {
			request: request.clone(),
			fields,
			refresh: false,
			observed_fields: 0,
			observed_channels: BTreeMap::new(),
			all_channels: false,
			unversioned_observation: false,
		});
		Some(Command::ChannelPreferences(Box::new(request)))
	}
	fn preference_owner_allowed(&self, request: &Request) -> bool {
		request.generation == self.generation
			&& self.guild(request.guild).is_some()
			&& (self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
			&& request.patch.channels.iter().all(|row| {
				self.channel(row.channel)
					.is_some_and(|channel| channel.guild == Some(request.guild))
					&& self.can_personally_manage_channel(row.channel)
			})
	}
	fn flight_conflicts(&self, flight: &Flight) -> bool {
		flight.fields.iter().any(|(key, edit)| {
			self.confirmed_preference(*key).is_none_or(|value| {
				!same(value, edit.before, edit.mask) && !same(value, edit.desired, edit.mask)
			})
		})
	}
	fn flight_confirmed(&self, flight: &Flight) -> bool {
		!flight.fields.is_empty()
			&& flight.fields.iter().all(|(key, edit)| {
				self.confirmed_preference(*key)
					.is_some_and(|value| same(value, edit.desired, edit.mask))
			})
	}
	fn flight_flags_changed(&self, flight: &Flight) -> bool {
		flight.fields.iter().any(|(key, edit)| {
			match (self.confirmed_preference(*key), edit.before) {
				(Some(Value::Flags(now)), Value::Flags(before)) => (now ^ before) & !edit.mask != 0,
				_ => false,
			}
		})
	}
	/// Scope for a transport request which may still be waiting for the shared mutation lane.
	pub fn channel_preference_scope(&self) -> Option<(u64, Id, u64)> {
		let flight = self.preference_queue.active.as_ref()?;
		let request = &flight.request;
		(self.preference_owner_allowed(request)
			&& !self.flight_conflicts(flight)
			&& !self.flight_confirmed(flight)
			&& !self.flight_flags_changed(flight))
		.then_some((request.generation, request.guild, request.request))
	}

	pub fn channel_preference_request_allowed(&self, request: &Request) -> bool {
		request.generation == self.generation
			&& request.patch.valid()
			&& self.guild(request.guild).is_some()
			&& self
				.preference_queue
				.active
				.as_ref()
				.is_some_and(|flight| flight.request == *request)
			&& request.patch.channels.iter().all(|row| {
				self.channel(row.channel)
					.is_some_and(|c| c.guild == Some(request.guild))
					&& self.can_personally_manage_channel(row.channel)
			})
	}
	pub(crate) fn forget_guild_preferences(&mut self, guild: Id) {
		self.preference_queue
			.edits
			.retain(|key, _| key.guild != guild);
		if self
			.preference_queue
			.active
			.as_ref()
			.is_some_and(|flight| flight.request.guild == guild)
		{
			self.preference_queue.active = None;
		}
		if self.preference_queue.pending_refresh == Some(guild) {
			self.preference_queue.pending_refresh = None;
		}
		if self
			.preference_queue
			.refresh_status
			.is_some_and(|(target, _)| target == guild)
		{
			self.preference_queue.refresh_status = None;
		}
		self.preference_queue.versions.remove(&Some(guild));
		self.preference_queue.recount();
		self.notification_preferences.settings.remove(&Some(guild));
		if self.browse_channels == Some(guild) {
			self.browse_channels = None;
		}
	}
	pub(crate) fn interrupt_channel_preferences(&mut self) {
		let active = self.preference_queue.active.take();
		self.preference_queue.pending_refresh = None;
		if let Some(flight) = active.as_ref().filter(|flight| flight.refresh) {
			self.preference_queue.refresh_status =
				Some((flight.request.guild, Status::Unconfirmed));
		}
		for (key, edit) in &mut self.preference_queue.edits {
			if active.as_ref().is_some_and(|flight| {
				flight
					.fields
					.iter()
					.any(|(active, sent)| active == key && sent.sequence == edit.sequence)
			}) {
				edit.status = Status::Unconfirmed;
			} else if edit.status == Status::Saving {
				edit.status = Status::Failed("Save cancelled while disconnected");
			}
		}
	}

	pub fn demo_channel_preferences(&self, request: &Request) -> Finished {
		Finished {
			generation: request.generation,
			guild: request.guild,
			request: request.request,
			result: Ok(Update {
				guild: Some(request.guild),
				fields: request.patch.fields.clone(),
				channels: Some(request.patch.channels.clone()),
				..Update::default()
			}),
		}
	}
	pub(crate) fn finish_channel_preferences(
		&mut self,
		event: Finished,
	) -> Result<(), &'static str> {
		if event.generation != self.generation
			|| !self.preference_queue.active.as_ref().is_some_and(|flight| {
				flight.request.guild == event.guild && flight.request.request == event.request
			}) {
			return Ok(());
		}
		let flight = self
			.preference_queue
			.active
			.take()
			.expect("matching preference write");
		match event.result {
			Ok(mut update) => {
				if update.guild != Some(event.guild) {
					for (key, _) in &flight.fields {
						if let Some(edit) = self.preference_queue.edits.get_mut(key) {
							edit.status = Status::Unconfirmed;
						}
					}
					if flight.refresh {
						self.preference_queue.refresh_status =
							Some((event.guild, Status::Unconfirmed));
					}
					self.preference_queue.recount();
					self.revision = self.revision.wrapping_add(1);
					return Ok(());
				}
				// A legacy acknowledgement must not roll back an overlapping newer observation.
				let stale = update
					.version
					.zip(self.preference_queue.versions.get(&update.guild).copied())
					.is_some_and(|(new, old)| new < old);
				if (update.version.is_none() || flight.unversioned_observation)
					&& (flight.observed_fields != 0
						|| flight.all_channels
						|| !flight.observed_channels.is_empty())
				{
					update.full = false;
					strip_fields(&mut update.fields, flight.observed_fields);
					if flight.all_channels {
						update.channels = None;
						update.channels_unknown = false;
					} else if let Some(rows) = &mut update.channels {
						for row in rows {
							strip_fields(
								&mut row.fields,
								flight
									.observed_channels
									.get(&row.channel)
									.copied()
									.unwrap_or(0),
							);
						}
					}
				}
				let refresh_update = flight.refresh.then(|| update.clone());
				if let Err(error) = self.merge_guild_preference_update(update, false) {
					for (key, sent) in &flight.fields {
						if let Some(edit) = self.preference_queue.edits.get_mut(key)
							&& edit.sequence == sent.sequence
						{
							edit.status = Status::Unconfirmed;
						}
					}
					if flight.refresh {
						self.preference_queue.refresh_status =
							Some((event.guild, Status::Unconfirmed));
					}
					self.status = error;
					self.preference_queue.recount();
					self.revision = self.revision.wrapping_add(1);
					return Ok(());
				}
				for (key, sent) in &flight.fields {
					let key = *key;
					let current = self.confirmed_preference(key);
					let Some(edit) = self.preference_queue.edits.get(&key) else {
						continue;
					};
					if edit.sequence != sent.sequence {
						if let Some(current) = current
							&& let Some(edit) = self.preference_queue.edits.get_mut(&key)
						{
							edit.before = current;
						}
						continue;
					}
					if current.is_some_and(|value| same(value, sent.desired, sent.mask)) {
						self.preference_queue.edits.remove(&key);
					} else if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
						edit.status = if stale
							|| flight_observed(&flight, key)
							|| current.is_some_and(|value| !same(value, sent.before, sent.mask))
						{
							Status::Conflict
						} else {
							Status::Unconfirmed
						};
					}
				}
				if flight.refresh {
					self.preference_queue.refresh_status = None;
					self.reconcile_preference_edits(Some(event.guild), refresh_update.as_ref());
				}
			}
			Err(error) => {
				let rebase = error == Failure::ProtocolAt(NOT_DISPATCHED)
					&& self.preference_owner_allowed(&flight.request)
					&& !self.flight_conflicts(&flight)
					&& !self.flight_confirmed(&flight)
					&& self.flight_flags_changed(&flight);
				if flight.refresh {
					self.preference_queue.refresh_status = Some((
						event.guild,
						if matches!(error, Failure::Ambiguous | Failure::Network) {
							Status::Unconfirmed
						} else {
							Status::Failed(error.label())
						},
					));
				}
				for (key, sent) in flight.fields {
					let current = rebase.then(|| self.confirmed_preference(key)).flatten();
					if let Some(edit) = self.preference_queue.edits.get_mut(&key)
						&& edit.sequence == sent.sequence
						&& edit.status != Status::Conflict
					{
						if rebase && edit.status == Status::Saving {
							if let Some(current) = current {
								edit.before = current;
							}
							edit.due = Instant::now();
							continue;
						}
						edit.status = if matches!(error, Failure::Ambiguous | Failure::Network) {
							Status::Unconfirmed
						} else {
							Status::Failed(error.label())
						};
					}
				}
			}
		}
		self.preference_queue.recount();
		self.revision = self.revision.wrapping_add(1);
		Ok(())
	}
	fn reconcile_preference_edits(&mut self, guild: Option<Id>, update: Option<&Update>) {
		if self.preference_queue.edits.is_empty() {
			return;
		}
		let mut remove = Vec::new();
		let mut conflict = Vec::new();
		let mut rebase = Vec::new();
		for (key, edit) in &self.preference_queue.edits {
			if Some(key.guild) != guild
				|| update.is_some_and(|update| !update_has_field(update, *key))
			{
				continue;
			}
			let Some(value) = self.confirmed_preference(*key) else {
				if update.is_some() {
					conflict.push(*key);
				}
				continue;
			};
			if same(value, edit.desired, edit.mask) {
				remove.push(*key);
				continue;
			}
			let sent = self.preference_queue.active.as_ref().and_then(|flight| {
				flight
					.fields
					.iter()
					.find(|(active, _)| active == key)
					.map(|(_, sent)| sent)
			});
			if sent.is_some_and(|sent| {
				sent.sequence != edit.sequence && same(value, sent.desired, sent.mask)
			}) {
				rebase.push((*key, value));
				continue;
			}
			if !same(value, edit.before, edit.mask)
				|| (edit.status == Status::Unconfirmed && update.is_some())
			{
				conflict.push(*key);
			}
		}
		for key in remove {
			self.preference_queue.edits.remove(&key);
		}
		for (key, value) in rebase {
			if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
				edit.before = value;
			}
		}
		for key in conflict {
			if let Some(edit) = self.preference_queue.edits.get_mut(&key) {
				edit.status = Status::Conflict;
			}
		}
		self.preference_queue.recount();
	}

	#[cfg(test)]
	pub(crate) fn apply_guild_preference_update(
		&mut self,
		update: Update,
	) -> Result<(), &'static str> {
		self.apply_preference_updates(vec![update], false, true)
	}
	fn merge_guild_preference_update(
		&mut self,
		update: Update,
		observe: bool,
	) -> Result<(), &'static str> {
		self.apply_preference_updates(vec![update], false, observe)
	}
	pub(super) fn apply_preference_updates(
		&mut self,
		entries: Vec<Update>,
		replace: bool,
		observe: bool,
	) -> Result<(), &'static str> {
		use std::collections::BTreeSet;
		let keys: BTreeSet<_> = entries.iter().map(|update| update.guild).collect();
		if entries.capacity() * size_of::<Update>()
			+ entries
				.iter()
				.map(|update| update.bytes() - size_of::<Update>())
				.sum::<usize>()
			> 32 * 1024 * 1024
		{
			return Err("Channel preference snapshot exceeds safe capacity");
		}
		if entries.len() > crate::MAX_NAV
			|| keys.len() != entries.len()
			|| entries.iter().any(|update| !update.valid())
		{
			return Err("Invalid channel preference snapshot");
		}
		let stale = |update: &Update| {
			update
				.version
				.zip(self.preference_queue.versions.get(&update.guild).copied())
				.is_some_and(|(new, old)| new < old)
		};
		// An old complete inventory cannot retire settings that arrived with a newer observation.
		let replace = replace && !entries.iter().any(&stale);
		let mut prepared = Vec::with_capacity(entries.len());
		let mut applied = Vec::with_capacity(entries.len());
		for update in entries {
			if stale(&update) {
				continue;
			}
			let guild = update.guild;
			let mut setting = if update.full {
				crate::notifications::Setting {
					guild,
					..Default::default()
				}
			} else {
				self.notification_preferences
					.setting(guild)
					.cloned()
					.unwrap_or(crate::notifications::Setting {
						guild,
						..Default::default()
					})
			};
			setting.guild = guild;
			merge_fields(&mut setting, None, &update.fields);
			if update.full {
				setting.overrides_known = true;
			}
			if update.channels_unknown {
				setting.overrides_known = false;
				setting.channels.clear();
				setting.channel_flags.clear();
				setting.channel_mute_until.clear();
			}
			if let Some(rows) = &update.channels {
				for row in rows {
					merge_fields(&mut setting, Some(row.channel), &row.fields);
				}
			}
			prepared.push(setting);
			applied.push(update);
		}
		let changed: BTreeSet<_> = prepared.iter().map(|setting| setting.guild).collect();
		let retained: Vec<_> = self
			.notification_preferences
			.settings
			.iter()
			.filter(|(key, _)| (!replace || keys.contains(key)) && !changed.contains(key))
			.map(|(_, setting)| setting)
			.collect();
		let count = retained.len() + prepared.len();
		let rows = retained
			.iter()
			.map(|setting| setting.channels.len().max(setting.channel_flags.len()))
			.chain(
				prepared
					.iter()
					.map(|setting| setting.channels.len().max(setting.channel_flags.len())),
			)
			.sum::<usize>();
		let versions = self
			.preference_queue
			.versions
			.keys()
			.filter(|key| !replace || keys.contains(key))
			.count() + applied
			.iter()
			.filter(|update| {
				update.version.is_some()
					&& !self.preference_queue.versions.contains_key(&update.guild)
			})
			.count();
		let bytes = retained
			.iter()
			.map(|setting| setting.bytes() + 64)
			.chain(prepared.iter().map(|setting| setting.bytes() + 64))
			.sum::<usize>()
			+ self.preference_queue.bytes()
			- self.preference_queue.versions.len() * 48
			+ versions * 48;
		if count > crate::MAX_NAV
			|| rows > crate::MAX_NAV
			|| versions > crate::MAX_NAV
			|| bytes > 32 * 1024 * 1024
		{
			return Err("Channel preferences exceed safe capacity");
		}
		let known: Vec<_> = prepared
			.iter()
			.map(|setting| (setting.guild, setting.overrides_known))
			.collect();
		if replace {
			self.notification_preferences
				.settings
				.retain(|key, _| keys.contains(key));
			self.preference_queue.retain_versions(&keys);
			self.notification_preferences.complete = true;
			self.startup_warnings.notifications = false;
		}
		self.apply_notification_preferences(crate::notifications::Event::Settings {
			entries: prepared,
			replace: false,
		})?;
		for (guild, known) in known {
			if let Some(setting) = self.notification_preferences.settings.get_mut(&guild) {
				setting.overrides_known = known;
			}
		}
		if replace && observe {
			let mut omitted: BTreeSet<_> = self
				.preference_queue
				.edits
				.keys()
				.map(|key| key.guild)
				.filter(|guild| !keys.contains(&Some(*guild)))
				.collect();
			if let Some(flight) = &self.preference_queue.active
				&& !keys.contains(&Some(flight.request.guild))
			{
				omitted.insert(flight.request.guild);
			}
			for guild in omitted {
				let update = Update {
					guild: Some(guild),
					full: true,
					fields: Fields {
						flags: Patch::Value(0),
						muted: Patch::Value(false),
						level: Patch::Value(3),
						mute_until: Patch::Null,
						suppress_everyone: Patch::Value(false),
						suppress_roles: Patch::Value(false),
						hide_muted: Patch::Value(false),
						..Default::default()
					},
					channels: Some(vec![]),
					..Default::default()
				};
				let available = model::guild_preferences::MAX_PENDING_BYTES
					.saturating_sub(self.preference_queue.bytes());
				let targets = self.preference_queue.edits.len();
				if let Some(flight) = &mut self.preference_queue.active
					&& flight.request.guild == guild
				{
					observe_update(flight, &update, available, targets);
				}
				self.reconcile_preference_edits(Some(guild), Some(&update));
			}
		}
		for update in applied {
			if let Some(version) = update.version {
				self.preference_queue.versions.insert(update.guild, version);
			}
			if observe {
				let available = model::guild_preferences::MAX_PENDING_BYTES
					.saturating_sub(self.preference_queue.bytes());
				let targets = self.preference_queue.edits.len();
				if let Some(flight) = &mut self.preference_queue.active
					&& Some(flight.request.guild) == update.guild
				{
					observe_update(flight, &update, available, targets);
				}
				self.reconcile_preference_edits(update.guild, Some(&update));
			}
		}
		Ok(())
	}
}
fn update_has_field(update: &Update, key: Key) -> bool {
	let fields = if let Some(channel) = key.channel {
		let Some(row) = update
			.channels
			.as_ref()
			.and_then(|rows| rows.iter().find(|row| row.channel == channel))
		else {
			return update.full || update.channels_unknown;
		};
		&row.fields
	} else {
		&update.fields
	};
	match key.field {
		Field::Flags => fields.flags != Patch::Absent,
		Field::Collapsed => fields.collapsed != Patch::Absent,
		Field::Muted => fields.muted != Patch::Absent,
		Field::Level => fields.level != Patch::Absent,
		Field::Until => fields.mute_until != Patch::Absent,
		Field::Everyone => fields.suppress_everyone != Patch::Absent,
		Field::Roles => fields.suppress_roles != Patch::Absent,
		Field::HideMuted => fields.hide_muted != Patch::Absent,
	}
}
fn field_mask(fields: &Fields) -> u16 {
	let present = [
		fields.flags != Patch::Absent,
		fields.collapsed != Patch::Absent,
		fields.muted != Patch::Absent,
		fields.level != Patch::Absent,
		fields.mute_until != Patch::Absent,
		fields.suppress_everyone != Patch::Absent,
		fields.suppress_roles != Patch::Absent,
		fields.hide_muted != Patch::Absent,
	];
	present
		.into_iter()
		.enumerate()
		.fold(0, |mask, (index, present)| {
			mask | if present { 1 << index } else { 0 }
		})
}
fn strip_fields(fields: &mut Fields, mask: u16) {
	if mask & 1 != 0 {
		fields.flags = Patch::Absent;
	}
	if mask & 2 != 0 {
		fields.collapsed = Patch::Absent;
	}
	if mask & 4 != 0 {
		fields.muted = Patch::Absent;
	}
	if mask & 8 != 0 {
		fields.level = Patch::Absent;
	}
	if mask & 16 != 0 {
		fields.mute_until = Patch::Absent;
	}
	if mask & 32 != 0 {
		fields.suppress_everyone = Patch::Absent;
	}
	if mask & 64 != 0 {
		fields.suppress_roles = Patch::Absent;
	}
	if mask & 128 != 0 {
		fields.hide_muted = Patch::Absent;
	}
}
fn flight_observed(flight: &Flight, key: Key) -> bool {
	let mask = key.channel.map_or(flight.observed_fields, |channel| {
		if flight.all_channels {
			u16::MAX
		} else {
			flight.observed_channels.get(&channel).copied().unwrap_or(0)
		}
	});
	mask & (1 << (key.field as u8)) != 0
}
fn observe_update(flight: &mut Flight, update: &Update, mut available: usize, targets: usize) {
	if update.version.is_none()
		&& (update.full
			|| update.channels_unknown
			|| field_mask(&update.fields) != 0
			|| update
				.channels
				.as_ref()
				.is_some_and(|rows| !rows.is_empty()))
	{
		flight.unversioned_observation = true;
	}
	flight.observed_fields |= field_mask(&update.fields);
	if update.full || update.channels_unknown {
		flight.all_channels = true;
		flight.observed_channels.clear();
		return;
	}
	if flight.all_channels {
		return;
	}
	if let Some(rows) = &update.channels {
		for row in rows {
			let mask = field_mask(&row.fields);
			if mask == 0 {
				continue;
			}
			if let Some(observed) = flight.observed_channels.get_mut(&row.channel) {
				*observed |= mask;
				continue;
			}
			let bytes = size_of::<(Id, u16)>() + 48;
			if bytes > available
				|| flight.observed_channels.len() + targets >= model::guild_preferences::MAX_EDITS
			{
				flight.all_channels = true;
				flight.observed_channels.clear();
				return;
			}
			available -= bytes;
			flight.observed_channels.insert(row.channel, mask);
		}
	}
}
fn set<T: Copy>(target: &mut Option<T>, patch: &Patch<T>) {
	match patch {
		Patch::Absent => {}
		Patch::Null => *target = None,
		Patch::Value(value) => *target = Some(*value),
	}
}
fn merge_fields(setting: &mut crate::notifications::Setting, channel: Option<Id>, fields: &Fields) {
	let Some(channel) = channel else {
		set(&mut setting.flags, &fields.flags);
		set(&mut setting.muted, &fields.muted);
		if matches!(fields.muted, Patch::Value(false) | Patch::Null) {
			setting.mute_until = None;
		}
		set(&mut setting.level, &fields.level);
		set(&mut setting.mute_until, &fields.mute_until);
		set(&mut setting.suppress_everyone, &fields.suppress_everyone);
		set(&mut setting.suppress_roles, &fields.suppress_roles);
		set(&mut setting.hide_muted_channels, &fields.hide_muted);
		return;
	};
	if fields.flags != Patch::Absent || fields.collapsed != Patch::Absent {
		let index = setting
			.channel_flags
			.iter()
			.position(|(id, ..)| *id == channel)
			.unwrap_or_else(|| {
				setting.channel_flags.push((
					channel,
					setting.overrides_known.then_some(0),
					setting.overrides_known.then_some(false),
				));
				setting.channel_flags.len() - 1
			});
		let (_, flags, collapsed) = &mut setting.channel_flags[index];
		set(flags, &fields.flags);
		set(collapsed, &fields.collapsed);
	}
	if fields.muted != Patch::Absent || fields.level != Patch::Absent {
		let index = setting
			.channels
			.iter()
			.position(|(id, ..)| *id == channel)
			.unwrap_or_else(|| {
				setting.channels.push((
					channel,
					setting.overrides_known.then_some(false),
					setting.overrides_known.then_some(3),
				));
				setting.channels.len() - 1
			});
		let (_, muted, level) = &mut setting.channels[index];
		set(muted, &fields.muted);
		set(level, &fields.level);
	}
	if fields.mute_until != Patch::Absent
		|| matches!(fields.muted, Patch::Value(false) | Patch::Null)
	{
		setting.channel_mute_until.retain(|(id, _)| *id != channel);
		if let Patch::Value(until) = fields.mute_until
			&& setting
				.channels
				.iter()
				.any(|(id, muted, _)| *id == channel && *muted == Some(true))
		{
			setting.channel_mute_until.push((channel, until));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event};
	use model::channel_visibility as p;
	fn state() -> State {
		crate::personal_channels::tests::state()
	}
	fn due(state: &mut State) -> Box<Request> {
		state.flush_channel_preferences();
		let Command::ChannelPreferences(request) = state
			.poll_channel_preferences(Instant::now())
			.expect("due preference")
		else {
			panic!()
		};
		request
	}
	fn incoming(state: &mut State, fields: Fields, version: Option<u64>) {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::NotificationPreferences(crate::notifications::Event::Updates {
				entries: vec![Update {
					guild: Some(Id(2)),
					version,
					fields,
					..Default::default()
				}],
				replace: false,
			}),
		});
	}
	fn finish(state: &mut State, request: &Request) {
		let event = state.demo_channel_preferences(request);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelPreferences(event),
		});
	}
	#[test]
	fn debounce_coalesces_and_keeps_one_write_while_new_choices_remain_optimistic() {
		let mut state = state();
		let before = Instant::now();
		state.request_channel_action(Id(3), Action::HideMuted(true));
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Saving)
		);
		assert!(state.poll_channel_preferences(before).is_none());
		assert!(state.next_channel_preference_deadline().unwrap() >= before + DEBOUNCE);
		let request = due(&mut state);
		state.request_channel_action(Id(3), Action::HideMuted(false));
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		assert!(
			state
				.poll_channel_preferences(Instant::now() + Duration::from_secs(1))
				.is_none()
		);
		finish(&mut state, &request);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		let next = due(&mut state);
		assert_eq!(next.patch.fields.hide_muted, Patch::Value(false));
		finish(&mut state, &next);
		assert_eq!(state.channel_preference_status(Id(2), None), None);
	}
	#[test]
	fn newer_gateway_false_beats_legacy_true_ack_even_when_it_matches_the_old_value() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(false),
				..Default::default()
			},
			None,
		);
		finish(&mut state, &request);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Conflict)
		);
		assert!(
			state
				.poll_channel_preferences(Instant::now() + Duration::from_secs(2))
				.is_none()
		);
		assert!(state.retry_channel_preference(Id(2), None));
		let retry = due(&mut state);
		finish(&mut state, &retry);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
	}
	#[test]
	fn versioned_and_sparse_updates_preserve_latest_values_and_unknown_bits() {
		let mut state = state();
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(false),
				flags: Patch::Value((1 << 55) | p::OPT_IN_CHANNELS_ON),
				..Default::default()
			},
			Some(10),
		);
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(false),
				..Default::default()
			},
			Some(12),
		);
		let mut result = state.demo_channel_preferences(&request);
		result.result.as_mut().unwrap().version = Some(11);
		state.finish_channel_preferences(result).unwrap();
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		assert_eq!(
			state.account_guild_flags(Id(2)),
			Some((1 << 55) | p::OPT_IN_CHANNELS_ON)
		);
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Conflict)
		);
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Null,
				..Default::default()
			},
			Some(13),
		);
		assert_eq!(state.preference_bool(Id(2), None, "hide_muted"), None);
		state.demo = false;
		assert_eq!(state.preference_bool(Id(2), None, "hide_muted"), None);
		assert!(state.channel_preferences_known(Id(2)));
	}
	#[test]
	fn unrelated_gateway_changes_rebase_flags_without_dropping_a_pending_choice() {
		let mut state = state();
		let action = state
			.personal_channel_flag_change(Id(3), p::FAVORITED, p::FAVORITED)
			.unwrap();
		state.request_channel_action(Id(3), action);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels: Some(vec![Row {
					channel: Id(3),
					fields: Fields {
						flags: Patch::Value((1 << 54) | (1 << 53)),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		let request = due(&mut state);
		assert_eq!(
			request.patch.channels[0].fields.flags,
			Patch::Value((1 << 54) | (1 << 53) | p::FAVORITED)
		);
		finish(&mut state, &request);
		assert_eq!(
			state.personal_channel_flags(Id(3)),
			Some((1 << 54) | (1 << 53) | p::FAVORITED)
		);
	}
	#[test]
	fn uncertain_save_is_not_replayed_and_refresh_is_queued_behind_the_active_write() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		assert!(state.refresh_channel_preferences(Id(2)).is_none());
		state
			.finish_channel_preferences(Finished {
				generation: request.generation,
				guild: request.guild,
				request: request.request,
				result: Err(Failure::Ambiguous),
			})
			.unwrap();
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Unconfirmed)
		);
		assert!(!state.retry_channel_preference(Id(2), None));
		let Command::ChannelPreferences(refresh) =
			state.poll_channel_preferences(Instant::now()).unwrap()
		else {
			panic!()
		};
		assert_eq!(refresh.patch, Write::default());
		state
			.finish_channel_preferences(Finished {
				generation: refresh.generation,
				guild: refresh.guild,
				request: refresh.request,
				result: Ok(Update {
					guild: Some(Id(2)),
					fields: Fields {
						hide_muted: Patch::Value(true),
						..Default::default()
					},
					..Default::default()
				}),
			})
			.unwrap();
		assert_eq!(state.channel_preference_status(Id(2), None), None);
		assert!(
			state
				.poll_channel_preferences(Instant::now() + Duration::from_secs(10))
				.is_none()
		);
	}
	#[test]
	fn rejection_rolls_back_only_failed_fields_and_retry_is_explicit() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::Notifications(1));
		let request = due(&mut state);
		assert_eq!(state.channel_notification_level(Id(3)), Some(1));
		state
			.finish_channel_preferences(Finished {
				generation: request.generation,
				guild: request.guild,
				request: request.request,
				result: Err(Failure::Forbidden),
			})
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(3)), Some(2));
		assert!(matches!(
			state.channel_preference_status(Id(2), Some(Id(3))),
			Some(Status::Failed(_))
		));
		assert_eq!(state.channel_preference_status(Id(2), None), None);
		assert!(state.retry_channel_preference(Id(2), Some(Id(3))));
		let request = due(&mut state);
		finish(&mut state, &request);
		assert_eq!(state.channel_notification_level(Id(3)), Some(1));
	}
	#[test]
	fn category_inheritance_and_single_child_removal_preserve_siblings_and_thread_parent() {
		let mut state = state();
		incoming(
			&mut state,
			Fields {
				flags: Patch::Value(p::OPT_IN_CHANNELS_ON),
				..Default::default()
			},
			None,
		);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels: Some(vec![Row {
					channel: Id(5),
					fields: Fields {
						flags: Patch::Value(p::OPT_IN_ENABLED),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		let mut thread = state.channel(Id(3)).unwrap().clone();
		thread.id = Id(8);
		thread.kind = 11;
		thread.parent_id = Some(Id(3));
		state.channels.push(thread);
		state.invalidate_navigation();
		assert_eq!(state.channel_in_custom_list(Id(3)), Some(true));
		assert_eq!(state.channel_in_custom_list(Id(4)), Some(true));
		assert_eq!(state.channel_in_custom_list(Id(8)), Some(true));
		let action = state.personal_visibility_change(Id(3), false).unwrap();
		state.request_channel_action(Id(3), action);
		assert_eq!(state.channel_in_custom_list(Id(3)), Some(false));
		assert_eq!(state.channel_in_custom_list(Id(4)), Some(true));
		assert_eq!(state.channel_in_custom_list(Id(8)), Some(false));
		assert_eq!(state.personal_channel_flags(Id(5)), Some(0));
		let request = due(&mut state);
		finish(&mut state, &request);
		assert_eq!(state.guild_channel_muted(Id(3)), Some(true));
	}
	#[test]
	fn bounded_edits_are_atomic_and_logout_retires_the_old_generation() {
		let mut state = state();
		let now = Instant::now();
		let values = vec![
			(None, Field::HideMuted, Value::Bool(true), 0);
			model::guild_preferences::MAX_EDITS + 1
		];
		assert!(!state.queue_preference_fields(Id(2), values, now));
		assert_eq!(state.preference_queue.bytes(), 0);
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		let generation = state.generation;
		state.logout();
		state.apply(Envelope {
			generation,
			event: Event::ChannelPreferences(Finished {
				generation,
				guild: request.guild,
				request: request.request,
				result: Ok(Update {
					guild: Some(Id(2)),
					fields: Fields {
						hide_muted: Patch::Value(true),
						..Default::default()
					},
					..Default::default()
				}),
			}),
		});
		assert!(!state.channel_preferences_known(Id(2)));
		assert!(state.preference_queue.edits.is_empty());
		assert!(state.preference_queue.active.is_none());
		assert!(
			state
				.poll_channel_preferences(now + Duration::from_secs(10))
				.is_none()
		);
	}
	#[test]
	fn invalid_updates_keep_confirmed_preferences_and_disconnection_never_replays() {
		let mut state = state();
		assert!(
			state
				.apply_guild_preference_update(Update {
					guild: Some(Id(2)),
					fields: Fields {
						level: Patch::Value(9),
						..Default::default()
					},
					..Default::default()
				})
				.is_err()
		);
		assert_eq!(state.channel_notification_level(Id(3)), Some(2));
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		state.interrupt_channel_preferences();
		state.interrupt_channel_preferences();
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Unconfirmed)
		);
		finish(&mut state, &request);
		assert!(
			state
				.poll_channel_preferences(Instant::now() + Duration::from_secs(10))
				.is_none()
		);
	}
	#[test]
	fn stale_legacy_refresh_cannot_restore_fields_or_clear_a_newer_override_collection() {
		let mut state = state();
		let Command::ChannelPreferences(refresh) =
			state.refresh_channel_preferences(Id(2)).unwrap()
		else {
			panic!()
		};
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(true),
				..Default::default()
			},
			None,
		);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels: Some(vec![Row {
					channel: Id(4),
					fields: Fields {
						flags: Patch::Value(p::OPT_IN_ENABLED | 1 << 52),
						muted: Patch::Value(true),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		let old = Update {
			guild: Some(Id(2)),
			full: true,
			fields: Fields {
				flags: Patch::Value(1 << 55),
				muted: Patch::Value(false),
				level: Patch::Value(0),
				hide_muted: Patch::Value(false),
				..Default::default()
			},
			channels: Some(vec![Row {
				channel: Id(4),
				fields: Fields {
					flags: Patch::Value(0),
					muted: Patch::Value(false),
					..Default::default()
				},
			}]),
			..Default::default()
		};
		state
			.finish_channel_preferences(Finished {
				generation: refresh.generation,
				guild: refresh.guild,
				request: refresh.request,
				result: Ok(old),
			})
			.unwrap();
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
		assert_eq!(
			state.personal_channel_flags(Id(4)),
			Some(p::OPT_IN_ENABLED | 1 << 52)
		);
		assert_eq!(state.guild_channel_muted(Id(4)), Some(true));
		assert_eq!(
			state.personal_channel_flags(Id(3)),
			Some(1 << 54),
			"a stale complete response becomes sparse instead of clearing absent overrides"
		);
	}
	#[test]
	fn ordinary_ack_does_not_roll_back_unrelated_newer_fields() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::Notifications(1));
		let request = due(&mut state);
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(true),
				..Default::default()
			},
			None,
		);
		let mut event = state.demo_channel_preferences(&request);
		event.result.as_mut().unwrap().fields.hide_muted = Patch::Value(false);
		finish_result(&mut state, event);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
		assert_eq!(state.channel_notification_level(Id(3)), Some(1));
		assert_eq!(state.channel_preference_status(Id(2), Some(Id(3))), None);
	}
	fn finish_result(state: &mut State, event: Finished) {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelPreferences(event),
		});
	}
	#[test]
	fn explicit_null_override_collection_is_unknown_and_preserves_other_preferences() {
		let mut state = state();
		state.demo = false;
		state.auth = AuthState::Authenticated;
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels_unknown: true,
				fields: Fields {
					hide_muted: Patch::Value(true),
					..Default::default()
				},
				..Default::default()
			})
			.unwrap();
		assert!(!state.channel_preferences_known(Id(2)));
		assert_eq!(
			state.channel_in_custom_list(Id(3)),
			Some(true),
			"implicit all remains an existing guild preference"
		);
		assert_eq!(state.personal_channel_flags(Id(3)), None);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
		assert!(!state.personal_channel_controls_available(Id(2)));
	}
	#[test]
	fn newer_choice_survives_old_ack_and_gateway_completion_before_next_save() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let old = due(&mut state);
		state.request_channel_action(Id(3), Action::HideMuted(false));
		incoming(
			&mut state,
			Fields {
				hide_muted: Patch::Value(true),
				..Default::default()
			},
			Some(20),
		);
		finish(&mut state, &old);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Saving)
		);
		let newer = due(&mut state);
		assert_eq!(newer.patch.fields.hide_muted, Patch::Value(false));
		finish(&mut state, &newer);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
	}
	#[test]
	fn optimistic_and_unconfirmed_timers_schedule_expiry_and_failed_admission_invalidates() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::Mute(Mute::For(900)));
		let until = state.preference_until(Id(2), Some(Id(3))).unwrap();
		assert_eq!(
			state.next_mute_deadline(),
			std::time::UNIX_EPOCH.checked_add(Duration::from_secs(until as u64))
		);
		let request = due(&mut state);
		state
			.finish_channel_preferences(Finished {
				generation: request.generation,
				guild: request.guild,
				request: request.request,
				result: Err(Failure::Ambiguous),
			})
			.unwrap();
		assert_eq!(
			state.next_mute_deadline(),
			std::time::UNIX_EPOCH.checked_add(Duration::from_secs(until as u64))
		);
		state.request_channel_action(Id(3), Action::Notifications(1));
		state.flush_channel_preferences();
		let revision = state.revision;
		state.permissions.channels.remove(&Id(3));
		state.permissions.clear_cache();
		assert!(state.poll_channel_preferences(Instant::now()).is_none());
		assert!(state.revision > revision);
		assert!(matches!(
			state.channel_preference_status(Id(2), Some(Id(3))),
			Some(Status::Failed(_))
		));
	}
	#[test]
	fn malformed_batch_is_atomic_and_unversioned_sparse_null_does_not_reset_other_settings() {
		let mut state = state();
		let before = state.account_guild_flags(Id(2));
		assert!(
			state
				.apply_preference_updates(
					vec![
						Update {
							guild: Some(Id(2)),
							fields: Fields {
								flags: Patch::Value(p::OPT_IN_CHANNELS_ON),
								..Default::default()
							},
							..Default::default()
						},
						Update {
							guild: Some(Id(9)),
							fields: Fields {
								level: Patch::Value(9),
								..Default::default()
							},
							..Default::default()
						}
					],
					true,
					true
				)
				.is_err()
		);
		assert_eq!(state.account_guild_flags(Id(2)), before);
		assert_eq!(state.guild_channel_muted(Id(3)), Some(true));
		incoming(
			&mut state,
			Fields {
				suppress_roles: Patch::Null,
				..Default::default()
			},
			None,
		);
		assert_eq!(
			state.server_notification_settings(Id(2)).suppress_roles,
			None
		);
		assert_eq!(state.account_guild_flags(Id(2)), before);
		assert_eq!(state.channel_notification_level(Id(3)), Some(2));
	}
	#[test]
	fn newer_complete_inventory_defaults_an_omitted_guild_and_blocks_an_old_ack() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		state.apply_preference_updates(vec![], true, true).unwrap();
		let mut event = state.demo_channel_preferences(&request);
		event.result.as_mut().unwrap().version = Some(8);
		state.finish_channel_preferences(event).unwrap();
		assert_eq!(state.hides_muted_channels(Id(2)), Some(false));
		assert_eq!(state.account_guild_flags(Id(2)), Some(0));
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Conflict)
		);
	}

	#[test]
	fn sparse_ack_after_a_complete_default_snapshot_keeps_the_target_guild_identity() {
		let mut state = state();
		state.apply_preference_updates(vec![], true, true).unwrap();
		assert!(!state.notification_preferences.settings.contains_key(&None));
		state.request_channel_action(Id(3), Action::HideMuted(true));
		let request = due(&mut state);
		finish(&mut state, &request);
		assert_eq!(state.hides_muted_channels(Id(2)), Some(true));
		assert!(
			state
				.notification_preferences
				.settings
				.contains_key(&Some(Id(2)))
		);
		assert!(
			!state.notification_preferences.settings.contains_key(&None),
			"a synthesized default must not become a global DM settings entry"
		);
	}

	#[test]
	fn queued_transport_scope_is_revoked_by_channel_access_guild_or_account_loss() {
		for reason in 0..3 {
			let mut state = state();
			state.request_channel_action(Id(3), Action::Notifications(1));
			let request = due(&mut state);
			assert_eq!(
				state.channel_preference_scope(),
				Some((request.generation, request.guild, request.request))
			);
			match reason {
				0 => {
					state.permissions.channels.remove(&Id(3));
					state.permissions.clear_cache();
				}
				1 => state.forget_guild_preferences(Id(2)),
				_ => state.logout(),
			}
			assert_eq!(state.channel_preference_scope(), None);
		}
	}
	#[test]
	fn meaningful_active_conflicts_are_authoritative_and_revoke_only_the_unsent_scope() {
		let mut state = state();
		state.request_channel_action(Id(3), Action::Notifications(1));
		let request = due(&mut state);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels: Some(vec![Row {
					channel: Id(3),
					fields: Fields {
						level: Patch::Value(0),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(3)), Some(0));
		assert_eq!(
			state.channel_preference_status(Id(2), Some(Id(3))),
			Some(Status::Conflict)
		);
		assert_eq!(state.channel_preference_scope(), None);
		state
			.finish_channel_preferences(Finished {
				generation: request.generation,
				guild: request.guild,
				request: request.request,
				result: Err(Failure::Network),
			})
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(3)), Some(0));
		assert_eq!(
			state.channel_preference_status(Id(2), Some(Id(3))),
			Some(Status::Conflict)
		);
		assert!(
			state
				.poll_channel_preferences(Instant::now() + Duration::from_secs(1))
				.is_none()
		);
	}
	#[test]
	fn unrelated_full_records_keep_unsent_admission_and_authoritative_confirmation_retires_redundant_work()
	 {
		let mut state = state();
		state.request_channel_action(Id(3), Action::Notifications(1));
		let request = due(&mut state);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				full: true,
				fields: Fields {
					flags: Patch::Value(1 << 55),
					muted: Patch::Value(false),
					level: Patch::Value(0),
					hide_muted: Patch::Value(true),
					..Default::default()
				},
				channels: Some(vec![Row {
					channel: Id(3),
					fields: Fields {
						flags: Patch::Value(1 << 54),
						muted: Patch::Value(true),
						level: Patch::Value(2),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		assert_eq!(state.channel_notification_level(Id(3)), Some(1));
		assert_eq!(
			state.channel_preference_scope(),
			Some((request.generation, request.guild, request.request))
		);
		assert_eq!(
			state.channel_preference_status(Id(2), Some(Id(3))),
			Some(Status::Saving)
		);
		state
			.apply_guild_preference_update(Update {
				guild: Some(Id(2)),
				channels: Some(vec![Row {
					channel: Id(3),
					fields: Fields {
						level: Patch::Value(1),
						..Default::default()
					},
				}]),
				..Default::default()
			})
			.unwrap();
		assert_eq!(state.channel_preference_scope(), None);
		assert_eq!(state.channel_preference_status(Id(2), Some(Id(3))), None);
		assert_eq!(state.channel_notification_level(Id(3)), Some(1));
	}
	#[test]
	fn an_unsent_flag_word_rebases_unknown_bits_but_an_uncertain_admitted_write_never_replays() {
		for admitted in [false, true] {
			let mut state = state();
			let action = state
				.personal_channel_flag_change(Id(3), p::FAVORITED, p::FAVORITED)
				.unwrap();
			state.request_channel_action(Id(3), action);
			let request = due(&mut state);
			state
				.apply_guild_preference_update(Update {
					guild: Some(Id(2)),
					channels: Some(vec![Row {
						channel: Id(3),
						fields: Fields {
							flags: Patch::Value((1 << 54) | (1 << 53)),
							..Default::default()
						},
					}]),
					..Default::default()
				})
				.unwrap();
			assert_eq!(
				state.channel_preference_scope(),
				None,
				"the outgoing word predates newly observed unrelated bits"
			);
			assert_eq!(
				state.channel_preference_status(Id(2), Some(Id(3))),
				Some(Status::Saving)
			);
			state
				.finish_channel_preferences(Finished {
					generation: request.generation,
					guild: request.guild,
					request: request.request,
					result: Err(if admitted {
						Failure::Ambiguous
					} else {
						Failure::ProtocolAt(NOT_DISPATCHED)
					}),
				})
				.unwrap();
			if admitted {
				assert_eq!(
					state.channel_preference_status(Id(2), Some(Id(3))),
					Some(Status::Unconfirmed)
				);
				assert!(
					state
						.poll_channel_preferences(Instant::now() + Duration::from_secs(10))
						.is_none()
				);
			} else {
				let next = due(&mut state);
				assert_eq!(
					next.patch.channels[0].fields.flags,
					Patch::Value((1 << 54) | (1 << 53) | p::FAVORITED)
				);
				finish(&mut state, &next);
				assert_eq!(
					state.personal_channel_flags(Id(3)),
					Some((1 << 54) | (1 << 53) | p::FAVORITED)
				);
			}
		}
	}
	#[test]
	fn paired_preference_modes_are_one_edit_and_cannot_merge_into_both_bits() {
		let mut state = state();
		let action = state.personal_show_all_change(Id(2), false).unwrap();
		state.request_channel_action(Id(3), action);
		let request = due(&mut state);
		incoming(
			&mut state,
			Fields {
				flags: Patch::Value((1 << 55) | p::OPT_IN_CHANNELS_OFF),
				..Default::default()
			},
			None,
		);
		assert_eq!(state.channel_preference_scope(), None);
		assert!(state.personally_shows_all_channels(Id(2)));
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Conflict)
		);
		state
			.finish_channel_preferences(Finished {
				generation: request.generation,
				guild: request.guild,
				request: request.request,
				result: Err(Failure::ProtocolAt(NOT_DISPATCHED)),
			})
			.unwrap();
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(Status::Conflict)
		);
		for group in [
			p::UNREADS_ONLY_MENTIONS | p::UNREADS_ALL_MESSAGES,
			p::NEW_FORUM_THREADS_OFF | p::NEW_FORUM_THREADS_ON,
		] {
			let mut state = crate::personal_channels::tests::state();
			let first = group.isolate_lowest_one();
			let other = group ^ first;
			let action = state
				.personal_channel_flag_change(Id(3), group, first)
				.unwrap();
			state.request_channel_action(Id(3), action);
			let request = due(&mut state);
			state
				.apply_guild_preference_update(Update {
					guild: Some(Id(2)),
					channels: Some(vec![Row {
						channel: Id(3),
						fields: Fields {
							flags: Patch::Value((1 << 54) | other),
							..Default::default()
						},
					}]),
					..Default::default()
				})
				.unwrap();
			assert_eq!(state.channel_preference_scope(), None);
			assert_eq!(state.personal_channel_flags(Id(3)), Some((1 << 54) | other));
			assert_eq!(
				state.channel_preference_status(Id(2), Some(Id(3))),
				Some(Status::Conflict)
			);
			state
				.finish_channel_preferences(Finished {
					generation: request.generation,
					guild: request.guild,
					request: request.request,
					result: Err(Failure::ProtocolAt(NOT_DISPATCHED)),
				})
				.unwrap();
		}
	}
}
