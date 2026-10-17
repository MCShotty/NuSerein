use crate::{Channel, Id};

/// Which device-local shortcut list a channel belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shortcut {
	Pinned,
	Favorite,
}

/// Outcome of one shortcut write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceEdit {
	Unchanged,
	Changed,
	CapacityReached,
}

/// Device-local channel navigation preferences, isolated by account; never synchronized to Discord.
/// At most 256 IDs (2 KiB of ID payload) across all lists.
/// Vec order is display order: index 0 is shown first, and a new entry goes to the front.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ChannelPreferences {
	pub favorites: Vec<Id>,
	pub pinned: Vec<Id>,
	pub collapsed_categories: Vec<Id>,
	/// Last opened channel per server, oldest first; bounded separately from the lists above.
	pub last_channels: Vec<(Id, Id)>,
}

impl ChannelPreferences {
	pub const MAX_ENTRIES: usize = 256;
	pub const MAX_JSON_BYTES: usize = 8192;
	/// Keeps the worst case (256 list IDs plus these pairs of 20-digit IDs) under the JSON cap.
	pub const MAX_LAST_CHANNELS: usize = 32;

	pub fn is_valid(&self) -> bool {
		// ponytail: duplicate scans are capped at 256 IDs; use a set if this limit grows.
		self.favorites
			.len()
			.saturating_add(self.pinned.len())
			.saturating_add(self.collapsed_categories.len())
			<= Self::MAX_ENTRIES
			&& self
				.favorites
				.capacity()
				.saturating_add(self.pinned.capacity())
				.saturating_add(self.collapsed_categories.capacity())
				<= Self::MAX_ENTRIES * 3
			&& self.last_channels.capacity() <= Self::MAX_LAST_CHANNELS * 2
			&& self
				.last_channels
				.iter()
				.enumerate()
				.all(|(index, (guild, channel))| {
					guild.0 != 0
						&& channel.0 != 0 && !self.last_channels[..index]
						.iter()
						.any(|(id, _)| id == guild)
				}) && [&self.favorites, &self.pinned, &self.collapsed_categories]
			.into_iter()
			.all(|ids| {
				ids.iter()
					.enumerate()
					.all(|(index, id)| id.0 != 0 && !ids[..index].contains(id))
			})
	}

	fn list(&self, kind: Shortcut) -> &Vec<Id> {
		match kind {
			Shortcut::Pinned => &self.pinned,
			Shortcut::Favorite => &self.favorites,
		}
	}

	/// Display-ordered ids of one list.
	pub fn ids(&self, kind: Shortcut) -> &[Id] {
		self.list(kind)
	}

	pub fn contains(&self, kind: Shortcut, channel: Id) -> bool {
		self.list(kind).contains(&channel)
	}

	/// Home pins are 1:1 and group DMs only.
	pub fn can_pin(channel: &Channel) -> bool {
		channel.id.0 != 0 && channel.guild.is_none() && matches!(channel.kind, 1 | 3)
	}

	/// Server favorites are guild channels that are not categories.
	pub fn can_favorite(channel: &Channel) -> bool {
		channel.id.0 != 0 && channel.guild.is_some() && channel.kind != 4
	}

	/// Idempotent. Turning a shortcut on inserts it at the front of its list.
	pub fn set(&mut self, kind: Shortcut, channel: Id, on: bool) -> PreferenceEdit {
		if channel.0 == 0 || !self.is_valid() || self.contains(kind, channel) == on {
			return PreferenceEdit::Unchanged;
		}
		let full = self.favorites.len() + self.pinned.len() + self.collapsed_categories.len()
			== Self::MAX_ENTRIES;
		let ids = match kind {
			Shortcut::Pinned => &mut self.pinned,
			Shortcut::Favorite => &mut self.favorites,
		};
		if on {
			if full {
				return PreferenceEdit::CapacityReached;
			}
			ids.insert(0, channel);
		} else {
			ids.retain(|id| *id != channel);
		}
		PreferenceEdit::Changed
	}

	pub fn category_collapsed(&self, category: Id) -> bool {
		self.collapsed_categories.contains(&category)
	}

	pub fn set_category_collapsed(&mut self, category: Id, collapsed: bool) -> PreferenceEdit {
		if category.0 == 0 || !self.is_valid() || self.category_collapsed(category) == collapsed {
			return PreferenceEdit::Unchanged;
		}
		if collapsed {
			if self.favorites.len() + self.pinned.len() + self.collapsed_categories.len()
				== Self::MAX_ENTRIES
			{
				return PreferenceEdit::CapacityReached;
			}
			self.collapsed_categories.push(category);
		} else {
			self.collapsed_categories.retain(|id| *id != category);
		}
		PreferenceEdit::Changed
	}
	/// Applies a whole visible category group atomically, preserving other servers' choices.
	pub fn set_categories_collapsed(
		&mut self,
		categories: &[Id],
		collapsed: bool,
	) -> PreferenceEdit {
		if !self.is_valid() || categories.iter().any(|id| id.0 == 0) {
			return PreferenceEdit::Unchanged;
		}
		if categories.len() > Self::MAX_ENTRIES {
			return PreferenceEdit::CapacityReached;
		}
		let mut next = self.collapsed_categories.clone();
		if collapsed {
			for category in categories {
				if !next.contains(category) {
					next.push(*category);
				}
			}
		} else {
			next.retain(|id| !categories.contains(id));
		}
		if self.favorites.len() + self.pinned.len() + next.len() > Self::MAX_ENTRIES {
			return PreferenceEdit::CapacityReached;
		}
		if next == self.collapsed_categories {
			return PreferenceEdit::Unchanged;
		}
		self.collapsed_categories = next;
		PreferenceEdit::Changed
	}

	/// Records the channel a server reopens to; returns whether anything changed.
	pub fn remember_channel(&mut self, guild: Id, channel: Id) -> bool {
		if guild.0 == 0 || channel.0 == 0 || self.last_channels.last() == Some(&(guild, channel)) {
			return false;
		}
		self.last_channels.retain(|(id, _)| *id != guild);
		if self.last_channels.len() >= Self::MAX_LAST_CHANNELS {
			self.last_channels.remove(0);
		}
		self.last_channels.push((guild, channel));
		true
	}

	/// Drops a confirmed-deleted channel from every local navigation list.
	pub fn forget(&mut self, channel: Id) -> bool {
		let count = |value: &Self| {
			value.favorites.len()
				+ value.pinned.len()
				+ value.collapsed_categories.len()
				+ value.last_channels.len()
		};
		let before = count(self);
		self.favorites.retain(|id| *id != channel);
		self.pinned.retain(|id| *id != channel);
		self.collapsed_categories.retain(|id| *id != channel);
		self.last_channels.retain(|(_, id)| *id != channel);
		before != count(self)
	}
}

#[cfg(test)]
mod category_tests {
	use super::*;
	#[test]
	fn collapsing_a_group_is_atomic_and_retains_other_servers() {
		let mut prefs = ChannelPreferences {
			collapsed_categories: vec![Id(10)],
			favorites: (100..354).map(Id).collect(),
			..Default::default()
		};
		let before = prefs.clone();
		assert_eq!(
			prefs.set_categories_collapsed(&[Id(20), Id(21)], true),
			PreferenceEdit::CapacityReached
		);
		assert_eq!(prefs, before);
		assert_eq!(
			prefs.set_categories_collapsed(&[Id(20), Id(20)], true),
			PreferenceEdit::Changed
		);
		assert!(prefs.is_valid());
		assert_eq!(
			prefs.set_categories_collapsed(&[Id(20)], false),
			PreferenceEdit::Changed
		);
		assert_eq!(prefs.collapsed_categories, [Id(10)]);
		assert_eq!(
			prefs.set_categories_collapsed(&[Id(0)], true),
			PreferenceEdit::Unchanged
		);
	}
}
