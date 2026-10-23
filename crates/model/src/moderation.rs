//! Explicit, bounded member moderation and one page of server bans.
use crate::{Id, User, server_admin::Member};
pub const MAX_BANS: usize = 100;
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_BULK: usize = 100;
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
	BanSearch {
		query: String,
	},
	Review {
		user: Id,
		channel: Id,
		audit: bool,
	},
	BulkBan {
		users: Vec<Id>,
		delete_seconds: u32,
		reason: String,
	},
	BulkRefresh {
		users: Vec<Id>,
	},
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
	Review(Box<Review>),
	Bulk(BulkReport),
	Changed {
		user: Id,
		member: Option<Box<Member>>,
	},
}
impl Action {
	pub fn write(&self) -> bool {
		matches!(
			self,
			Self::Change { .. } | Self::Unban { .. } | Self::BulkBan { .. }
		)
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Bans { after } => after.is_none_or(|id| id.0 != 0),
			Self::Member(user) => user.0 != 0,
			Self::BanSearch { query } => {
				!query.trim().is_empty()
					&& query.chars().count() <= 32
					&& query.capacity() <= 128
					&& !query.chars().any(char::is_control)
			}
			Self::Review { user, channel, .. } => user.0 != 0 && channel.0 != 0,
			Self::BulkBan {
				users,
				delete_seconds,
				reason,
			} => {
				valid_targets(users)
					&& users.capacity() <= MAX_BULK * 2
					&& *delete_seconds <= 604800
					&& crate::automod::valid_reason(reason)
					&& reason.capacity() <= 4096
			}
			Self::BulkRefresh { users } => valid_targets(users) && users.capacity() <= MAX_BULK * 2,
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
				Self::Review(review) => review.bytes(),
				Self::Bulk(report) => report.bytes(),
				Self::Changed { member, .. } => member.as_ref().map_or(0, |member| member.bytes()),
			}
	}
	pub fn valid(&self) -> bool {
		self.bytes() <= MAX_BYTES
			&& match self {
				Self::Bans(page) => page.valid(),
				Self::Member(member) => member.valid(),
				Self::Review(review) => review.valid(),
				Self::Bulk(report) => report.valid(),
				Self::Changed { user, member } => {
					user.0 != 0
						&& member
							.as_ref()
							.is_none_or(|member| member.user.id == *user && member.valid())
				}
			}
	}
}

/// A submitted operation is pending until an exact-target Gateway completion arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanStatus {
	Pending,
	Banned,
	Failed,
	NotBanned,
	Unknown,
}
#[derive(Clone)]
pub struct BulkReport {
	pub users: Vec<(Id, BanStatus)>,
	pub accepted: bool,
}
impl BulkReport {
	pub fn pending(users: &[Id]) -> Self {
		Self {
			users: users.iter().map(|id| (*id, BanStatus::Pending)).collect(),
			accepted: true,
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.users.capacity() * size_of::<(Id, BanStatus)>()
	}
	pub fn valid(&self) -> bool {
		!self.users.is_empty()
			&& self.users.len() <= MAX_BULK
			&& self.users.capacity() <= MAX_BULK * 2
			&& self.users.iter().enumerate().all(|(index, (id, _))| {
				id.0 != 0 && !self.users[..index].iter().any(|(other, _)| other == id)
			})
	}
	pub fn matches(&self, users: &[Id]) -> bool {
		self.users.len() == users.len() && self.users.iter().all(|(id, _)| users.contains(id))
	}
}
pub fn valid_targets(users: &[Id]) -> bool {
	!users.is_empty()
		&& users.len() <= MAX_BULK
		&& users
			.iter()
			.enumerate()
			.all(|(index, id)| id.0 != 0 && !users[..index].contains(id))
}
#[derive(Clone)]
pub struct ReviewMessage {
	pub id: Id,
	pub channel: Id,
	pub excerpt: String,
}
#[derive(Clone)]
pub struct ReviewAudit {
	pub id: Id,
	pub actor: Option<Id>,
	pub action: u16,
	pub reason: Option<String>,
}
#[derive(Clone)]
pub struct Review {
	pub guild: Id,
	pub channel: Id,
	pub member: Member,
	pub roles: Vec<crate::server_admin::Role>,
	pub messages: Vec<ReviewMessage>,
	pub audit: Vec<ReviewAudit>,
	/// Inclusive oldest/newest audit IDs in the loaded server window, before member filtering.
	pub audit_window: Option<(Id, Id)>,
	pub messages_error: Option<&'static str>,
	pub audit_error: Option<&'static str>,
}
impl Review {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.member.bytes()
			+ self.roles.capacity() * size_of::<crate::server_admin::Role>()
			+ self
				.roles
				.iter()
				.map(|role| role.role.heap_bytes())
				.sum::<usize>()
			+ self.messages.capacity() * size_of::<ReviewMessage>()
			+ self
				.messages
				.iter()
				.map(|row| row.excerpt.capacity())
				.sum::<usize>()
			+ self.audit.capacity() * size_of::<ReviewAudit>()
			+ self
				.audit
				.iter()
				.map(|row| row.reason.as_ref().map_or(0, String::capacity))
				.sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.channel.0 != 0
			&& self.member.valid()
			&& self.bytes() <= MAX_BYTES
			&& self.roles.len() <= crate::permissions::MAX_ROLES
			&& self
				.roles
				.iter()
				.all(|role| role.role.id.0 != 0 && role.role.heap_bytes() <= 1024)
			&& self.messages.len() <= 25
			&& self.audit.len() <= 20
			&& self
				.audit_window
				.is_none_or(|(oldest, newest)| oldest.0 != 0 && oldest <= newest)
			&& self
				.messages
				.iter()
				.all(|row| row.id.0 != 0 && row.channel.0 != 0 && row.excerpt.len() <= 2048)
			&& self.audit.iter().all(|row| {
				row.id.0 != 0
					&& row
						.reason
						.as_ref()
						.is_none_or(|reason| reason.len() <= 2048)
			})
	}
}
#[cfg(test)]
mod bounds_tests {
	use super::*;
	#[test]
	fn bulk_actions_refuse_implicit_oversized_and_duplicate_selection() {
		let action = |users| Action::BulkBan {
			users,
			delete_seconds: 0,
			reason: String::new(),
		};
		assert!(!action(vec![]).valid());
		assert!(!action(vec![Id(7), Id(7)]).valid());
		assert!(action((1..=100).map(Id).collect()).valid());
		assert!(!action((1..=101).map(Id).collect()).valid());
		let report = BulkReport::pending(&[Id(7), Id(8)]);
		assert!(report.valid());
		assert_eq!(report.users[0].1, BanStatus::Pending);
		assert!(report.matches(&[Id(8), Id(7)]));
		assert!(!report.matches(&[Id(7)]));
	}
}
