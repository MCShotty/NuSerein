//! Bounded, read-only synchronization contracts. These results never replace a timeline.
use crate::auth::Failure;
use model::{Channel, CustomEmoji, GuildPatch, Id, Member, Message, Patch, Sticker};

pub const PAGE_MESSAGES: usize = 50;
pub const MAX_READ_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MEMBER_IDS: usize = 100;
pub const HISTORY_DAYS: i64 = 21;
pub const HISTORY_MILLIS: i64 = HISTORY_DAYS * 86_400_000;
pub const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
	Guild {
		guild: Id,
		user: Id,
	},
	Channels {
		guild: Id,
		user: Id,
	},
	Emojis {
		guild: Id,
	},
	Stickers {
		guild: Id,
	},
	Sounds {
		guild: Id,
	},
	History {
		channel: Id,
		before: Option<Id>,
		cutoff_ms: i64,
		end_ms: i64,
	},
	Archives {
		guild: Id,
		parent: Id,
		kind: model::archives::Kind,
		before: Option<model::archives::Cursor>,
	},
	ActiveThreads {
		guild: Id,
	},
	Members {
		guild: Id,
		users: Vec<Id>,
	},
}
impl Read {
	pub fn guild(&self) -> Option<Id> {
		match self {
			Self::Guild { guild, .. }
			| Self::Channels { guild, .. }
			| Self::Emojis { guild }
			| Self::Stickers { guild }
			| Self::Sounds { guild }
			| Self::Archives { guild, .. }
			| Self::ActiveThreads { guild }
			| Self::Members { guild, .. } => Some(*guild),
			Self::History { .. } => None,
		}
	}
	pub fn valid(&self) -> bool {
		if self.guild().is_some_and(|guild| guild.0 == 0) {
			return false;
		}
		match self {
			Self::Guild { user, .. } | Self::Channels { user, .. } => user.0 != 0,
			Self::History {
				channel,
				before,
				cutoff_ms,
				end_ms,
			} => {
				channel.0 != 0
					&& before.is_none_or(|id| id.0 != 0)
					&& *cutoff_ms >= DISCORD_EPOCH_MS
					&& end_ms >= cutoff_ms
					&& end_ms.saturating_sub(*cutoff_ms) <= HISTORY_MILLIS
					&& last_before(*end_ms).is_some()
			}
			Self::Archives { parent, .. } => parent.0 != 0,
			Self::Members { users, .. } => {
				!users.is_empty()
					&& users.len() <= MAX_MEMBER_IDS
					&& users.iter().all(|id| id.0 != 0)
					&& users.windows(2).all(|pair| pair[0] < pair[1])
			}
			_ => true,
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Members { users, .. } => users.capacity() * size_of::<Id>(),
				_ => 0,
			}
	}
}

/// Exclusive snowflake bound immediately after a frozen timestamp's millisecond.
pub fn last_before(end_ms: i64) -> Option<Id> {
	let elapsed = u64::try_from(end_ms.checked_sub(DISCORD_EPOCH_MS)?.checked_add(1)?).ok()?;
	if elapsed > u64::MAX >> 22 {
		return None;
	}
	Some(Id(elapsed << 22))
}
pub fn timestamp_ms(id: Id) -> i64 {
	(id.0 >> 22) as i64 + DISCORD_EPOCH_MS
}

/// Validate an actionable row without installing it into the visible conversation.
pub fn valid_history_message(message: &Message) -> bool {
	session_cache::Timeline::valid_message(message) && !message.ephemeral
}

pub struct GuildRead {
	pub display: GuildPatch,
	pub permissions: model::permissions::Snapshot,
	pub roles: model::server_roles::Catalog,
	/// A failed own-member read leaves previous membership unknown; it must never clear roles.
	pub self_member: Result<Member, Failure>,
	pub banner: Patch<String>,
	pub member_count: Option<u64>,
}
pub struct HistoryPage {
	pub channel: Id,
	/// Only rows inside the fixed interval, newest first.
	pub messages: Vec<Message>,
	/// The oldest raw row, including a row outside the requested interval.
	pub oldest: Option<Id>,
	pub boundary_reached: bool,
	pub channel_beginning: bool,
}
pub enum ReadResult {
	Guild(Box<GuildRead>),
	Channels {
		guild: Id,
		channels: Vec<Channel>,
		permissions: Vec<model::permissions::Channel>,
	},
	Emojis {
		guild: Id,
		items: Vec<CustomEmoji>,
	},
	Stickers {
		guild: Id,
		items: Vec<Sticker>,
	},
	Sounds {
		guild: Id,
		items: Vec<model::soundboard::Sound>,
	},
	History(HistoryPage),
	Archives(model::archives::Page),
	ActiveThreads {
		guild: Id,
		threads: Vec<Channel>,
	},
	Members {
		guild: Id,
		items: Vec<Member>,
		missing: Vec<Id>,
	},
}
impl ReadResult {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Guild(value) => {
					value.permissions.bytes()
						+ value.roles.bytes()
						+ match &value.self_member {
							Ok(member) => member.bytes(),
							Err(_) => 0,
						} + match &value.display.name {
						Patch::Value(name) => name.capacity(),
						_ => 0,
					} + match &value.display.icon {
						Patch::Value(icon) => icon.capacity(),
						_ => 0,
					} + match &value.banner {
						Patch::Value(banner) => banner.capacity(),
						_ => 0,
					}
				}
				Self::Channels {
					channels,
					permissions,
					..
				} => {
					channels.iter().map(Channel::bytes).sum::<usize>()
						+ channels.capacity().saturating_sub(channels.len()) * size_of::<Channel>()
						+ permissions
							.iter()
							.map(model::permissions::Channel::bytes)
							.sum::<usize>() + permissions.capacity().saturating_sub(permissions.len())
						* size_of::<model::permissions::Channel>()
				}
				Self::Emojis { items, .. } => model::custom_emoji_bytes(items),
				Self::Stickers { items, .. } => model::sticker_bytes(items),
				Self::Sounds { items, .. } => model::soundboard::sound_bytes(items),
				Self::History(page) => {
					page.messages.iter().map(Message::bytes).sum::<usize>()
						+ page.messages.capacity().saturating_sub(page.messages.len())
							* size_of::<Message>()
				}
				Self::Archives(page) => page.bytes(),
				Self::ActiveThreads { threads, .. } => {
					threads.iter().map(Channel::bytes).sum::<usize>()
						+ threads.capacity().saturating_sub(threads.len()) * size_of::<Channel>()
				}
				Self::Members { items, missing, .. } => {
					items.iter().map(Member::bytes).sum::<usize>()
						+ items.capacity().saturating_sub(items.len()) * size_of::<Member>()
						+ missing.capacity() * size_of::<Id>()
				}
			}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn interval_bounds_and_exact_member_ids_are_bounded() {
		assert_eq!(timestamp_ms(Id(1)), DISCORD_EPOCH_MS);
		assert_eq!(last_before(DISCORD_EPOCH_MS), Some(Id(1 << 22)));
		assert!(last_before(i64::MAX).is_none());
		let request = Read::History {
			channel: Id(1),
			before: None,
			cutoff_ms: DISCORD_EPOCH_MS,
			end_ms: DISCORD_EPOCH_MS + HISTORY_MILLIS,
		};
		assert!(request.valid());
		assert!(
			!Read::Members {
				guild: Id(1),
				users: vec![Id(2), Id(2)]
			}
			.valid()
		);
		assert!(
			!Read::Members {
				guild: Id(1),
				users: vec![]
			}
			.valid()
		);
	}
}
