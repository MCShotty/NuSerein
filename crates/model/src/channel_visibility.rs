//! Unofficial, account-synchronized guild channel preferences. Unknown flag bits survive edits.
use crate::Id;

pub const FAVORITED: u64 = 1 << 11;
pub const OPT_IN_ENABLED: u64 = 1 << 12;
pub const UNREADS_ONLY_MENTIONS: u64 = 1 << 9;
pub const UNREADS_ALL_MESSAGES: u64 = 1 << 10;
pub const NEW_FORUM_THREADS_OFF: u64 = 1 << 13;
pub const NEW_FORUM_THREADS_ON: u64 = 1 << 14;
pub const OPT_IN_CHANNELS_OFF: u64 = 1 << 13;
pub const OPT_IN_CHANNELS_ON: u64 = 1 << 14;
pub const MAX_CHANGES: usize = 1024;
pub const CHANNEL_MASK: u64 = FAVORITED
	| OPT_IN_ENABLED
	| UNREADS_ONLY_MENTIONS
	| UNREADS_ALL_MESSAGES
	| NEW_FORUM_THREADS_OFF
	| NEW_FORUM_THREADS_ON;
pub const GUILD_MASK: u64 = OPT_IN_CHANNELS_OFF | OPT_IN_CHANNELS_ON;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowChange {
	pub channel: Id,
	pub flags: Option<(u64, u64)>,
	pub collapsed: Option<(bool, bool)>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
	pub guild_flags: Option<(u64, u64)>,
	pub channels: Vec<RowChange>,
}
impl Change {
	pub fn valid(&self) -> bool {
		self.channels.len() <= MAX_CHANGES
			&& self.channels.capacity() <= MAX_CHANGES
			&& (self.guild_flags.is_some() || !self.channels.is_empty())
			&& self.guild_flags.is_none_or(|(before, after)| {
				before != after
					&& (before ^ after) & !GUILD_MASK == 0
					&& after & GUILD_MASK != GUILD_MASK
			}) && self.channels.iter().enumerate().all(|(index, row)| {
			row.channel.0 != 0
				&& !self.channels[..index]
					.iter()
					.any(|other| other.channel == row.channel)
				&& (row.flags.is_some() || row.collapsed.is_some())
				&& row.flags.is_none_or(|(before, after)| {
					before != after && (before ^ after) & !CHANNEL_MASK == 0
				}) && row.collapsed.is_none_or(|(before, after)| before != after)
		})
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.channels.capacity() * size_of::<RowChange>()
	}
}
