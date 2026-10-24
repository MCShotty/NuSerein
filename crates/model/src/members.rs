//! Bounded member-list records.
use super::{
	ClientPlatforms, Freshness, Id, MAX_RICH_ACTIVITIES, RichActivity, User, valid_presence_text,
};

/// Only the active member pane is retained; group rows and unloaded slots remain None.
#[derive(Clone)]
pub struct Member {
	pub roles: Vec<Id>,
	pub user: User,
	pub nick: Option<String>,
	pub status: Option<String>,
	/// Custom status text with any unicode emoji; bounded, never a rich activity.
	pub custom_status: Option<String>,
	pub activities: Vec<RichActivity>,
	pub clients: ClientPlatforms,
}

impl Member {
	pub fn valid(&self) -> bool {
		self.user.id.0 != 0
			&& self
				.status
				.as_deref()
				.is_none_or(|status| matches!(status, "online" | "idle" | "dnd" | "offline"))
			&& self
				.custom_status
				.as_deref()
				.is_none_or(valid_presence_text)
			&& self.activities.len() <= MAX_RICH_ACTIVITIES
			&& self.activities.iter().all(RichActivity::valid)
	}
	/// Drops presence details this client cannot show, keeping the member row itself.
	pub fn sanitize_presence(&mut self) {
		if self
			.status
			.as_deref()
			.is_some_and(|status| !matches!(status, "online" | "idle" | "dnd" | "offline"))
		{
			self.status = None;
		}
		if self
			.custom_status
			.as_deref()
			.is_some_and(|text| !valid_presence_text(text))
		{
			self.custom_status = None;
		}
		self.activities.retain(RichActivity::valid);
		self.activities.truncate(MAX_RICH_ACTIVITIES);
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.roles.capacity() * size_of::<Id>()
			+ self.user.heap_bytes()
			+ self.nick.as_ref().map_or(0, String::capacity)
			+ self.status.as_ref().map_or(0, String::capacity)
			+ self.custom_status.as_ref().map_or(0, String::capacity)
			+ self.activities.capacity() * size_of::<RichActivity>()
			+ self
				.activities
				.iter()
				.map(RichActivity::heap_bytes)
				.sum::<usize>()
	}
}

#[derive(Clone)]
pub enum MemberSlot {
	Person(Member),
	/// Gateway group id: role snowflake, "online", or "offline".
	Group(String),
}

impl MemberSlot {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Person(member) => member.bytes(),
			Self::Group(id) => id.capacity(),
		}
	}
}

#[derive(Clone)]
pub struct MemberList {
	pub guild: Option<Id>,
	pub channel: Id,
	pub request: u64,
	/// Absolute index of `slots[0]`.
	pub start: usize,
	/// Contiguous window. None is a hole. At most 200 entries.
	pub slots: Vec<Option<MemberSlot>>,
	pub total: u64,
	/// Guild channel lazy list. Scrollbar length is `total`. DMs and threads are false and scroll `slots.len()`.
	pub lazy: bool,
	pub freshness: Freshness,
	/// id -> count from the update's top-level groups array. At most MAX_ROLES + 2.
	pub groups: Vec<(String, u64)>,
	/// Ranges last requested for a lazy guild list.
	pub ranges: Vec<[usize; 2]>,
}

impl MemberList {
	pub fn slot_bytes(&self) -> usize {
		self.slots.iter().flatten().map(MemberSlot::bytes).sum()
	}
}
