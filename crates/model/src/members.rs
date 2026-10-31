//! Bounded member-list records.
use super::{
	ClientPlatforms, Freshness, Id, MAX_RICH_ACTIVITIES, Patch, RichActivity, User,
	valid_presence_text,
};

/// A bounded, persisted display observation. It carries no permissions, role assignments,
/// presence or membership proof; callers must validate current access before displaying it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberDisplay {
	pub guild: Id,
	pub user: Id,
	pub name: String,
	pub username: Option<Box<str>>,
	pub nick: Option<String>,
}
impl MemberDisplay {
	pub fn valid(&self) -> bool {
		let text = |s: &str| {
			s.len() <= 512 && s.chars().count() <= 128 && !s.chars().any(char::is_control)
		};
		self.guild.0 != 0
			&& self.user.0 != 0
			&& !self.name.is_empty()
			&& text(&self.name)
			&& self.username.as_deref().is_none_or(text)
			&& self.nick.as_deref().is_none_or(text)
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.name.capacity()
			+ self.username.as_ref().map_or(0, |s| s.len())
			+ self.nick.as_ref().map_or(0, String::capacity)
	}
}

/// Partial guild identity for display only. Never grants permissions. Missing fields leave
/// previous observations intact; a null nickname or an empty role list explicitly clears it.
#[derive(Clone, PartialEq, Eq)]
pub struct MemberIdentity {
	pub guild: Id,
	pub user: Id,
	pub profile: Option<User>,
	/// Server-specific avatar patch; this never changes the account's global avatar.
	pub avatar: Patch<String>,
	pub nick: Patch<String>,
	pub roles: Patch<Vec<Id>>,
	pub departed: bool,
}
impl MemberIdentity {
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.user.0 != 0
			&& self.profile.as_ref().is_none_or(|user| {
				user.id == self.user
					&& !user.webhook
					&& user.heap_bytes() <= 16 * 1024
					&& user
						.member_avatar
						.as_ref()
						.is_none_or(|avatar| avatar.guild == self.guild && avatar.valid())
			}) && match &self.avatar {
			Patch::Value(hash) => super::valid_avatar_hash(hash),
			_ => true,
		} && match &self.nick {
			Patch::Value(nick) => {
				nick.len() <= 512
					&& nick.chars().count() <= 128
					&& !nick.chars().any(char::is_control)
			}
			_ => true,
		} && match &self.roles {
			Patch::Value(roles) => {
				roles.len() <= super::permissions::MAX_MEMBER_ROLES
					&& roles.iter().all(|id| id.0 != 0)
					&& roles.windows(2).all(|ids| ids[0] < ids[1])
			}
			Patch::Absent => true,
			Patch::Null => false,
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.profile.as_ref().map_or(0, User::heap_bytes)
			+ match &self.avatar {
				Patch::Value(hash) => hash.capacity(),
				_ => 0,
			} + match &self.nick {
			Patch::Value(nick) => nick.capacity(),
			_ => 0,
		} + match &self.roles {
			Patch::Value(roles) => roles.capacity() * size_of::<Id>(),
			_ => 0,
		}
	}
	pub fn apply(&self, member: &mut Member) {
		if member.user.id != self.user || self.departed {
			return;
		}
		if let Some(profile) = &self.profile {
			// Member updates can carry only part of the user object. Profile cosmetics and
			// avatars remain owned by full snapshots; this path refreshes account names.
			member.user.name.clone_from(&profile.name);
			member.user.username.clone_from(&profile.username);
			if let Some(avatar) = &profile.member_avatar
				&& avatar.guild == self.guild
				&& avatar.valid()
			{
				member.user.member_avatar = Some(avatar.clone());
			}
		}
		match &self.avatar {
			Patch::Absent => {}
			Patch::Null => {
				member.user.member_avatar = Some(Box::new(super::GuildAvatar {
					guild: self.guild,
					hash: None,
				}))
			}
			Patch::Value(hash) => {
				member.user.member_avatar = Some(Box::new(super::GuildAvatar {
					guild: self.guild,
					hash: Some(hash.clone()),
				}))
			}
		}
		match &self.nick {
			Patch::Value(nick) => member.nick = (!nick.is_empty()).then(|| nick.clone()),
			Patch::Null => member.nick = None,
			Patch::Absent => {}
		}
		if let Patch::Value(roles) = &self.roles {
			member.roles.clone_from(roles);
		}
	}
}

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
// The bounded list stores people inline; boxing every loaded person adds an allocation
// per row. MemberList byte accounting includes this larger inline representation.
#[allow(clippy::large_enum_variant)]
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

/// Fields omitted by one member-list snapshot; explicit null/empty values are not missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingMemberFields {
	pub user: Id,
	pub nick: bool,
	pub roles: bool,
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
	/// Presence information for partial rows, bounded by the current slot count.
	pub missing_identity: Vec<MissingMemberFields>,
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
		self.slots
			.iter()
			.flatten()
			.map(MemberSlot::bytes)
			.sum::<usize>()
			+ self.missing_identity.capacity() * size_of::<MissingMemberFields>()
	}
}
