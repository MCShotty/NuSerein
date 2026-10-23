//! Bounded server application/command permission edits. Unknown target kinds are retained.
use crate::Id;
use serde::{Deserialize, Serialize};
pub const MAX_OVERRIDES: usize = 100;
pub const MAX_COMMANDS: usize = 256;
pub const MAX_BYTES: usize = 512 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Override {
	pub id: Id,
	#[serde(rename = "type")]
	pub kind: u8,
	pub permission: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
	pub id: Id,
	pub overrides: Vec<Override>,
}
impl Permissions {
	pub fn valid(&self) -> bool {
		self.id.0 != 0
			&& self.overrides.len() <= MAX_OVERRIDES
			&& self.overrides.capacity() <= MAX_OVERRIDES * 2
			&& self.overrides.iter().enumerate().all(|(i, item)| {
				item.id.0 != 0
					&& item.kind != 0
					&& !self.overrides[..i]
						.iter()
						.any(|old| old.id == item.id && old.kind == item.kind)
			})
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.overrides.capacity() * size_of::<Override>()
	}
	pub fn equivalent(&self, other: &Self) -> bool {
		self.id == other.id
			&& self.overrides.len() == other.overrides.len()
			&& self
				.overrides
				.iter()
				.all(|row| other.overrides.contains(row))
	}
	/// Explicitly reapply the user's edited keys after they have reviewed a fresh response.
	pub fn reapply(before: &Self, after: &Self, actual: &Self) -> Option<Self> {
		if !before.valid()
			|| !after.valid()
			|| !actual.valid()
			|| before.id != after.id
			|| after.id != actual.id
		{
			return None;
		}
		let mut latest = actual.clone();
		let mut keys = std::collections::BTreeSet::new();
		for row in before.overrides.iter().chain(after.overrides.iter()) {
			keys.insert((row.kind, row.id));
		}
		for (kind, id) in keys {
			let old = before
				.overrides
				.iter()
				.find(|row| row.kind == kind && row.id == id);
			let new = after
				.overrides
				.iter()
				.find(|row| row.kind == kind && row.id == id);
			if old == new {
				continue;
			}
			latest
				.overrides
				.retain(|row| row.kind != kind || row.id != id);
			if let Some(row) = new {
				latest.overrides.push(row.clone());
			}
		}
		latest.overrides.shrink_to_fit();
		latest.valid().then_some(latest)
	}
	pub fn confirms(before: &Self, after: &Self, actual: &Self) -> bool {
		if before.id != after.id || actual.id != after.id || !actual.valid() {
			return false;
		}
		let mut keys = std::collections::BTreeSet::new();
		for row in before.overrides.iter().chain(after.overrides.iter()) {
			keys.insert((row.kind, row.id));
		}
		keys.into_iter().all(|(kind, id)| {
			let find = |set: &Self| {
				set.overrides
					.iter()
					.find(|row| row.kind == kind && row.id == id)
					.cloned()
			};
			find(before) == find(after) || find(actual) == find(after)
		})
	}
	/// Apply only intentionally edited keys to a fresh server response. Concurrent edits of
	/// the same key conflict; unrelated/new/unknown overrides survive the full-list PUT.
	pub fn merge(before: &Self, after: &Self, actual: &Self) -> Option<Self> {
		if !before.valid()
			|| !after.valid()
			|| !actual.valid()
			|| before.id != after.id
			|| before.id != actual.id
		{
			return None;
		}
		let mut result = actual.clone();
		let mut keys = std::collections::BTreeSet::new();
		for row in before.overrides.iter().chain(after.overrides.iter()) {
			keys.insert((row.kind, row.id));
		}
		for (kind, id) in keys {
			let find = |set: &Self| {
				set.overrides
					.iter()
					.find(|row| row.kind == kind && row.id == id)
					.cloned()
			};
			let old = find(before);
			let new = find(after);
			if old == new {
				continue;
			}
			if find(actual) != old {
				return None;
			}
			result
				.overrides
				.retain(|row| row.kind != kind || row.id != id);
			if let Some(row) = new {
				result.overrides.push(row);
			}
		}
		result.overrides.sort_by_key(|row| (row.kind, row.id));
		result.overrides.shrink_to_fit();
		result.valid().then_some(result)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
	pub id: Id,
	pub name: String,
	pub kind: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
	pub guild: Id,
	pub application: Id,
	pub commands: Vec<Command>,
	pub permissions: Vec<Permissions>,
}
impl Snapshot {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.commands.capacity() * size_of::<Command>()
			+ self
				.commands
				.iter()
				.map(|row| row.name.capacity())
				.sum::<usize>()
			+ self
				.permissions
				.iter()
				.map(Permissions::bytes)
				.sum::<usize>()
			+ self
				.permissions
				.capacity()
				.saturating_sub(self.permissions.len())
				* size_of::<Permissions>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.application.0 != 0
			&& self.commands.len() <= MAX_COMMANDS
			&& self.permissions.len() <= MAX_COMMANDS + 1
			&& self.bytes() <= MAX_BYTES
			&& self.commands.iter().enumerate().all(|(i, row)| {
				row.id.0 != 0
					&& row.id != self.application
					&& (1..=3).contains(&row.kind)
					&& !row.name.is_empty()
					&& row.name.len() <= 128
					&& !row.name.chars().any(char::is_control)
					&& !self.commands[..i].iter().any(|old| old.id == row.id)
			}) && self.permissions.iter().enumerate().all(|(i, row)| {
			row.valid() && !self.permissions[..i].iter().any(|old| old.id == row.id)
		})
	}
	pub fn get(&self, id: Id) -> Option<Permissions> {
		if id != self.application && !self.commands.iter().any(|row| row.id == id) {
			return None;
		}
		Some(
			self.permissions
				.iter()
				.find(|row| row.id == id)
				.cloned()
				.unwrap_or(Permissions {
					id,
					overrides: vec![],
				}),
		)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
	Load {
		application: Id,
	},
	Save {
		application: Id,
		before: Permissions,
		after: Permissions,
	},
}
impl Action {
	pub fn application(&self) -> Id {
		match self {
			Self::Load { application } | Self::Save { application, .. } => *application,
		}
	}
	pub fn write(&self) -> bool {
		matches!(self, Self::Save { .. })
	}
	pub fn valid(&self) -> bool {
		self.application().0 != 0
			&& match self {
				Self::Load { .. } => true,
				Self::Save { before, after, .. } => {
					before.valid()
						&& after.valid() && before.id == after.id
						&& !before.equivalent(after)
				}
			}
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	fn set(rows: &[(u64, u8, bool)]) -> Permissions {
		Permissions {
			id: Id(2),
			overrides: rows
				.iter()
				.map(|(id, kind, permission)| Override {
					id: Id(*id),
					kind: *kind,
					permission: *permission,
				})
				.collect(),
		}
	}
	#[test]
	fn sparse_permissions_merge_preserves_concurrent_unknown_and_conflicts_on_changed_keys() {
		let before = set(&[(3, 1, true), (4, 77, true)]);
		let after = set(&[(3, 1, false), (4, 77, true)]);
		let actual = set(&[(3, 1, true), (4, 77, false), (5, 3, true)]);
		let result = Permissions::merge(&before, &after, &actual).unwrap();
		assert!(result.equivalent(&set(&[(3, 1, false), (4, 77, false), (5, 3, true)])));
		assert!(
			Permissions::merge(&before, &after, &set(&[(3, 1, false), (4, 77, true)])).is_none()
		);
		assert!(
			Permissions::merge(&before, &set(&[(4, 77, true)]), &actual)
				.unwrap()
				.equivalent(&set(&[(4, 77, false), (5, 3, true)]))
		);
	}
	#[test]
	fn current_hundred_override_limit_preserves_unknown_kinds_without_truncation() {
		let before = Permissions {
			id: Id(2),
			overrides: (1..=MAX_OVERRIDES as u64)
				.map(|id| Override {
					id: Id(id),
					kind: if id == 100 { 77 } else { 2 },
					permission: true,
				})
				.collect(),
		};
		assert!(before.valid());
		let mut after = before.clone();
		after.overrides[0].permission = false;
		let merged = Permissions::merge(&before, &after, &before).unwrap();
		assert_eq!(merged.overrides.len(), MAX_OVERRIDES);
		assert_eq!(merged.overrides.last().unwrap().kind, 77);
		after.overrides.push(Override {
			id: Id(101),
			kind: 2,
			permission: true,
		});
		assert!(!after.valid());
		assert!(Permissions::merge(&before, &after, &before).is_none());
	}
}
