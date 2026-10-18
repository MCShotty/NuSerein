//! Explicit, bounded member moderation and one page of server bans.
use crate::{Id, User, server_admin::Member};
pub const MAX_BANS: usize = 100;
pub const MAX_BYTES: usize = 256 * 1024;
#[derive(Clone)]
pub struct Ban {
	pub user: User,
	pub reason: Option<String>,
}
#[derive(Clone)]
pub struct Bans {
	pub guild: Id,
	pub items: Vec<Ban>,
	pub next: Option<Id>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Change {
	Kick,
	Ban { delete_seconds: u32 },
	Timeout { until: Option<i128> },
}
#[derive(Clone)]
pub enum Action {
	Bans {
		after: Option<Id>,
	},
	Member(Id),
	Change {
		user: Id,
		change: Change,
		reason: String,
	},
	Unban {
		user: Id,
		reason: String,
	},
}
#[derive(Clone)]
pub enum Outcome {
	Bans(Bans),
	Member(Box<Member>),
	Changed {
		user: Id,
		member: Option<Box<Member>>,
	},
}
impl Action {
	pub fn write(&self) -> bool {
		matches!(self, Self::Change { .. } | Self::Unban { .. })
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Bans { after } => after.is_none_or(|id| id.0 != 0),
			Self::Member(user) => user.0 != 0,
			Self::Change {
				user,
				change,
				reason,
			} => {
				user.0 != 0
					&& crate::automod::valid_reason(reason)
					&& reason.capacity() <= 4096
					&& match change {
						Change::Ban { delete_seconds } => *delete_seconds <= 604800,
						Change::Timeout { until } => {
							until.is_none_or(|at| (0..=253_402_300_799_999_999_999).contains(&at))
						}
						Change::Kick => true,
					}
			}
			Self::Unban { user, reason } => {
				user.0 != 0 && crate::automod::valid_reason(reason) && reason.capacity() <= 4096
			}
		}
	}
}
impl Bans {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.items.capacity() * size_of::<Ban>()
			+ self
				.items
				.iter()
				.map(|row| row.user.heap_bytes() + row.reason.as_ref().map_or(0, String::capacity))
				.sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.items.len() <= MAX_BANS
			&& self.bytes() <= MAX_BYTES
			&& self.items.iter().enumerate().all(|(index, row)| {
				row.user.id.0 != 0
					&& row.user.heap_bytes() < 8192
					&& row
						.reason
						.as_ref()
						.is_none_or(|reason| reason.len() <= 2048 && reason.chars().count() <= 512)
					&& !self.items[..index]
						.iter()
						.any(|other| other.user.id == row.user.id)
			}) && self
			.next
			.is_none_or(|id| id.0 != 0 && self.items.last().is_none_or(|row| row.user.id <= id))
	}
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Bans(page) => page.bytes(),
				Self::Member(member) => member.bytes(),
				Self::Changed { member, .. } => member.as_ref().map_or(0, |member| member.bytes()),
			}
	}
	pub fn valid(&self) -> bool {
		self.bytes() <= MAX_BYTES
			&& match self {
				Self::Bans(page) => page.valid(),
				Self::Member(member) => member.valid(),
				Self::Changed { user, member } => {
					user.0 != 0
						&& member
							.as_ref()
							.is_none_or(|member| member.user.id == *user && member.valid())
				}
			}
	}
}
