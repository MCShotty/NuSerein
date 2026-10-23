//! Bounded, session-only application review and server access controls.
use crate::{Channel, Id, User, server_admin::Member};

pub const MAX_APPLICATIONS: usize = 100;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024 - 4096;
pub const MAX_DETAIL_BYTES: usize = 32 * 1024;
pub const MAX_VIEW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
	#[default]
	Pending,
	Approved,
	Rejected,
}
impl Status {
	pub fn wire(self) -> &'static str {
		match self {
			Self::Pending => "SUBMITTED",
			Self::Approved => "APPROVED",
			Self::Rejected => "REJECTED",
		}
	}
}
#[derive(Clone, PartialEq, Eq)]
pub struct Answer {
	pub label: String,
	pub answer: String,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Application {
	pub id: Id,
	pub guild: Id,
	pub user_id: Id,
	pub user: Option<User>,
	pub status: Status,
	pub created_at: i128,
	pub reviewed_at: Option<i128>,
	pub moderator: Option<User>,
	pub reason: Option<String>,
	pub answers: Vec<Answer>,
	pub interview: Option<Id>,
}
impl Application {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.user.as_ref().map_or(0, User::heap_bytes)
			+ self.moderator.as_ref().map_or(0, User::heap_bytes)
			+ self.reason.as_ref().map_or(0, String::capacity)
			+ self.answers.capacity() * size_of::<Answer>()
			+ self
				.answers
				.iter()
				.map(|v| v.label.capacity() + v.answer.capacity())
				.sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.id.0 != 0
			&& self.guild.0 != 0
			&& self.user_id.0 != 0
			&& self.bytes() <= MAX_DETAIL_BYTES
			&& self
				.user
				.as_ref()
				.is_none_or(|u| u.id == self.user_id && u.heap_bytes() <= 8192)
			&& self
				.moderator
				.as_ref()
				.is_none_or(|u| u.id.0 != 0 && u.heap_bytes() <= 8192)
			&& self.reason.as_ref().is_none_or(|v| text(v, 160))
			&& self.answers.len() <= 5
			&& self
				.answers
				.iter()
				.all(|v| text(&v.label, 300) && text(&v.answer, 5000))
			&& [Some(self.created_at), self.reviewed_at]
				.into_iter()
				.all(|v| v.is_none_or(timestamp))
			&& self.interview.is_none_or(|id| id.0 != 0)
	}
}
#[derive(Clone)]
pub struct Applications {
	pub guild: Id,
	pub status: Status,
	pub items: Vec<Application>,
	pub total: Option<u64>,
	pub next: Option<Id>,
}
impl Applications {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.items.capacity() * size_of::<Application>()
			+ self
				.items
				.iter()
				.map(|v| v.bytes() - size_of::<Application>())
				.sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.items.len() <= MAX_APPLICATIONS
			&& self.bytes() <= MAX_PAGE_BYTES
			&& self.items.iter().enumerate().all(|(i, v)| {
				v.valid()
					&& v.guild == self.guild
					&& v.status == self.status
					&& !self.items[..i].iter().any(|other| other.id == v.id)
			}) && self
			.next
			.is_none_or(|id| self.items.last().is_some_and(|v| v.id == id))
	}
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Incident {
	/// UTC nanoseconds, as returned by the service; absence means not paused.
	pub invites_until: Option<i128>,
	pub dms_until: Option<i128>,
}
impl Incident {
	pub fn valid(&self) -> bool {
		[self.invites_until, self.dms_until]
			.into_iter()
			.all(|v| v.is_none_or(timestamp))
	}
	pub fn active(&self, now: i128) -> bool {
		[self.invites_until, self.dms_until]
			.into_iter()
			.flatten()
			.any(|until| until > now)
	}
	pub fn writable(&self, now: i128) -> bool {
		self.valid()
			&& [self.invites_until, self.dms_until]
				.into_iter()
				.all(|v| v.is_none_or(|until| until > now && until - now <= 86_400_000_000_000))
	}
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
	pub community: bool,
	pub rules: Option<Id>,
	pub updates: Option<Id>,
	pub alerts: Option<Id>,
}
impl Configuration {
	pub fn valid(&self) -> bool {
		[self.rules, self.updates, self.alerts]
			.into_iter()
			.all(|v| v.is_none_or(|id| id.0 != 0))
			&& (!self.community || self.rules.is_some() && self.updates.is_some())
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
	pub guild: Id,
	pub owner: Id,
	pub features: Vec<String>,
	pub configuration: Configuration,
	pub mfa_level: u8,
	pub verification_level: u8,
	pub content_filter: u8,
	/// Missing incident metadata remains unknown, never implicitly cleared.
	pub incident: Option<Incident>,
}
impl Settings {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.features.capacity() * size_of::<String>()
			+ self.features.iter().map(String::capacity).sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.owner.0 != 0
			&& self.bytes() <= 64 * 1024
			&& [
				self.configuration.rules,
				self.configuration.updates,
				self.configuration.alerts,
			]
			.into_iter()
			.all(|v| v.is_none_or(|id| id.0 != 0))
			&& self.mfa_level <= 1
			&& self.verification_level <= 4
			&& self.content_filter <= 2
			&& self.features.len() <= 256
			&& self.features.iter().all(|v| {
				!v.is_empty()
					&& v.len() <= 128
					&& v.bytes()
						.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
			}) && self.incident.is_none_or(|v| v.valid())
	}
}
#[derive(Clone)]
pub enum Action {
	Applications {
		status: Status,
		before: Option<Id>,
	},
	Application(Id),
	Decide {
		request: Id,
		status: Status,
		reason: String,
	},
	Interview {
		request: Id,
	},
	Settings,
	Incident {
		before: Incident,
		after: Incident,
	},
	Configure {
		before: Configuration,
		after: Configuration,
	},
	MfaLevel {
		before: u8,
		after: u8,
	},
	Recipient(Id),
	Transfer {
		before_owner: Id,
		recipient: Id,
	},
}
impl Action {
	pub fn write(&self) -> bool {
		matches!(
			self,
			Self::Decide { .. }
				| Self::Interview { .. }
				| Self::Incident { .. }
				| Self::Configure { .. }
				| Self::MfaLevel { .. }
				| Self::Transfer { .. }
		)
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Applications { before, .. } => before.is_none_or(|id| id.0 != 0),
			Self::Application(id) | Self::Recipient(id) => id.0 != 0,
			Self::Decide {
				request,
				status,
				reason,
			} => {
				request.0 != 0
					&& *status != Status::Pending
					&& text(reason, 160)
					&& reason.capacity() <= 640
					&& (*status == Status::Rejected || reason.is_empty())
			}
			Self::Interview { request } => request.0 != 0,
			Self::Settings => true,
			Self::Incident { before, after } => before.valid() && after.valid() && before != after,
			Self::Configure { before, after } => before != after && after.valid(),
			Self::MfaLevel { before, after } => *before <= 1 && *after <= 1 && before != after,
			Self::Transfer {
				before_owner,
				recipient,
			} => before_owner.0 != 0 && recipient.0 != 0 && before_owner != recipient,
		}
	}
}
#[derive(Clone)]
pub enum Outcome {
	Applications(Applications),
	Application(Application),
	Interview {
		application: Application,
		channel: Box<Channel>,
	},
	Settings(Settings),
	Recipient(Box<Member>),
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Applications(v) => v.bytes(),
				Self::Application(v) => v.bytes(),
				Self::Interview {
					application,
					channel,
				} => application.bytes() + channel.bytes(),
				Self::Settings(v) => v.bytes(),
				Self::Recipient(v) => v.bytes(),
			}
	}
	pub fn valid(&self) -> bool {
		self.bytes() <= MAX_PAGE_BYTES + 2048
			&& match self {
				Self::Applications(v) => v.valid(),
				Self::Application(v) => v.valid(),
				Self::Interview {
					application,
					channel,
				} => {
					application.valid()
						&& application.interview == Some(channel.id)
						&& channel.id == application.id
						&& channel.guild.is_none()
						&& channel.kind == 3
						&& channel.bytes() <= 64 * 1024
						&& channel.recipients.len() <= 64
						&& channel
							.recipients
							.iter()
							.any(|u| u.id == application.user_id)
				}
				Self::Settings(v) => v.valid(),
				Self::Recipient(v) => v.valid(),
			}
	}
}
pub fn timestamp(value: i128) -> bool {
	(0..=253_402_300_799_999_999_999).contains(&value)
}
fn text(value: &str, max: usize) -> bool {
	value.len() <= max * 4
		&& value.chars().count() <= max
		&& !value.chars().any(|c| c.is_control() && c != '\n')
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn incident_expiry_is_service_based_and_new_actions_are_limited_to_a_day() {
		let now = 1_000_000_000;
		assert!(
			Incident {
				invites_until: Some(now + 86_400_000_000_000),
				dms_until: None
			}
			.writable(now)
		);
		assert!(
			!Incident {
				invites_until: Some(now + 86_400_000_000_001),
				dms_until: None
			}
			.writable(now)
		);
		assert!(
			!Incident {
				invites_until: Some(now),
				dms_until: None
			}
			.active(now)
		);
		assert!(Incident::default().writable(now));
	}
	#[test]
	fn applications_require_explicit_supported_decisions_and_bounded_reasons() {
		assert!(
			Action::Decide {
				request: Id(1),
				status: Status::Rejected,
				reason: "Not a fit".into()
			}
			.valid()
		);
		assert!(
			!Action::Decide {
				request: Id(1),
				status: Status::Pending,
				reason: String::new()
			}
			.valid()
		);
		assert!(
			!Action::Decide {
				request: Id(1),
				status: Status::Approved,
				reason: "discarded".into()
			}
			.valid()
		);
		assert!(
			!Action::Transfer {
				before_owner: Id(1),
				recipient: Id(1)
			}
			.valid()
		);
	}
}
