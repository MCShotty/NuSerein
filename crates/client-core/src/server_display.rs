//! Presentation-only server metadata. Access always comes from current Gateway state.
use crate::{Event, State};
use model::{GuildPatch, Id, Patch};
use std::collections::BTreeMap;

pub const MAX_SERVERS: usize = 4096;
// Conservative charge covers B-tree nodes as well as the key and flags.
const FIELD_BYTES: usize = 96;

#[derive(Default)]
pub struct Displays {
	fields: BTreeMap<Id, (bool, bool, bool)>,
	revision: u64,
	available: bool,
	missing: usize,
	dirty: usize,
}

impl Displays {
	pub(crate) fn bytes(&self) -> usize {
		self.fields.len() * FIELD_BYTES
	}
}

impl State {
	pub fn server_display_revision(&self) -> u64 {
		self.server_display.revision
	}
	pub fn has_server_display_metadata(&self) -> bool {
		self.server_display.available
	}
	pub fn needs_server_display_hints(&self) -> bool {
		self.server_display.missing != 0
	}
	pub fn missing_server_display_ids(&self) -> Vec<Id> {
		self.server_display
			.fields
			.iter()
			.filter_map(|(&id, &(name, icon, _))| (!name || !icon).then_some(id))
			.collect()
	}
	pub fn has_dirty_server_displays(&self) -> bool {
		self.server_display.dirty != 0
	}
	pub fn acknowledge_server_displays(&mut self, revision: u64, ids: &[Id]) {
		if revision != self.server_display.revision {
			return;
		}
		for id in ids {
			if let Some((_, _, dirty)) = self.server_display.fields.get_mut(id)
				&& *dirty
			{
				*dirty = false;
				self.server_display.dirty -= 1;
			}
		}
	}

	#[cold]
	#[inline(never)]
	pub(crate) fn observe_server_display(&mut self, event: &Event) {
		match event {
			Event::Ready { .. } | Event::Resync => {
				self.server_display.fields.clear();
				self.server_display.missing = 0;
				self.server_display.dirty = 0;
				self.navigation_index.bytes.set(None);
				self.server_display.available = false;
				self.server_display.revision = self.server_display.revision.wrapping_add(1);
			}
			Event::GuildRemoved(id) => {
				if let Some((name, icon, dirty)) = self.server_display.fields.remove(id) {
					self.server_display.missing -= usize::from(!name || !icon);
					self.server_display.dirty -= usize::from(dirty);
					self.navigation_index.bytes.set(None);
				}
				self.server_display.revision = self.server_display.revision.wrapping_add(1);
			}
			Event::GuildDisplayMetadata(patches) if patches.len() <= MAX_SERVERS => {
				self.server_display.fields.clear();
				self.server_display.missing = 0;
				self.server_display.dirty = 0;
				self.navigation_index.bytes.set(None);
				self.server_display.available = true;
				let mut bytes = self
					.navigation_bytes()
					.saturating_add(self.permissions.bytes());
				for patch in patches {
					if self.guild(patch.id).is_some()
						&& !self.server_display.fields.contains_key(&patch.id)
						&& bytes.saturating_add(FIELD_BYTES) <= model::account::MAX_BYTES
					{
						let name = !matches!(patch.name, Patch::Absent);
						let icon = !matches!(patch.icon, Patch::Absent);
						let dirty = name || icon;
						self.server_display
							.fields
							.insert(patch.id, (name, icon, dirty));
						self.server_display.missing += usize::from(!name || !icon);
						self.server_display.dirty += usize::from(dirty);
						bytes += FIELD_BYTES;
					}
				}
				self.navigation_index.bytes.set(None);
				self.server_display.revision = self.server_display.revision.wrapping_add(1);
			}
			Event::GuildChanged(patch)
				if self.guild(patch.id).is_some()
					&& (!matches!(patch.name, Patch::Absent)
						|| !matches!(patch.icon, Patch::Absent)) =>
			{
				let exists = self.server_display.fields.contains_key(&patch.id);
				if exists
					|| (self.server_display.fields.len() < MAX_SERVERS
						&& self
							.navigation_bytes()
							.saturating_add(self.permissions.bytes())
							.saturating_add(FIELD_BYTES)
							<= model::account::MAX_BYTES)
				{
					let field = self.server_display.fields.entry(patch.id).or_default();
					let was_missing = !field.0 || !field.1;
					if !exists {
						self.server_display.missing += 1;
					}
					field.0 |= !matches!(patch.name, Patch::Absent);
					field.1 |= !matches!(patch.icon, Patch::Absent);
					if was_missing && field.0 && field.1 {
						self.server_display.missing -= 1;
					}
					if !field.2 {
						field.2 = true;
						self.server_display.dirty += 1;
					}
					if !exists {
						self.navigation_index.bytes.set(None);
					}
				}
				self.server_display.revision = self.server_display.revision.wrapping_add(1);
			}
			_ => {}
		}
	}

	/// Copy only values actually observed from Discord, never earlier cache fallbacks.
	pub fn dirty_server_displays(&self) -> Vec<GuildPatch> {
		self.server_display
			.fields
			.iter()
			.filter_map(|(&id, &(name, icon, dirty))| {
				if !dirty {
					return None;
				}
				let guild = self.guild(id)?;
				Some(GuildPatch {
					id,
					name: if name {
						Patch::Value(guild.name.clone())
					} else {
						Patch::Absent
					},
					icon: if icon {
						guild.icon.clone().map_or(Patch::Null, Patch::Value)
					} else {
						Patch::Absent
					},
					default_message_notifications: Patch::Absent,
				})
			})
			.take(128)
			.collect()
	}

	/// Only fill genuinely absent fields for guilds admitted by the current session.
	/// Reject the entire read if anything changed since it was queued.
	pub fn hydrate_server_displays(&mut self, revision: u64, patches: Vec<GuildPatch>) {
		if revision != self.server_display.revision || patches.len() > MAX_SERVERS {
			return;
		}
		let mut bytes = self.navigation_bytes();
		let permissions = self.permissions.bytes();
		let mut changed = false;
		for patch in patches {
			let Some(&(name_known, icon_known, _)) = self.server_display.fields.get(&patch.id)
			else {
				continue;
			};
			if self.guild(patch.id).is_none() {
				continue;
			}
			let Some(index) = self
				.navigation_index
				.guilds
				.borrow()
				.get(&patch.id)
				.copied()
			else {
				continue;
			};
			let guild = &mut self.guilds[index];
			let before = guild.bytes();
			let mut old_name = None;
			let mut old_icon = None;
			if !name_known
				&& let Patch::Value(name) = patch.name
				&& name.chars().count() <= 128
				&& !name.chars().any(char::is_control)
				&& guild.name != name
			{
				old_name = Some(std::mem::replace(&mut guild.name, name));
			}
			if !icon_known
				&& let Patch::Value(icon) = patch.icon
				&& model::valid_avatar_hash(&icon)
				&& guild.icon.as_ref() != Some(&icon)
			{
				old_icon = Some(guild.icon.replace(icon));
			}
			let next = bytes.saturating_sub(before).saturating_add(guild.bytes());
			if next.saturating_add(permissions) > model::account::MAX_BYTES {
				if let Some(name) = old_name {
					guild.name = name;
				}
				if let Some(icon) = old_icon {
					guild.icon = icon;
				}
			} else if old_name.is_some() || old_icon.is_some() {
				bytes = next;
				changed = true;
			}
		}
		if changed {
			self.set_navigation_bytes(bytes);
			self.revision = self.revision.wrapping_add(1);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::Envelope;
	use model::Guild;

	fn patch(id: u64, name: Patch<String>, icon: Patch<String>) -> GuildPatch {
		GuildPatch {
			id: Id(id),
			name,
			icon,
			default_message_notifications: Patch::Absent,
		}
	}
	fn state() -> State {
		let mut state = State::default();
		state.guilds.push(Guild {
			id: Id(1),
			name: String::new(),
			icon: None,
			emojis: None,
			stickers: None,
			default_message_notifications: None,
		});
		state
	}
	fn apply(state: &mut State, event: Event) {
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	}
	#[test]
	fn cached_values_only_fill_missing_fields_of_confirmed_guilds() {
		let mut state = state();
		apply(
			&mut state,
			Event::GuildDisplayMetadata(vec![patch(1, Patch::Absent, Patch::Null)]),
		);
		let rev = state.server_display_revision();
		state.hydrate_server_displays(
			rev,
			vec![
				patch(
					1,
					Patch::Value("cached".into()),
					Patch::Value("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
				),
				patch(9, Patch::Value("not a member".into()), Patch::Absent),
			],
		);
		assert_eq!(state.guilds[0].name, "cached");
		assert_eq!(state.guilds[0].icon, None, "explicit null must win");
		assert!(state.guild(Id(9)).is_none());
		let observed = state.dirty_server_displays();
		assert!(
			matches!(observed[0].name, Patch::Absent),
			"cached names must not become service observations"
		);
	}
	#[test]
	fn updates_and_removals_reject_late_cache_reads() {
		let mut state = state();
		apply(
			&mut state,
			Event::GuildDisplayMetadata(vec![patch(1, Patch::Absent, Patch::Absent)]),
		);
		let rev = state.server_display_revision();
		apply(
			&mut state,
			Event::GuildChanged(patch(1, Patch::Value("live".into()), Patch::Null)),
		);
		state.hydrate_server_displays(
			rev,
			vec![patch(
				1,
				Patch::Value("stale".into()),
				Patch::Value("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
			)],
		);
		assert_eq!(state.guilds[0].name, "live");
		assert_eq!(state.guilds[0].icon, None);
		let rev = state.server_display_revision();
		apply(&mut state, Event::GuildRemoved(Id(1)));
		state.hydrate_server_displays(
			rev,
			vec![patch(1, Patch::Value("resurrected".into()), Patch::Absent)],
		);
		assert!(state.guild(Id(1)).is_none());
	}

	#[test]
	fn provenance_respects_navigation_budget_and_invalidates_accounting() {
		let mut state = state();
		let base = state.navigation_bytes() + state.permissions.bytes();
		state.guilds[0].name =
			String::with_capacity(model::account::MAX_BYTES - base - FIELD_BYTES + 1);
		state.invalidate_navigation();
		assert!(
			state.navigation_bytes() + state.permissions.bytes() + FIELD_BYTES
				> model::account::MAX_BYTES
		);
		apply(
			&mut state,
			Event::GuildDisplayMetadata(vec![patch(1, Patch::Absent, Patch::Absent)]),
		);
		assert!(
			state.server_display.fields.is_empty(),
			"optional hints cannot exceed the navigation budget"
		);
		assert!(state.navigation_bytes() + state.permissions.bytes() <= model::account::MAX_BYTES);
		state.guilds[0].name = String::new();
		state.invalidate_navigation();
		let before = state.navigation_bytes();
		apply(
			&mut state,
			Event::GuildDisplayMetadata(vec![patch(1, Patch::Absent, Patch::Absent)]),
		);
		assert_eq!(state.navigation_bytes(), before + FIELD_BYTES);
		state.observe_server_display(&Event::GuildRemoved(Id(1)));
		assert_eq!(state.navigation_bytes(), before);
	}

	#[test]
	fn server_and_expression_hints_share_navigation_accounting() {
		let mut state = state();
		let base = state.navigation_bytes();
		state.invalidate_expression_event(&Event::GuildEmojis {
			guild: Id(1),
			emojis: Vec::new(),
		});
		let with_expression = state.navigation_bytes();
		assert!(with_expression > base);
		state.observe_server_display(&Event::GuildDisplayMetadata(vec![patch(
			1,
			Patch::Absent,
			Patch::Absent,
		)]));
		assert_eq!(state.navigation_bytes(), with_expression + FIELD_BYTES);
		state.observe_server_display(&Event::GuildRemoved(Id(1)));
		assert_eq!(state.navigation_bytes(), with_expression);
	}

	#[test]
	fn writes_only_dirty_server_fields_and_coalesces_repeated_changes() {
		let mut state = state();
		apply(
			&mut state,
			Event::GuildDisplayMetadata(vec![patch(1, Patch::Value("one".into()), Patch::Null)]),
		);
		assert!(!state.needs_server_display_hints());
		assert_eq!(state.dirty_server_displays().len(), 1);
		state.acknowledge_server_displays(state.server_display_revision(), &[Id(1)]);
		assert!(!state.has_dirty_server_displays());
		apply(
			&mut state,
			Event::GuildChanged(patch(1, Patch::Value("two".into()), Patch::Absent)),
		);
		let queued_revision = state.server_display_revision();
		apply(
			&mut state,
			Event::GuildChanged(patch(1, Patch::Value("three".into()), Patch::Absent)),
		);
		let pending = state.dirty_server_displays();
		state.acknowledge_server_displays(queued_revision, &[Id(1)]);
		assert!(
			state.has_dirty_server_displays(),
			"older write completion cannot clear a newer update"
		);
		assert_eq!(pending.len(), 1);
		assert!(matches!(&pending[0].name, Patch::Value(name) if name == "three"));
		state.acknowledge_server_displays(state.server_display_revision(), &[Id(1)]);
		assert!(!state.has_dirty_server_displays());
	}

	#[test]
	fn dirty_snapshots_are_bounded_and_progress_without_duplicates() {
		let mut state = state();
		for id in 2..=130 {
			state.guilds.push(Guild {
				id: Id(id),
				name: format!("Server {id}"),
				icon: None,
				emojis: None,
				stickers: None,
				default_message_notifications: None,
			});
		}
		state.invalidate_navigation();
		let patches = (1..=130)
			.map(|id| patch(id, Patch::Value(format!("Server {id}")), Patch::Null))
			.collect();
		apply(&mut state, Event::GuildDisplayMetadata(patches));
		let first = state.dirty_server_displays();
		assert_eq!(first.len(), 128);
		state.acknowledge_server_displays(
			state.server_display_revision(),
			&first.iter().map(|p| p.id).collect::<Vec<_>>(),
		);
		let last = state.dirty_server_displays();
		assert_eq!(last.len(), 2);
		assert!(
			last.iter()
				.all(|row| !first.iter().any(|prior| prior.id == row.id))
		);
		state.acknowledge_server_displays(
			state.server_display_revision(),
			&last.iter().map(|p| p.id).collect::<Vec<_>>(),
		);
		assert!(!state.has_dirty_server_displays());
	}
}
