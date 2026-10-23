//! One deliberate channel operation at a time; Discord remains authoritative.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id, Patch,
	permissions::{
		CREATE_PUBLIC_THREADS, MANAGE_CHANNELS, MANAGE_ROLES, MANAGE_THREADS, VIEW_CHANNEL,
	},
};

const MAX_CHANNEL_REFERENCE_NAMES: usize = 64;
const MAX_CHANNEL_REFERENCE_NAME_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
	pub name: String,
	pub topic: String,
	pub slowmode: u32,
	pub nsfw: bool,
	pub overwrites: Vec<model::permissions::Overwrite>,
	/// Forum and media channel settings; None for every other channel type.
	pub forum: Option<Box<ForumEdit>>,
	/// Voice/Stage settings; absent edits preserve them.
	pub voice: Option<Box<VoiceEdit>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceRegion {
	pub id: String,
	pub name: String,
	pub deprecated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceEdit {
	pub bitrate: u32,
	pub user_limit: u32,
	pub region: Option<String>,
	pub video_quality: u8,
	pub regions: Vec<VoiceRegion>,
}
impl VoiceEdit {
	pub fn valid(&self) -> bool {
		(8000..=384000).contains(&self.bitrate)
			&& self.user_limit <= 10000
			&& matches!(self.video_quality, 1 | 2)
			&& self.region.as_ref().is_none_or(|r| {
				!r.is_empty()
					&& r.len() <= 64
					&& r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
			}) && self.regions.len() <= 100
			&& self
				.regions
				.iter()
				.all(|r| r.id.len() <= 64 && r.name.len() <= 256)
			&& self.bytes() <= 64 * 1024
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.region.as_ref().map_or(0, String::capacity)
			+ self.regions.capacity() * size_of::<VoiceRegion>()
			+ self
				.regions
				.iter()
				.map(|r| r.id.capacity() + r.name.capacity())
				.sum::<usize>()
	}
	pub fn settings_eq(&self, other: &Self) -> bool {
		self.bitrate == other.bitrate
			&& self.user_limit == other.user_limit
			&& self.region == other.region
			&& self.video_quality == other.video_quality
	}
	pub fn conflicts(before: &Self, after: &Self, actual: &Self) -> bool {
		(before.bitrate != after.bitrate && before.bitrate != actual.bitrate)
			|| (before.user_limit != after.user_limit && before.user_limit != actual.user_limit)
			|| (before.region != after.region && before.region != actual.region)
			|| (before.video_quality != after.video_quality
				&& before.video_quality != actual.video_quality)
	}
	pub fn confirms(before: &Self, after: &Self, actual: &Self) -> bool {
		(before.bitrate == after.bitrate || after.bitrate == actual.bitrate)
			&& (before.user_limit == after.user_limit || after.user_limit == actual.user_limit)
			&& (before.region == after.region || after.region == actual.region)
			&& (before.video_quality == after.video_quality
				|| after.video_quality == actual.video_quality)
	}
}
/// Discord's forum tag name limit.
pub const TAG_NAME_LIMIT: usize = 20;
/// "Hide After Inactivity" choices, in minutes.
pub const HIDE_AFTER: [u32; 4] = [60, 1440, 4320, 10080];
/// Forum channel flag: new posts must carry a tag.
pub const REQUIRE_TAG: u64 = 1 << 4;

/// Forum and media channel settings, loaded and saved with the rest of the channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForumEdit {
	/// Tags in display order; `Id(0)` marks one the service has not created yet.
	pub tags: Vec<model::forum::Tag>,
	pub require_tag: bool,
	pub reaction: Option<model::ReactionEmoji>,
	/// Slowmode for messages inside posts (`default_thread_rate_limit_per_user`).
	pub message_slowmode: u32,
	pub layout: model::forum::Layout,
	pub sort: model::forum::Sort,
	pub match_all: bool,
	/// Minutes of inactivity before new posts hide (`default_auto_archive_duration`).
	pub hide_after: u32,
	/// Channel flags other than [`REQUIRE_TAG`], preserved on save.
	pub flags: u64,
}
impl ForumEdit {
	pub fn valid(&self) -> bool {
		self.tags.len() <= model::forum::MAX_TAGS
			&& self.tags.iter().enumerate().all(|(index, tag)| {
				valid_tag_name(&tag.name)
					&& tag.emoji_name.as_ref().is_none_or(|name| name.len() <= 128)
					&& (tag.id.0 == 0 || !self.tags[..index].iter().any(|other| other.id == tag.id))
			}) && self
			.reaction
			.as_ref()
			.is_none_or(model::ReactionEmoji::valid)
			&& self.message_slowmode <= 21600
			&& HIDE_AFTER.contains(&self.hide_after)
			&& self.flags & REQUIRE_TAG == 0
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.tags.capacity() * size_of::<model::forum::Tag>()
			+ self
				.tags
				.iter()
				.map(|tag| {
					tag.name.capacity() + tag.emoji_name.as_ref().map_or(0, String::capacity)
				})
				.sum::<usize>()
			+ self
				.reaction
				.as_ref()
				.and_then(|emoji| emoji.name.as_ref())
				.map_or(0, String::capacity)
	}
}
pub fn valid_tag_name(name: &str) -> bool {
	!name.trim().is_empty()
		&& name.chars().count() <= TAG_NAME_LIMIT
		&& !name.chars().any(char::is_control)
}
impl Edit {
	pub fn valid(&self) -> bool {
		self.forum.as_deref().is_none_or(ForumEdit::valid)
			&& self.voice.as_deref().is_none_or(VoiceEdit::valid)
			&& valid_name(&self.name)
			&& self.name.capacity() <= 400
			&& self.topic.chars().count() <= 4096
			&& self.topic.capacity() <= 16384
			&& self.slowmode <= 21600
			&& !self.topic.contains('\0')
			&& self.overwrites.capacity() <= model::permissions::MAX_OVERWRITES
			&& self.overwrites.iter().enumerate().all(|(index, row)| {
				row.id.0 != 0
					&& row.kind <= 1
					&& !self.overwrites[..index]
						.iter()
						.any(|other| other.id == row.id)
			})
	}
	pub fn bytes(&self) -> usize {
		self.name.capacity()
			+ self.topic.capacity()
			+ self.overwrites.capacity() * size_of::<model::permissions::Overwrite>()
			+ self.forum.as_deref().map_or(0, ForumEdit::bytes)
			+ self.voice.as_deref().map_or(0, VoiceEdit::bytes)
	}
}
pub fn valid_name(name: &str) -> bool {
	!name.trim().is_empty()
		&& name.chars().count() <= 100
		&& name.len() <= 400
		&& !name.chars().any(char::is_control)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mute {
	Unmute,
	For(u32),
	Forever,
}
impl Mute {
	pub const DURATIONS: [u32; 5] = [900, 3600, 10800, 28800, 86400];
	pub fn valid(self) -> bool {
		match self {
			Self::For(seconds) => Self::DURATIONS.contains(&seconds),
			_ => true,
		}
	}
	pub fn expires_at(self, now: i64) -> Option<i64> {
		match self {
			Self::For(seconds) if self.valid() => now.checked_add(i64::from(seconds)),
			_ => None,
		}
	}
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostDetails {
	pub owner: Option<Id>,
	pub archived: bool,
	pub locked: bool,
	pub pinned: bool,
	pub followed: bool,
	pub invitable: bool,
	pub muted: bool,
	pub level: u8,
	pub mute_until: Option<i64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreateKind {
	#[default]
	Text,
	Voice,
	Announcement,
	Forum,
}
impl CreateKind {
	pub fn wire_kind(self) -> u8 {
		match self {
			Self::Text => 0,
			Self::Voice => 2,
			Self::Announcement => 5,
			Self::Forum => 15,
		}
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
	Reference,
	Description,
	Publish(Id),
	FollowAnnouncement(Id),
	Load,
	PostLoad,
	PostFollow(bool),
	PostArchive(bool),
	PostLock(bool),
	PostRename(String),
	PostTags(Vec<Id>),
	PostAddMember(Id),
	PostRemoveMember(Id),
	PostPin(bool),
	PostMute(Mute),
	PostNotifications(u8),
	Edit {
		before: Edit,
		after: Edit,
	},
	Duplicate {
		name: String,
	},
	Create {
		name: String,
		kind: CreateKind,
	},
	/// Start a thread in this channel, optionally from one of its messages.
	CreateThread {
		name: String,
		message: Option<Id>,
	},
	CreatePrivateThread {
		name: String,
		invitable: bool,
	},
	CreateCategory {
		name: String,
	},
	Move {
		parent: Option<Id>,
		position: i32,
		lock_permissions: bool,
		shifts: Vec<(Id, i32)>,
	},
	Delete,
	Mute(Mute),
	Notifications(u8),
	HideMuted(bool),
	Personal(Box<model::channel_visibility::Change>),
}
impl Action {
	/// Reads never acquire an MFA continuation or enter the administrative write slot.
	pub fn write(&self) -> bool {
		!matches!(
			self,
			Self::Reference | Self::Description | Self::Load | Self::PostLoad
		)
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Personal(change) => change.valid(),
			Self::Publish(message) | Self::FollowAnnouncement(message) => message.0 != 0,
			Self::PostAddMember(user) | Self::PostRemoveMember(user) => user.0 != 0,
			Self::PostTags(tags) => {
				tags.len() <= 5
					&& tags.capacity() <= 8
					&& tags
						.iter()
						.enumerate()
						.all(|(index, tag)| tag.0 != 0 && !tags[..index].contains(tag))
			}
			Self::Edit { before, after } => before.valid() && after.valid(),
			Self::Move {
				parent,
				position,
				shifts,
				..
			} => {
				*position >= 0
					&& parent.is_none_or(|id| id.0 != 0)
					&& shifts.len() <= 100
					&& shifts.capacity() <= 128
					&& shifts.iter().all(|(id, pos)| id.0 != 0 && *pos >= 0)
			}
			Self::PostRename(name)
			| Self::CreatePrivateThread { name, .. }
			| Self::Duplicate { name }
			| Self::Create { name, .. }
			| Self::CreateCategory { name } => valid_name(name) && name.capacity() <= 400,
			Self::CreateThread { name, message } => {
				valid_name(name) && name.capacity() <= 400 && message.is_none_or(|id| id.0 != 0)
			}
			Self::Mute(Mute::For(seconds)) | Self::PostMute(Mute::For(seconds)) => {
				Mute::For(*seconds).valid()
			}
			Self::Notifications(level) | Self::PostNotifications(level) => *level <= 3,
			_ => true,
		}
	}
}
pub enum Outcome {
	Description(Box<str>),
	Published(Id),
	AnnouncementFollowed {
		destination: Id,
		webhook: Id,
	},
	Post {
		channel: Box<model::Channel>,
		details: PostDetails,
	},
	Details(Edit),
	Channel {
		channel: Box<model::Channel>,
		permissions: Option<model::permissions::Channel>,
	},
	Deleted,
	Moved,
	Preferences {
		muted: Option<bool>,
		level: Option<u8>,
		mute_until: Option<i64>,
	},
	HideMuted(bool),
	Personal(Box<model::channel_visibility::Change>),
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Details(edit) => edit.bytes(),
			Self::Description(value) => value.len(),
			Self::Personal(change) => change.bytes(),
			Self::Post { channel, .. } => channel.bytes(),
			Self::Channel {
				channel,
				permissions,
			} => {
				channel.bytes()
					+ permissions
						.as_ref()
						.and_then(|p| p.overwrites.as_ref())
						.map_or(0, |o| {
							o.capacity() * size_of::<model::permissions::Overwrite>()
						})
			}
			_ => 0,
		}
	}
}
pub enum Event {
	Finished {
		guild: Id,
		channel: Id,
		request: u64,
		result: Result<Outcome, Failure>,
	},
}
#[derive(Default)]
pub struct Actions {
	sequence: u64,
	pending: Option<(Id, Id, u64, Action, bool)>,
	details: Option<(Id, Edit)>,
	post: Option<(Id, PostDetails)>,
	/// The loaded post's owner. A thread's owner never changes, so this survives the live
	/// updates that invalidate the rest of the details, including the echo of a rename.
	post_owner: Option<(Id, Id)>,
	status: Option<(Id, &'static str, bool)>,
	reference_names: Vec<(Id, Box<str>)>,
	descriptions: Vec<(Id, Box<str>)>,
	retired_description: Option<(Id, Id, u64)>,
	created_thread: Option<(Id, Id, u64)>,
	completed: Option<(Id, u64, bool)>,
}
impl Actions {
	pub(crate) fn reset(&mut self) {
		*self = Self {
			sequence: self.sequence,
			completed: self.completed,
			status: self.status,
			..Self::default()
		};
	}
}
impl State {
	pub fn can_follow_announcement(&self, source: Id, destination: Id) -> bool {
		source != destination
			&& self
				.channel(source)
				.is_some_and(|channel| channel.guild.is_some() && channel.kind == 5)
			&& self.can_view(source)
			&& self
				.channel(destination)
				.is_some_and(|channel| channel.guild.is_some() && matches!(channel.kind, 0 | 5))
			&& self.permission(
				destination,
				VIEW_CHANNEL | model::permissions::MANAGE_WEBHOOKS,
			) == Some(true)
	}
	/// Publishing requires Send Messages; publishing another author's announcement also
	/// requires Manage Messages. Current channel, session and loaded message are rechecked.
	pub fn can_publish_message(&self, channel: Id, message: Id) -> bool {
		(self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
			&& self.selected == Some(channel)
			&& self.freshness == model::Freshness::Fresh
			&& self
				.channel(channel)
				.is_some_and(|entry| entry.kind == 5 && entry.guild.is_some())
			&& self.permission(
				channel,
				model::permissions::VIEW_CHANNEL | model::permissions::SEND_MESSAGES,
			) == Some(true)
			&& self.timeline.get(message).is_some_and(|entry| {
				entry.channel == channel
					&& matches!(entry.kind, 0 | 19)
					&& !entry.ephemeral
					&& !entry.unsupported
					&& entry.flags & 3 == 0
					&& !self.timeline.is_deleted(message)
					&& (self
						.user
						.as_ref()
						.is_some_and(|user| user.id == entry.author.id)
						|| self.permission(channel, model::permissions::MANAGE_MESSAGES)
							== Some(true))
			})
	}
	pub fn is_forum_post(&self, channel: Id) -> bool {
		self.channel(channel).is_some_and(|c| {
			c.kind == 11
				&& c.guild.is_some()
				&& c.parent_id
					.and_then(|id| self.channel(id))
					.is_some_and(|p| p.guild == c.guild && matches!(p.kind, 15 | 16))
		})
	}
	/// Any thread the session can act on: a forum post or a thread under a text channel.
	pub fn is_thread_channel(&self, channel: Id) -> bool {
		self.channel(channel).is_some_and(|c| {
			matches!(c.kind, 10..=12)
				&& c.guild.is_some()
				&& c.parent_id
					.and_then(|id| self.channel(id))
					.is_some_and(|p| p.guild == c.guild && matches!(p.kind, 0 | 5 | 15 | 16))
		})
	}
	pub fn post_details(&self, channel: Id) -> Option<&PostDetails> {
		self.channel_actions
			.post
			.as_ref()
			.filter(|(id, _)| {
				*id == channel && self.is_thread_channel(channel) && self.can_view(channel)
			})
			.map(|(_, p)| p)
	}
	pub fn can_manage_post(&self, channel: Id) -> bool {
		self.is_thread_channel(channel)
			&& self.permission(channel, VIEW_CHANNEL | MANAGE_THREADS) == Some(true)
	}
	pub fn can_edit_post(&self, channel: Id) -> bool {
		self.can_manage_post(channel)
			|| self
				.post_details(channel)
				.and_then(|p| p.owner)
				.or_else(|| {
					self.channel_actions
						.post_owner
						.filter(|(id, _)| {
							*id == channel
								&& self.is_thread_channel(channel)
								&& self.can_view(channel)
						})
						.map(|(_, owner)| owner)
				})
				.is_some_and(|owner| self.user.as_ref().is_some_and(|u| u.id == owner))
	}
	/// Whether a thread may be started in `channel`; the service stays authoritative.
	pub fn can_create_thread(&self, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|c| c.guild.is_some() && matches!(c.kind, 0 | 5))
			&& self.permission(channel, VIEW_CHANNEL | CREATE_PUBLIC_THREADS) == Some(true)
	}
	pub fn can_create_private_thread(&self, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|entry| entry.guild.is_some() && entry.kind == 0)
			&& self.permission(
				channel,
				VIEW_CHANNEL | model::permissions::CREATE_PRIVATE_THREADS,
			) == Some(true)
	}
	pub fn created_thread(&self, parent: Id, request: u64) -> Option<Id> {
		self.channel_actions
			.created_thread
			.filter(|(source, id, sequence)| {
				*source == parent && *sequence == request && self.channel(*id).is_some()
			})
			.map(|(_, id, _)| id)
	}
	pub fn can_change_post_tags(&self, channel: Id, tags: &[Id]) -> bool {
		let Some(post) = self
			.channel(channel)
			.filter(|_| self.is_forum_post(channel) && self.can_edit_post(channel))
		else {
			return false;
		};
		let Some(parent) = post.parent_id else {
			return false;
		};
		let Some(current) = post.tags.as_ref() else {
			return false;
		};
		self.post_details(channel)
			.is_some_and(|details| !details.archived)
			&& tags.len() <= 5
			&& (!self.forum_requires_tag(parent) || !tags.is_empty())
			&& tags.iter().enumerate().all(|(index, tag)| {
				!tags[..index].contains(tag)
					&& self.forum_tags(parent).iter().any(|known| {
						known.id == *tag
							&& (current.applied.contains(tag) || self.can_apply_tag(parent, known))
					})
			}) && current
			.applied
			.iter()
			.filter(|tag| !tags.contains(tag))
			.all(|tag| {
				self.forum_tags(parent)
					.iter()
					.find(|known| known.id == *tag)
					.is_some_and(|known| self.can_apply_tag(parent, known))
			})
	}
	pub fn can_add_thread_member(&self, channel: Id, user: Id) -> bool {
		user.0 != 0
			&& self.user.as_ref().is_some_and(|owner| owner.id != user)
			&& self.user_blocked(user) != Some(true)
			&& !self.members.as_ref().is_some_and(|members| {
				members.channel == channel
					&& members.freshness == model::Freshness::Fresh
					&& members.slots.iter().flatten().any(
						|slot| matches!(slot, model::MemberSlot::Person(member) if member.user.id == user),
					)
			}) && self.permission(
			channel,
			VIEW_CHANNEL | model::permissions::SEND_MESSAGES_IN_THREADS,
		) == Some(true)
			&& self.post_details(channel).is_some_and(|details| {
				!details.archived
					&& (self
						.channel(channel)
						.is_some_and(|thread| thread.kind != 12)
						|| self.can_manage_post(channel)
						|| (details.followed && details.invitable))
			})
	}
	pub fn can_remove_thread_member(&self, channel: Id, user: Id) -> bool {
		(self.can_manage_post(channel)
			|| (self
				.channel(channel)
				.is_some_and(|thread| thread.kind == 12)
				&& self.can_edit_post(channel)))
			&& self.user.as_ref().is_some_and(|owner| owner.id != user)
			&& self
				.post_details(channel)
				.is_some_and(|details| !details.archived)
			&& self.members.as_ref().is_some_and(|members| {
				members.channel == channel
					&& members.freshness == model::Freshness::Fresh
					&& members.slots.iter().flatten().any(
						|slot| matches!(slot, model::MemberSlot::Person(member) if member.user.id == user),
					)
			})
	}
	pub fn can_manage_channel(&self, channel: Id) -> bool {
		self.channel(channel)
			.is_some_and(|c| c.guild.is_some() && matches!(c.kind, 0 | 2 | 4 | 5 | 13 | 15 | 16))
			&& self.permission(channel, VIEW_CHANNEL | MANAGE_CHANNELS) == Some(true)
	}
	pub fn can_manage_channel_permissions(&self, channel: Id) -> bool {
		self.can_manage_channel(channel) && self.permission(channel, MANAGE_ROLES) == Some(true)
	}
	pub fn can_open_channel_settings(&self, channel: Id) -> bool {
		self.can_manage_channel(channel)
			|| self
				.channel(channel)
				.and_then(|channel| channel.guild)
				.is_some_and(|guild| self.can_manage_webhook_channel(guild, channel))
	}
	fn channel_move_allowed(
		&self,
		channel: Id,
		parent: Option<Id>,
		lock_permissions: bool,
		shifts: &[(Id, i32)],
	) -> bool {
		let Some(source) = self.channel(channel) else {
			return false;
		};
		if !self.can_manage_channel(channel) {
			return false;
		}
		if source.kind == 4 {
			if parent.is_some() || lock_permissions {
				return false;
			}
			return shifts.iter().all(|(id, _)| {
				self.channel(*id).is_some_and(|target| {
					target.kind == 4 && target.guild == source.guild && self.can_manage_channel(*id)
				})
			});
		}
		let valid_parent = parent.is_none_or(|id| {
			id != channel
				&& self.channel(id).is_some_and(|target| {
					target.kind == 4 && target.guild == source.guild && self.can_manage_channel(id)
				})
		});
		if !valid_parent || lock_permissions != (source.parent_id != parent && parent.is_some()) {
			return false;
		}
		shifts.iter().all(|(id, _)| {
			self.channel(*id)
				.is_some_and(|c| c.guild == source.guild && self.can_manage_channel(*id))
		})
	}
	pub fn can_edit_channel_permission(&self, channel: Id, bits: u128) -> bool {
		if !self.can_manage_channel_permissions(channel) {
			return false;
		}
		let Some(source) = self.channel(channel) else {
			return false;
		};
		let Some(guild) = source.guild.and_then(|id| self.permissions.guilds.get(&id)) else {
			return false;
		};
		let Some(user) = self.user.as_ref().map(|u| u.id) else {
			return false;
		};
		// Discord permits guild/parent bits, or any bit when the actor has an
		// applicable MANAGE_ROLES channel overwrite. Existing unknown bits stay intact.
		let elevated = self
			.permissions
			.channels
			.get(&channel)
			.and_then(|c| c.overwrites.as_ref())
			.is_some_and(|rows| {
				rows.iter().any(|row| {
					row.allow & MANAGE_ROLES != 0
						&& match row.kind {
							0 => {
								row.id == guild.id
									|| guild
										.member
										.as_ref()
										.is_some_and(|m| m.roles.contains(&row.id))
							}
							1 => row.id == user,
							_ => false,
						}
				})
			});
		if elevated {
			return true;
		}
		let now = Self::permission_time();
		let Some(mut available) = model::permissions::effective(guild, user, Some(&[]), now) else {
			return false;
		};
		if let Some(parent) = source
			.parent_id
			.and_then(|id| self.channel(id))
			.filter(|p| p.guild == Some(guild.id) && p.kind == 4)
			&& let Some(overwrites) = self
				.permissions
				.channels
				.get(&parent.id)
				.filter(|p| p.guild == guild.id)
				.and_then(|p| p.overwrites.as_deref())
			&& let Some(parent_bits) =
				model::permissions::effective(guild, user, Some(overwrites), now)
		{
			available |= parent_bits;
		}
		available & bits == bits
	}
	fn channel_edit_allowed(&self, channel: Id, before: &Edit, after: &Edit) -> bool {
		let kind = self.channel(channel).map(|c| c.kind);
		if before
			.voice
			.as_ref()
			.zip(after.voice.as_ref())
			.is_some_and(|(before, after)| !before.settings_eq(after))
		{
			let Some(voice) = &after.voice else {
				return false;
			};
			if !matches!(kind, Some(2 | 13))
				|| (kind == Some(13) && voice.bitrate > 64000)
				|| (kind == Some(2) && voice.user_limit > 99)
			{
				return false;
			}
		}
		let overview = before.name != after.name
			|| before.topic != after.topic
			|| before.slowmode != after.slowmode
			|| before.nsfw != after.nsfw;
		if before.topic != after.topic
			&& after.topic.chars().count()
				> if matches!(kind, Some(15 | 16)) {
					4096
				} else {
					1024
				} {
			return false;
		}
		if overview && !self.can_manage_channel(channel) {
			return false;
		}
		if before.topic != after.topic && !matches!(kind, Some(0 | 5 | 15 | 16)) {
			return false;
		}
		if (before.slowmode != after.slowmode || before.nsfw != after.nsfw)
			&& !matches!(kind, Some(0 | 2 | 5 | 13 | 15 | 16))
		{
			return false;
		}
		if before.overwrites != after.overwrites {
			if !self.can_manage_channel_permissions(channel) {
				return false;
			}
			for row in before.overwrites.iter().chain(&after.overwrites) {
				let old = before
					.overwrites
					.iter()
					.find(|o| o.id == row.id && o.kind == row.kind);
				let new = after
					.overwrites
					.iter()
					.find(|o| o.id == row.id && o.kind == row.kind);
				let changed = old.map_or(0, |o| o.allow) ^ new.map_or(0, |o| o.allow)
					| (old.map_or(0, |o| o.deny) ^ new.map_or(0, |o| o.deny));
				if !self.can_edit_channel_permission(channel, changed) {
					return false;
				}
			}
		}
		self.can_manage_channel(channel)
	}
	pub fn channel_action_pending(&self) -> bool {
		self.channel_actions.pending.is_some()
	}
	pub fn channel_details(&self, channel: Id) -> Option<&Edit> {
		self.channel_actions
			.details
			.as_ref()
			.filter(|(id, _)| *id == channel && self.can_open_channel_settings(channel))
			.map(|(_, edit)| edit)
	}
	pub fn channel_action_status(&self, channel: Id) -> Option<&'static str> {
		self.channel_actions
			.status
			.filter(|(id, _, _)| *id == channel)
			.map(|(_, status, _)| status)
	}
	pub fn channel_action_succeeded(&self, channel: Id) -> bool {
		self.channel_actions
			.status
			.is_some_and(|(id, _, success)| id == channel && success)
	}
	pub fn channel_action_completed(&self, channel: Id, request: u64) -> Option<bool> {
		self.channel_actions
			.completed
			.filter(|(target, sequence, _)| *target == channel && *sequence == request)
			.map(|(_, _, success)| success)
	}
	pub fn clear_channel_action_result(&mut self, channel: Id) {
		if self
			.channel_actions
			.status
			.is_some_and(|(id, _, _)| id == channel)
		{
			self.channel_actions.status = None;
		}
	}
	fn channel_action_allowed(&self, guild: Id, channel: Id, action: &Action) -> bool {
		if self
			.channel(channel)
			.is_none_or(|source| source.guild != Some(guild))
		{
			return false;
		}
		let personal = matches!(
			action,
			Action::Mute(_) | Action::Notifications(_) | Action::HideMuted(_) | Action::Personal(_)
		);
		action.valid()
			&& !matches!(action, Action::Personal(change) if !self.personal_change_allowed(guild, change, false))
			&& (self.can_view(channel) || personal && self.can_personally_manage_channel(channel))
			&& (personal
				|| match action {
					Action::Reference => false,
					Action::Description => self.can_view(channel),
					Action::Publish(message) => self.can_publish_message(channel, *message),
					Action::FollowAnnouncement(destination) => {
						self.can_follow_announcement(channel, *destination)
					}
					Action::Load => self.can_open_channel_settings(channel),
					Action::PostLoad => self.is_thread_channel(channel),
					Action::PostFollow(_) => {
						self.post_details(channel).is_some_and(|p| !p.archived)
					}
					Action::PostArchive(archived) => {
						self.can_edit_post(channel)
							&& self.post_details(channel).is_some_and(|p| {
								*archived || !p.locked || self.can_manage_post(channel)
							})
					}
					Action::PostRename(_) => self.can_edit_post(channel),
					Action::PostTags(tags) => self.can_change_post_tags(channel, tags),
					Action::PostAddMember(user) => self.can_add_thread_member(channel, *user),
					Action::PostRemoveMember(user) => self.can_remove_thread_member(channel, *user),
					Action::CreatePrivateThread { .. } => self.can_create_private_thread(channel),
					Action::CreateThread { message, .. } => {
						self.can_create_thread(channel)
							&& message.is_none_or(|id| {
								self.timeline.get(id).is_some_and(|m| m.channel == channel)
							})
					}
					Action::PostPin(_) | Action::PostLock(_) => self.can_manage_post(channel),
					Action::PostMute(_) | Action::PostNotifications(_) => {
						self.post_details(channel).is_some_and(|p| p.followed)
					}
					Action::Delete if self.is_thread_channel(channel) => {
						self.can_manage_post(channel)
					}
					Action::Edit { before, after } => {
						self.channel_edit_allowed(channel, before, after)
					}
					Action::Move {
						parent,
						lock_permissions,
						shifts,
						..
					} => self.channel_move_allowed(channel, *parent, *lock_permissions, shifts),
					_ => self.can_manage_channel(channel),
				})
	}
	pub(crate) fn channel_action_mfa_allowed(
		&self,
		guild: Id,
		channel: Id,
		request: u64,
		action: &Action,
	) -> bool {
		self.channel_actions
			.pending
			.as_ref()
			.is_some_and(|(g, c, r, pending, _)| {
				(*g, *c, *r) == (guild, channel, request) && pending == action
			}) && action.write()
			&& self.channel_action_allowed(guild, channel, action)
			&& !matches!(action,Action::Edit {before,..} if self.channel_details(channel) != Some(before))
			&& !matches!(action,Action::Delete if self.pending.iter().any(|p|p.channel == channel)
				|| self.voice.active.as_ref().is_some_and(|call|call.channel == channel))
	}
	pub fn request_channel_action(&mut self, channel: Id, action: Action) -> Option<Command> {
		// Explicit actions supersede one optional topic read. Its late result is retired,
		// and no further automatic read can start until it completes: at most one retired read.
		if !matches!(action, Action::Description)
			&& self.channel_description_pending()
			&& self.channel_actions.retired_description.is_none()
			&& let Some((guild, channel, request, _, _)) = self.channel_actions.pending.take()
		{
			self.channel_actions.retired_description = Some((guild, channel, request));
		}
		if matches!(action, Action::Description)
			&& self.channel_actions.retired_description.is_some()
		{
			return None;
		}
		if self.channel_action_pending() {
			return None;
		}
		self.clear_channel_action_result(channel);
		let source = self.channel(channel)?;
		let guild = source.guild?;
		if !self.channel_action_allowed(guild, channel, &action) {
			self.channel_actions.status =
				Some((channel, "Channel action unavailable or invalid", false));
			return None;
		}
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			self.channel_actions.status = Some((
				channel,
				"Channel actions unavailable while disconnected",
				false,
			));
			return None;
		}
		if matches!(action, Action::Delete)
			&& (self.pending.iter().any(|p| p.channel == channel)
				|| self
					.voice
					.active
					.as_ref()
					.is_some_and(|call| call.channel == channel))
		{
			self.channel_actions.status = Some((
				channel,
				"Finish pending messages and leave the call before deleting this channel",
				false,
			));
			return None;
		}
		if let Action::Edit { before, .. } = &action
			&& self.channel_details(channel) != Some(before)
		{
			self.channel_actions.status = Some((
				channel,
				"Channel settings changed; reopen the editor",
				false,
			));
			return None;
		}
		if matches!(action, Action::PostLoad) && !self.demo {
			self.channel_actions.post = None;
		}
		if matches!(action, Action::Load) && !self.demo {
			self.channel_actions.details = None;
		}
		if let Action::HideMuted(hide) = &action
			&& let Err(status) = self.confirm_guild_hides_muted(guild, *hide)
		{
			self.channel_actions.status = Some((channel, status, false));
			self.status = status;
			return None;
		}
		self.channel_actions.sequence = self.channel_actions.sequence.wrapping_add(1);
		let request = self.channel_actions.sequence;
		self.channel_actions.pending = Some((guild, channel, request, action.clone(), false));
		Some(Command::ChannelAction {
			guild,
			channel,
			request,
			action,
		})
	}
	pub fn request_channel_reference(&mut self, guild: Id, channel: Id) -> Option<Command> {
		if self.channel_action_pending()
			|| self.channel(channel).is_some()
			|| self.channel_action_status(channel).is_some()
			|| self.guild(guild).is_none()
			|| self
				.selected
				.and_then(|id| self.channel(id))
				.and_then(|source| source.guild)
				!= Some(guild)
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return None;
		}
		self.channel_actions.sequence = self.channel_actions.sequence.wrapping_add(1);
		let request = self.channel_actions.sequence;
		let action = Action::Reference;
		self.channel_actions.pending = Some((guild, channel, request, action.clone(), false));
		Some(Command::ChannelAction {
			guild,
			channel,
			request,
			action,
		})
	}
	pub fn channel_description_pending(&self) -> bool {
		self.channel_actions
			.pending
			.as_ref()
			.is_some_and(|(_, _, _, action, _)| matches!(action, Action::Description))
	}
	pub fn channel_description_bytes(&self) -> usize {
		self.channel_actions.descriptions.capacity() * size_of::<(Id, Box<str>)>()
			+ self
				.channel_actions
				.descriptions
				.iter()
				.map(|(_, value)| value.len())
				.sum::<usize>()
	}
	pub fn personal_channel_dispatch_allowed(
		&self,
		guild: Id,
		channel: Id,
		request: u64,
		action: &Action,
	) -> bool {
		self.channel_actions
			.pending
			.as_ref()
			.is_some_and(|(g, c, r, pending, _)| {
				(*g, *c, *r) == (guild, channel, request) && pending == action
			}) && self.can_personally_manage_channel(channel)
			&& matches!(action,Action::Personal(change) if self.personal_change_allowed(guild,change,false))
	}
	pub fn channel_description(&self, id: Id) -> Option<&str> {
		self.can_view(id)
			.then(|| {
				self.channel_actions
					.descriptions
					.iter()
					.find(|(known, _)| *known == id)
					.map(|(_, value)| value.as_ref())
			})
			.flatten()
	}
	pub fn channel_reference_name(&self, id: Id) -> Option<&str> {
		self.channel(id)
			.filter(|channel| {
				channel.guild.is_some() && matches!(channel.kind, 0 | 5 | 10..=12 | 15 | 16)
			})
			.map(|channel| channel.name.as_str())
			.or_else(|| {
				self.channel_actions
					.reference_names
					.iter()
					.find(|(known, _)| *known == id)
					.map(|(_, name)| name.as_ref())
			})
	}
	fn remember_channel_reference_name(&mut self, channel: &model::Channel) {
		self.forget_channel_reference_name(channel.id);
		if channel.name.len() > MAX_CHANNEL_REFERENCE_NAME_BYTES {
			return;
		}
		while self.channel_actions.reference_names.len() >= MAX_CHANNEL_REFERENCE_NAMES
			|| self
				.channel_actions
				.reference_names
				.iter()
				.map(|(_, name)| name.len())
				.sum::<usize>()
				+ channel.name.len()
				> MAX_CHANNEL_REFERENCE_NAME_BYTES
		{
			self.channel_actions.reference_names.remove(0);
		}
		self.channel_actions
			.reference_names
			.push((channel.id, channel.name.clone().into_boxed_str()));
	}
	pub(crate) fn forget_channel_reference_name(&mut self, id: Id) {
		self.channel_actions
			.reference_names
			.retain(|(known, _)| *known != id);
	}
	pub(crate) fn cancel_channel_action(&mut self) {
		if let Some((_, channel, request, action, _)) = self.channel_actions.pending.take() {
			self.channel_actions.completed = Some((channel, request, false));
			self.channel_actions.status = Some((
				channel,
				if matches!(
					action,
					Action::Reference | Action::Description | Action::Load | Action::PostLoad
				) {
					"Channel load interrupted"
				} else {
					Failure::Ambiguous.label()
				},
				false,
			));
		}
		self.channel_actions.details = None;
		self.channel_actions.post = None;
	}
	pub(crate) fn observe_channel_action(&mut self, event: &crate::Event) {
		match event {
			crate::Event::ChannelChanged(p) | crate::Event::ThreadChanged { patch: p, .. } => self
				.channel_actions
				.descriptions
				.retain(|(id, _)| *id != p.id),
			crate::Event::Unavailable(channel) => self
				.channel_actions
				.descriptions
				.retain(|(id, _)| id != channel),
			crate::Event::Permissions(_) => self.channel_actions.descriptions.clear(),
			_ => {}
		}
		if let Some((channel, _)) = &self.channel_actions.post {
			let invalid = match event {
				crate::Event::ChannelChanged(p) | crate::Event::ThreadChanged { patch: p, .. } => {
					p.id == *channel
				}
				crate::Event::Unavailable(id) => id == channel,
				crate::Event::ChannelCreated(c) => c.id == *channel,
				_ => false,
			};
			if invalid {
				self.channel_actions.post = None;
			}
		}
		if let Some((channel, _)) = &self.channel_actions.details {
			let invalid = match event {
				crate::Event::ChannelChanged(p) | crate::Event::ThreadChanged { patch: p, .. } => {
					p.id == *channel
				}
				crate::Event::Unavailable(id) => id == channel,
				crate::Event::ChannelCreated(c) => c.id == *channel,
				crate::Event::Permissions(crate::permissions::Event::Channel {
					channel: id,
					..
				}) => id == channel,
				_ => false,
			};
			if invalid {
				self.channel_actions.details = None;
			}
		}
		if let Some((guild, channel, _, action, changed)) = &mut self.channel_actions.pending {
			*changed |= match event {
				crate::Event::ChannelChanged(p) | crate::Event::ThreadChanged { patch: p, .. } => {
					p.id == *channel && !matches!(action, Action::Delete)
				}
				crate::Event::Unavailable(id) => id == channel && !matches!(action, Action::Delete),
				crate::Event::ChannelCreated(c) => c.id == *channel,
				crate::Event::Permissions(crate::permissions::Event::Channel {
					channel: id,
					..
				}) => id == channel && !matches!(action, Action::Delete),
				crate::Event::NotificationPreferences(crate::notifications::Event::Settings {
					entries,
					..
				}) if matches!(
					action,
					Action::Mute(_)
						| Action::Notifications(_)
						| Action::HideMuted(_)
						| Action::Personal(_)
				) =>
				{
					entries.iter().any(|s| s.guild == Some(*guild))
				}
				_ => false,
			};
		}
	}
	pub(crate) fn apply_channel_action(&mut self, event: Event) -> Result<(), &'static str> {
		let Event::Finished {
			guild,
			channel,
			request,
			result,
		} = event;
		if self.channel_actions.retired_description == Some((guild, channel, request)) {
			self.channel_actions.retired_description = None;
			return Ok(());
		}
		let Some((g, c, r, _, _)) = &self.channel_actions.pending else {
			return Ok(());
		};
		if (*g, *c, *r) != (guild, channel, request) {
			return Ok(());
		}
		let (_, _, _, action, observed) = self.channel_actions.pending.take().unwrap();
		self.channel_actions.completed = Some((channel, request, false));
		let result = result.and_then(|outcome| {
			let valid = match (&action, &outcome) {
				(
					Action::Reference,
					Outcome::Channel {
						channel: target, ..
					},
				) => {
					target.id == channel
						&& target.guild == Some(guild)
						&& matches!(target.kind, 10..=12)
						&& target.parent_id.is_some_and(|parent| {
							self.channel(parent).is_some_and(|source| {
								source.guild == Some(guild) && self.can_view(parent)
							})
						})
				}
				(
					Action::PostLoad
					| Action::PostFollow(_)
					| Action::PostArchive(_)
					| Action::PostLock(_)
					| Action::PostRename(_)
					| Action::PostTags(_)
					| Action::PostAddMember(_)
					| Action::PostRemoveMember(_)
					| Action::PostPin(_)
					| Action::PostMute(_)
					| Action::PostNotifications(_),
					Outcome::Post {
						channel: target,
						details,
					},
				) => {
					target.id == channel
						&& target.guild == Some(guild)
						&& matches!(target.kind, 10..=12)
						&& self
							.channel(channel)
							.is_some_and(|c| c.parent_id == target.parent_id)
						&& valid_name(&target.name)
						&& details.owner.is_none_or(|id| id.0 != 0)
						&& details.level <= 3
						&& match &action {
							Action::PostFollow(value) => details.followed == *value,
							Action::PostArchive(value) => details.archived == *value,
							Action::PostLock(value) => details.locked == *value,
							Action::PostPin(value) => details.pinned == *value,
							Action::PostRename(value) => target.name == *value,
							Action::PostTags(value) => target.tags.as_ref().is_some_and(|tags| {
								tags.applied.len() == value.len()
									&& tags.applied.iter().all(|tag| value.contains(tag))
							}),
							Action::PostMute(value) => details.muted == (*value != Mute::Unmute),
							Action::PostNotifications(value) => details.level == *value,
							_ => true,
						}
				}
				(Action::Load, Outcome::Details(edit)) => edit.valid(),
				(Action::Publish(message), Outcome::Published(actual)) => message == actual,
				(
					Action::FollowAnnouncement(target),
					Outcome::AnnouncementFollowed {
						destination,
						webhook,
					},
				) => target == destination && webhook.0 != 0,
				(Action::Edit { .. }, Outcome::Channel { channel: c, .. }) => {
					c.id == channel && c.guild == Some(guild)
				}
				(
					Action::Duplicate { .. }
					| Action::Create { .. }
					| Action::CreateCategory { .. }
					| Action::CreateThread { .. },
					Outcome::Channel { channel: c, .. },
				)
				| (Action::CreatePrivateThread { .. }, Outcome::Channel { channel: c, .. }) => {
					c.id.0 != 0
						&& c.id != channel && c.guild == Some(guild)
						&& match &action {
							Action::Create { kind, .. } => c.kind == kind.wire_kind(),
							Action::CreateCategory { .. } => c.kind == 4,
							Action::CreateThread { name, message } => {
								c.kind
									== if self
										.channel(channel)
										.is_some_and(|parent| parent.kind == 5)
									{
										10
									} else {
										11
									} && c.parent_id == Some(channel)
									&& c.name.trim() == name.trim()
									&& message.is_none_or(|message| c.id == message)
							}
							Action::CreatePrivateThread { name, .. } => {
								c.kind == 12
									&& c.parent_id == Some(channel)
									&& c.name.trim() == name.trim()
							}
							_ => true,
						}
				}
				(Action::Description, Outcome::Description(value)) => {
					value.len() <= 2048 && value.chars().count() <= 512
				}
				(Action::Delete, Outcome::Deleted) => true,
				(Action::Move { .. }, Outcome::Moved) => true,
				(Action::Mute(mute), Outcome::Preferences { muted, .. }) => {
					*muted == Some(*mute != Mute::Unmute)
				}
				(Action::Notifications(wanted), Outcome::Preferences { level, .. }) => {
					*level == Some(*wanted)
				}
				(Action::HideMuted(hide), Outcome::HideMuted(actual)) => hide == actual,
				(Action::Personal(wanted), Outcome::Personal(actual)) => wanted == actual,
				_ => false,
			};
			if valid && outcome.bytes() <= 64 * 1024 {
				Ok(outcome)
			} else {
				Err(Failure::Ambiguous)
			}
		});
		let status = match result {
			Err(failure) => {
				if matches!(
					action,
					Action::PostTags(_) | Action::PostAddMember(_) | Action::PostRemoveMember(_)
				) {
					// A failed readback can follow a successful write. Require an explicit
					// current-details read before enabling another membership/tag change.
					self.channel_actions.post = None;
					if matches!(
						action,
						Action::PostAddMember(_) | Action::PostRemoveMember(_)
					) && self
						.members
						.as_ref()
						.is_some_and(|members| members.channel == channel)
					{
						self.invalidate_members();
						self.members = None;
					}
				}
				if let Action::HideMuted(hide) = &action
					&& !observed
				{
					let _ = self.confirm_guild_hides_muted(guild, !*hide);
				}
				if failure.ends_session() {
					self.fail(failure);
				}
				self.channel_actions.status = Some((channel, failure.label(), false));
				self.status = failure.label();
				return Ok(());
			}
			Ok(Outcome::Description(value)) => {
				if !observed && self.can_view(channel) {
					self.channel_actions
						.descriptions
						.retain(|(id, _)| *id != channel);
					while self.channel_actions.descriptions.len() >= 64
						|| self
							.channel_actions
							.descriptions
							.iter()
							.map(|(_, value)| value.len())
							.sum::<usize>() + value.len()
							> 64 * 1024
					{
						self.channel_actions.descriptions.remove(0);
					}
					self.channel_actions.descriptions.push((channel, value));
				}
				self.channel_actions.completed = Some((channel, request, true));
				return Ok(());
			}
			Ok(Outcome::Published(message)) => {
				self.confirm_message_published(channel, message);
				"Announcement published"
			}
			Ok(Outcome::AnnouncementFollowed { .. }) => {
				"Announcement channel followed; manage it in the destination server's integrations"
			}
			Ok(Outcome::Post {
				channel: updated,
				details,
			}) => {
				if observed
					&& matches!(action, Action::PostRename(_))
					&& self.is_thread_channel(channel)
					&& self.can_view(channel)
					&& self
						.channel(channel)
						.is_some_and(|c| c.name == updated.name)
				{
					// The name confirms the rename, but a same-name update can change other details.
					self.channel_actions.post = None;
				} else if self.is_thread_channel(channel) && self.can_view(channel) && !observed {
					if self
						.channel(channel)
						.is_some_and(|c| c.name != updated.name || c.tags != updated.tags)
					{
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::ChannelChanged(model::ChannelPatch {
								id: channel,
								name: Patch::Value(updated.name.clone()),
								icon: Patch::Absent,
								last_message: Patch::Absent,
								parent_id: Patch::Absent,
								position: Patch::Absent,
								kind: Patch::Absent,
								message_count: Patch::Absent,
								tags: updated.tags.clone().map_or(Patch::Null, Patch::Value),
							}),
						});
					}
					if !details.archived && self.archived_thread == Some(channel) {
						self.archived_thread = None;
					}
					if let Some(view) = &mut self.archives
						&& let Some(page) = &mut view.page
					{
						if !details.archived {
							page.threads.retain(|c| c.id != channel);
							if page.threads.is_empty() {
								page.next = None;
							}
						} else if let Some(index) =
							page.threads.iter().position(|c| c.id == channel)
						{
							if page.bytes() - page.threads[index].name.capacity()
								+ updated.name.len() <= model::archives::MAX_BYTES
							{
								page.threads[index].name = updated.name.clone();
							} else {
								view.page = None;
							}
						}
					}
					if let Err(status) = self
						.confirm_channel_preferences(
							guild,
							channel,
							Some(details.muted),
							Some(details.level),
						)
						.and_then(|_| {
							self.confirm_channel_mute_timer(
								channel,
								Some(details.muted),
								details.mute_until,
							)
						}) {
						self.channel_actions.status = Some((channel, status, false));
						self.status = status;
						return Ok(());
					}
					self.channel_actions.post_owner = details.owner.map(|owner| (channel, owner));
					self.channel_actions.post = Some((channel, details));
				} else {
					self.channel_actions.status =
						Some((channel, "Post changed; reopen the menu to refresh", false));
					return Ok(());
				}
				if matches!(
					action,
					Action::PostAddMember(_) | Action::PostRemoveMember(_)
				) && self
					.members
					.as_ref()
					.is_some_and(|members| members.channel == channel)
				{
					self.invalidate_members();
					self.members = None;
				}
				"Post updated"
			}
			Ok(Outcome::Details(edit)) => {
				if self.can_open_channel_settings(channel) && !observed {
					self.channel_actions.details = Some((channel, edit));
					self.channel_actions.completed = Some((channel, request, true));
				} else {
					self.channel_actions.status = Some((
						channel,
						"Channel changed while loading; reopen settings",
						false,
					));
				}
				return Ok(());
			}
			Ok(Outcome::Channel {
				channel: updated,
				permissions,
			}) => {
				let reference = action == Action::Reference;
				let creating = matches!(
					action,
					Action::Duplicate { .. }
						| Action::Create { .. }
						| Action::CreateCategory { .. }
						| Action::CreateThread { .. }
						| Action::CreatePrivateThread { .. }
				);
				let existing_created_thread =
					matches!(
						action,
						Action::CreateThread { .. } | Action::CreatePrivateThread { .. }
					) && self.channel(updated.id).is_some_and(|current| {
						current.guild == updated.guild
							&& current.parent_id == updated.parent_id
							&& current.kind == updated.kind
					});
				if self.guild(guild).is_some()
					&& (if reference {
						true
					} else if matches!(
						action,
						Action::CreateThread { .. } | Action::CreatePrivateThread { .. }
					) {
						if matches!(action, Action::CreatePrivateThread { .. }) {
							self.can_create_private_thread(channel)
						} else {
							self.can_create_thread(channel)
						}
					} else if creating {
						self.can_manage_channel(channel)
					} else {
						self.can_open_channel_settings(channel)
					}) && (if creating {
					self.channel(updated.id).is_none() || existing_created_thread
				} else {
					!observed
				}) {
					let target = updated.id;
					if reference {
						self.remember_channel_reference_name(&updated);
					}
					if !existing_created_thread {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::ChannelCreated(*updated),
						});
					}
					if matches!(
						action,
						Action::CreateThread { .. } | Action::CreatePrivateThread { .. }
					) && self.channel(target).is_some()
					{
						self.channel_actions.created_thread = Some((channel, target, request));
					}
					if reference {
						if self.channel(target).is_none() {
							self.forget_channel_reference_name(target);
							self.channel_actions.status =
								Some((channel, "Thread could not be loaded", false));
							return Ok(());
						}
						self.retire_archived_thread(None);
						self.archived_thread = Some(target);
					}
					if let Some(p) = permissions.filter(|p| p.id == target && p.guild == guild) {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::Permissions(crate::permissions::Event::Channel {
								channel: target,
								guild: Some(guild),
								overwrites: p.overwrites.map_or(Patch::Absent, Patch::Value),
							}),
						});
					}
				}
				if self.demo
					&& !observed && self.can_open_channel_settings(channel)
					&& let Action::Edit { after, .. } = &action
				{
					self.channel_actions.details = Some((channel, after.clone()));
				}
				if reference {
					"Thread loaded"
				} else if creating {
					"Channel created"
				} else {
					"Channel updated"
				}
			}
			Ok(Outcome::Deleted) => {
				if let Some(page) = self.archives.as_mut().and_then(|view| view.page.as_mut()) {
					page.threads.retain(|c| c.id != channel);
					if page.threads.is_empty() {
						page.next = None;
					}
				}
				if !observed {
					self.apply(crate::Envelope {
						generation: self.generation,
						event: crate::Event::Unavailable(channel),
					});
				}
				"Channel deleted"
			}
			Ok(Outcome::Moved) => {
				if !observed
					&& let Action::Move {
						parent,
						position,
						shifts,
						..
					} = action
				{
					self.apply(crate::Envelope {
						generation: self.generation,
						event: crate::Event::ChannelChanged(model::ChannelPatch {
							id: channel,
							name: Patch::Absent,
							icon: Patch::Absent,
							last_message: Patch::Absent,
							parent_id: parent.map_or(Patch::Null, Patch::Value),
							position: Patch::Value(position),
							kind: Patch::Absent,
							message_count: Patch::Absent,
							tags: Patch::Absent,
						}),
					});
					for (shift_id, shift_pos) in shifts {
						self.apply(crate::Envelope {
							generation: self.generation,
							event: crate::Event::ChannelChanged(model::ChannelPatch {
								id: shift_id,
								name: Patch::Absent,
								icon: Patch::Absent,
								last_message: Patch::Absent,
								parent_id: Patch::Absent,
								position: Patch::Value(shift_pos),
								kind: Patch::Absent,
								message_count: Patch::Absent,
								tags: Patch::Absent,
							}),
						});
					}
				}
				"Channel moved"
			}
			Ok(Outcome::Preferences {
				muted,
				level,
				mute_until,
			}) => {
				if self.can_view(channel)
					&& !observed && let Err(status) = self
					.confirm_channel_preferences(guild, channel, muted, level)
					.and_then(|_| self.confirm_channel_mute_timer(channel, muted, mute_until))
				{
					self.channel_actions.status = Some((channel, status, false));
					self.status = status;
					return Ok(());
				}
				"Notification settings updated"
			}
			Ok(Outcome::Personal(change)) => {
				if !observed && !self.personal_change_allowed(guild, &change, false) {
					self.channel_actions.status = Some((
						channel,
						"Channel access or preferences changed; reload before retrying",
						false,
					));
					return Ok(());
				}
				if observed && !self.personal_change_allowed(guild, &change, true) {
					self.channel_actions.status = Some((
						channel,
						"Channel preferences changed in another client; review before retrying",
						false,
					));
					return Ok(());
				}
				if !observed {
					self.confirm_personal_channels(guild, &change)?;
				}
				"Channel preferences updated"
			}
			Ok(Outcome::HideMuted(hide)) => {
				if let Err(status) = self.confirm_guild_hides_muted(guild, hide) {
					self.channel_actions.status = Some((channel, status, false));
					self.status = status;
					return Ok(());
				}
				if hide {
					"Muted channels hidden"
				} else {
					"Muted channels shown"
				}
			}
		};
		self.channel_actions.status = Some((channel, status, true));
		self.channel_actions.completed = Some((channel, request, true));
		self.status = status;
		Ok(())
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event as CoreEvent};
	fn state() -> State {
		let mut state = State {
			demo: true,
			gateway_connected: true,
			user: Some(model::User {
				username: None,
				primary_guild: None,
				cosmetics: None,
				id: Id(1),
				name: "Synthetic".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
			}),
			guilds: vec![model::Guild {
				default_message_notifications: None,
				stickers: None,
				id: Id(2),
				name: "Synthetic guild".into(),
				icon: None,
				emojis: None,
			}],
			channels: vec![model::Channel {
				id: Id(3),
				guild: Some(Id(2)),
				name: "general".into(),
				kind: 0,
				parent_id: None,
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			..State::default()
		};
		state
			.permissions
			.replace(model::permissions::Snapshot {
				guilds: vec![model::permissions::Guild {
					id: Id(2),
					owner: Some(Id(1)),
					roles: None,
					member: None,
				}],
				channels: vec![model::permissions::Channel {
					id: Id(3),
					guild: Id(2),
					overwrites: Some(vec![]),
				}],
			})
			.unwrap();
		state
	}
	fn finish(state: &mut State, command: Command, result: Result<Outcome, Failure>) {
		let Command::ChannelAction {
			guild,
			channel,
			request,
			..
		} = command
		else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ChannelAction(Event::Finished {
				guild,
				channel,
				request,
				result,
			}),
		});
	}
	#[test]
	fn channel_mfa_revalidates_exact_pending_write_and_current_permissions() {
		let mut state = state();
		state.demo = false;
		state.auth = AuthState::Authenticated;
		let before = Edit {
			name: "general".into(),
			..Edit::default()
		};
		state.channel_actions.details = Some((Id(3), before.clone()));
		let mut after = before.clone();
		after.name = "updated".into();
		let command = state
			.request_channel_action(Id(3), Action::Edit { before, after })
			.unwrap();
		let target = crate::mfa::Target::from_command(&command).unwrap();
		let Command::ChannelAction {
			guild,
			channel,
			request,
			action,
		} = command
		else {
			panic!()
		};
		assert!(state.channel_action_mfa_allowed(guild, channel, request, &action));
		assert!(!state.channel_action_mfa_allowed(guild, channel, request + 1, &action));
		assert!(!state.channel_action_mfa_allowed(Id(99), channel, request, &action));
		state.apply_mfa(crate::mfa::Event::Required(crate::mfa::Notice {
			id: 7,
			target,
			methods: vec![crate::mfa::Method::Totp],
		}));
		assert!(state.mfa.pending.is_some());
		member_permissions(&mut state, VIEW_CHANNEL);
		assert!(!state.channel_action_mfa_allowed(guild, channel, request, &action));
		let Some(Command::Mfa(cancel)) = state.maintain_mfa() else {
			panic!()
		};
		assert!(matches!(cancel.action, crate::mfa::Action::Cancel));
		assert!(state.mfa.pending.is_none());
		for action in [
			Action::Reference,
			Action::Description,
			Action::Load,
			Action::PostLoad,
		] {
			assert!(
				crate::mfa::Target::from_command(&Command::ChannelAction {
					guild,
					channel,
					request,
					action
				})
				.is_none()
			);
		}
	}
	fn member_permissions(state: &mut State, bits: u128) {
		let guild = state.permissions.guilds.get_mut(&Id(2)).unwrap();
		guild.owner = Some(Id(99));
		guild.roles = Some(vec![model::permissions::Role {
			icon: None,
			unicode_emoji: None,
			id: Id(2),
			bits,
			name: String::new(),
			color: 0,
			secondary_color: None,
			tertiary_color: None,
			position: 0,
			hoist: false,
		}]);
		guild.member = Some(model::permissions::Member {
			roles: vec![],
			timeout_until: None,
		});
		state.permissions.clear_cache();
	}
	fn announcement(author: model::User) -> model::Message {
		model::Message {
			reply_preview: None,
			id: Id(50),
			channel: Id(3),
			author,
			content: "Synthetic announcement".into(),
			poll: None,
			sticker_items: vec![],
			flags: 0,
			ephemeral: false,
			components: vec![],
			application_id: None,
			reactions: Some(vec![]),
			author_roles: vec![],
			author_nick: None,
			mentions: vec![],
			mention_roles: vec![],
			mention_everyone: false,
			suppress_notifications: false,
			edited: false,
			edited_at: None,
			revision: 0,
			nonce: None,
			reply_to: None,
			kind: 0,
			reply_deleted: false,
			forwarded: false,
			interaction: None,
			unsupported: false,
			extra_content: Default::default(),
			embeds: vec![],
			embeds_suppressed: false,
			attachments: vec![],
		}
	}
	#[test]
	fn publishing_and_following_recheck_scope_permissions_exact_outcomes_and_newer_content() {
		use model::permissions::{MANAGE_MESSAGES, MANAGE_WEBHOOKS, SEND_MESSAGES};
		let mut state = state();
		state.selected = Some(Id(3));
		state.freshness = model::Freshness::Fresh;
		state.channels[0].kind = 5;
		member_permissions(&mut state, VIEW_CHANNEL | SEND_MESSAGES);
		let message = announcement(state.user.clone().unwrap());
		state.timeline.seed_cache(vec![message.clone()]).unwrap();
		assert!(state.can_publish_message(Id(3), Id(50)));
		let mut other = message.clone();
		other.author.id = Id(7);
		state.timeline.seed_cache(vec![other]).unwrap();
		assert!(!state.can_publish_message(Id(3), Id(50)));
		member_permissions(&mut state, VIEW_CHANNEL | SEND_MESSAGES | MANAGE_MESSAGES);
		let publish = state
			.request_channel_action(Id(3), Action::Publish(Id(50)))
			.unwrap();
		assert!(
			state
				.request_channel_action(Id(3), Action::Publish(Id(50)))
				.is_none()
		);
		let mut newer = message;
		newer.content = "Newer content must survive publication".into();
		newer.flags = 4096;
		state.timeline.seed_cache(vec![newer]).unwrap();
		finish(&mut state, publish, Ok(Outcome::Published(Id(50))));
		assert_eq!(
			state.timeline.get(Id(50)).unwrap().content,
			"Newer content must survive publication"
		);
		assert_eq!(state.timeline.get(Id(50)).unwrap().flags, 4097);
		assert!(
			!state.can_publish_message(Id(3), Id(50)),
			"confirmed announcements cannot be published again"
		);
		let mut destination = state.channels[0].clone();
		destination.id = Id(4);
		destination.kind = 0;
		state.channels.push(destination);
		state.permissions.channels.insert(
			Id(4),
			model::permissions::Channel {
				id: Id(4),
				guild: Id(2),
				overwrites: Some(vec![]),
			},
		);
		member_permissions(&mut state, VIEW_CHANNEL | SEND_MESSAGES);
		assert!(!state.can_follow_announcement(Id(3), Id(4)));
		member_permissions(&mut state, VIEW_CHANNEL | SEND_MESSAGES | MANAGE_WEBHOOKS);
		assert!(state.can_follow_announcement(Id(3), Id(4)));
		assert!(!state.can_follow_announcement(Id(3), Id(3)));
		let follow = state
			.request_channel_action(Id(3), Action::FollowAnnouncement(Id(4)))
			.unwrap();
		finish(
			&mut state,
			follow,
			Ok(Outcome::AnnouncementFollowed {
				destination: Id(99),
				webhook: Id(100),
			}),
		);
		assert!(!state.channel_action_succeeded(Id(3)));
	}
	#[test]
	fn private_threads_tags_and_invitations_follow_current_permissions_and_exact_creation() {
		use model::permissions::{CREATE_PRIVATE_THREADS, SEND_MESSAGES_IN_THREADS};
		let mut state = state();
		member_permissions(&mut state, VIEW_CHANNEL);
		assert!(
			!state.can_create_private_thread(Id(3)),
			"private creation requires its own permission"
		);
		member_permissions(
			&mut state,
			VIEW_CHANNEL | CREATE_PRIVATE_THREADS | SEND_MESSAGES_IN_THREADS,
		);
		assert!(state.can_create_private_thread(Id(3)));
		let command = state
			.request_channel_action(
				Id(3),
				Action::CreatePrivateThread {
					name: "Private".into(),
					invitable: false,
				},
			)
			.unwrap();
		let mut thread = state.channels[0].clone();
		thread.id = Id(4);
		thread.kind = 12;
		thread.parent_id = Some(Id(3));
		thread.name = "Private".into();
		finish(
			&mut state,
			command,
			Ok(Outcome::Channel {
				channel: Box::new(thread.clone()),
				permissions: None,
			}),
		);
		assert_eq!(
			state.created_thread(Id(3), state.channel_actions.completed.unwrap().1),
			Some(Id(4))
		);
		state.channel_actions.post = Some((
			Id(4),
			PostDetails {
				owner: Some(Id(1)),
				followed: true,
				..Default::default()
			},
		));
		assert!(
			!state.can_add_thread_member(Id(4), Id(7)),
			"non-invitable private threads require Manage Threads"
		);
		state.channel_actions.post.as_mut().unwrap().1.invitable = true;
		assert!(state.can_add_thread_member(Id(4), Id(7)));
		let mut member = state.user.clone().unwrap();
		member.id = Id(7);
		state.members = Some(model::MemberList {
			guild: Some(Id(2)),
			channel: Id(4),
			request: 1,
			start: 0,
			slots: vec![Some(model::MemberSlot::Person(model::Member {
				user: member,
				roles: vec![],
				nick: None,
				status: None,
				custom_status: None,
				activities: vec![],
				clients: Default::default(),
			}))],
			total: 1,
			lazy: false,
			freshness: model::Freshness::Fresh,
			groups: vec![],
			ranges: vec![],
		});
		assert!(
			state.can_remove_thread_member(Id(4), Id(7)),
			"private thread creators may remove a member without Manage Threads"
		);
		state.channel_actions.post.as_mut().unwrap().1.owner = Some(Id(9));
		assert!(!state.can_remove_thread_member(Id(4), Id(7)));
		state.channel_actions.post.as_mut().unwrap().1.owner = Some(Id(1));
		state.members.as_mut().unwrap().freshness = model::Freshness::Stale;
		assert!(!state.can_remove_thread_member(Id(4), Id(7)));
		state.members.as_mut().unwrap().freshness = model::Freshness::Fresh;
		state.channel_actions.post.as_mut().unwrap().1.archived = true;
		assert!(!state.can_add_thread_member(Id(4), Id(7)));
		state.channel_actions.post.as_mut().unwrap().1.archived = false;
		state.channels[0].kind = 15;
		state.channels[0].tags = Some(Box::new(model::forum::Tags {
			required: true,
			available: vec![
				model::forum::Tag {
					id: Id(100),
					name: "Help".into(),
					moderated: false,
					emoji_id: None,
					emoji_name: None,
				},
				model::forum::Tag {
					id: Id(101),
					name: "Resolved".into(),
					moderated: true,
					emoji_id: None,
					emoji_name: None,
				},
			],
			..Default::default()
		}));
		state.channels[1].kind = 11;
		state.channels[1].tags = Some(Box::new(model::forum::Tags {
			applied: vec![Id(100)],
			..Default::default()
		}));
		assert!(state.can_change_post_tags(Id(4), &[Id(100)]));
		assert!(!state.can_change_post_tags(Id(4), &[]));
		assert!(!state.can_change_post_tags(Id(4), &[Id(101)]));
		assert!(!state.can_change_post_tags(Id(4), &[Id(999)]));
		member_permissions(
			&mut state,
			VIEW_CHANNEL | SEND_MESSAGES_IN_THREADS | MANAGE_THREADS,
		);
		assert!(state.can_change_post_tags(Id(4), &[Id(101)]));
		member_permissions(&mut state, VIEW_CHANNEL | SEND_MESSAGES_IN_THREADS);
		state.channels[1].tags.as_mut().unwrap().applied = vec![Id(101)];
		assert!(
			!state.can_change_post_tags(Id(4), &[Id(100)]),
			"removing a moderated tag also requires permission"
		);
	}
	#[test]
	fn message_thread_creation_requires_the_source_message_id() {
		for response in [Id(50), Id(51)] {
			let mut state = state();
			let message = announcement(state.user.clone().unwrap());
			state.timeline.seed_cache(vec![message]).unwrap();
			let command = state
				.request_channel_action(
					Id(3),
					Action::CreateThread {
						name: "Discussion".into(),
						message: Some(Id(50)),
					},
				)
				.unwrap();
			let mut thread = state.channels[0].clone();
			thread.id = response;
			thread.kind = 11;
			thread.parent_id = Some(Id(3));
			thread.name = "Discussion".into();
			finish(
				&mut state,
				command,
				Ok(Outcome::Channel {
					channel: Box::new(thread),
					permissions: None,
				}),
			);
			assert_eq!(state.channel(response).is_some(), response == Id(50));
		}
	}
	#[test]
	fn thread_creation_uses_correlated_id_preserves_gateway_edits_and_cancels_on_reset() {
		let mut state = state();
		let action = Action::CreatePrivateThread {
			name: "Private".into(),
			invitable: true,
		};
		let command = state.request_channel_action(Id(3), action.clone()).unwrap();
		let Command::ChannelAction { request, .. } = &command else {
			panic!();
		};
		let request = *request;
		let mut response = state.channels[0].clone();
		response.id = Id(4);
		response.kind = 12;
		response.parent_id = Some(Id(3));
		response.name = "Private".into();
		let mut live = response.clone();
		live.name = "Newer gateway name".into();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ChannelCreated(live),
		});
		finish(
			&mut state,
			command,
			Ok(Outcome::Channel {
				channel: Box::new(response.clone()),
				permissions: None,
			}),
		);
		assert_eq!(state.created_thread(Id(3), request), Some(Id(4)));
		assert_eq!(state.channel_action_completed(Id(3), request), Some(true));
		assert_eq!(state.channel(Id(4)).unwrap().name, "Newer gateway name");
		assert_eq!(state.created_thread(Id(3), request + 1), None);
		let command = state.request_channel_action(Id(3), action).unwrap();
		let Command::ChannelAction { request, .. } = &command else {
			panic!();
		};
		let request = *request;
		state.cancel_channel_action();
		state.channel_actions.reset();
		assert_eq!(state.channel_action_completed(Id(3), request), Some(false));
		response.id = Id(5);
		finish(
			&mut state,
			command,
			Ok(Outcome::Channel {
				channel: Box::new(response),
				permissions: None,
			}),
		);
		assert!(state.channel(Id(5)).is_none());
	}
	#[test]
	fn uncertain_thread_member_write_requires_explicit_refresh_and_keeps_draft() {
		let mut state = state();
		let mut thread = state.channels[0].clone();
		thread.id = Id(4);
		thread.kind = 12;
		thread.parent_id = Some(Id(3));
		state.channels.push(thread);
		state.selected = Some(Id(4));
		state.drafts.insert(Id(4), "Keep private draft".into());
		state.members = Some(model::MemberList {
			guild: Some(Id(2)),
			channel: Id(4),
			request: 1,
			start: 0,
			slots: vec![],
			total: 0,
			lazy: false,
			freshness: model::Freshness::Fresh,
			groups: vec![],
			ranges: vec![],
		});
		state.channel_actions.post = Some((
			Id(4),
			PostDetails {
				owner: Some(Id(1)),
				followed: true,
				invitable: true,
				..Default::default()
			},
		));
		let command = state
			.request_channel_action(Id(4), Action::PostAddMember(Id(7)))
			.unwrap();
		finish(&mut state, command, Err(Failure::Ambiguous));
		assert!(state.post_details(Id(4)).is_none());
		assert!(state.members.is_none());
		assert!(
			state
				.request_channel_action(Id(4), Action::PostAddMember(Id(7)))
				.is_none()
		);
		assert!(
			state
				.request_channel_action(Id(4), Action::PostLoad)
				.is_some()
		);
		assert_eq!(state.drafts[&Id(4)], "Keep private draft");
	}
	#[test]
	fn creation_checks_kind_name_and_permissions() {
		{
			for kind in [CreateKind::Text, CreateKind::Voice, CreateKind::Forum] {
				for response_kind in [kind.wire_kind(), 4] {
					let mut state = state();
					assert!(
						state
							.request_channel_action(
								Id(3),
								Action::Create {
									name: " ".into(),
									kind,
								}
							)
							.is_none()
					);
					let action = Action::Create {
						name: "new".into(),
						kind,
					};
					let command = state.request_channel_action(Id(3), action.clone()).unwrap();
					let mut created = state.channels[0].clone();
					created.id = Id(4);
					created.kind = response_kind;
					finish(
						&mut state,
						command,
						Ok(Outcome::Channel {
							channel: Box::new(created),
							permissions: None,
						}),
					);
					assert_eq!(
						state.channel(Id(4)).is_some(),
						response_kind == kind.wire_kind()
					);
					assert_eq!(
						state.channel_action_succeeded(Id(3)),
						response_kind == kind.wire_kind()
					);
					state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(9));
					state.permissions.clear_cache();
					assert!(state.request_channel_action(Id(3), action).is_none());
				}
			}
		}
		{
			let mut state = state();
			state.channels[0].kind = 4;
			let before = Edit {
				name: "Category".into(),
				overwrites: vec![model::permissions::Overwrite {
					id: Id(2),
					kind: 0,
					allow: 1 << 100,
					deny: 0,
				}],
				..Edit::default()
			};
			let load = state.request_channel_action(Id(3), Action::Load).unwrap();
			finish(&mut state, load, Ok(Outcome::Details(before.clone())));
			let after = Edit {
				name: "Renamed".into(),
				..before.clone()
			};
			assert!(
				state
					.request_channel_action(
						Id(3),
						Action::Edit {
							before: before.clone(),
							after
						}
					)
					.is_some()
			);
			state.cancel_channel_action();
			let load = state.request_channel_action(Id(3), Action::Load).unwrap();
			finish(&mut state, load, Ok(Outcome::Details(before.clone())));
			assert_eq!(
				state.channel_details(Id(3)).unwrap().overwrites,
				before.overwrites
			);
			let guild = state.permissions.guilds.get_mut(&Id(2)).unwrap();
			guild.owner = Some(Id(99));
			guild.roles = Some(vec![model::permissions::Role {
				icon: None,
				unicode_emoji: None,
				id: Id(2),
				name: String::new(),
				color: 0,
				secondary_color: None,
				tertiary_color: None,
				position: 0,
				hoist: false,
				bits: VIEW_CHANNEL | MANAGE_ROLES,
			}]);
			guild.member = Some(model::permissions::Member {
				roles: vec![],
				timeout_until: None,
			});
			state.permissions.clear_cache();
			assert!(!state.can_manage_channel(Id(3)));
			assert!(!state.can_open_channel_settings(Id(3)));
			assert!(!state.can_edit_channel_permission(Id(3), VIEW_CHANNEL));
			state
				.permissions
				.guilds
				.get_mut(&Id(2))
				.unwrap()
				.roles
				.as_mut()
				.unwrap()[0]
				.bits |= MANAGE_CHANNELS;
			state.permissions.clear_cache();
			assert!(state.can_open_channel_settings(Id(3)));
			assert!(state.can_edit_channel_permission(Id(3), VIEW_CHANNEL));
			assert!(state.can_edit_channel_permission(Id(3), 0));
			assert!(!state.can_edit_channel_permission(Id(3), model::permissions::MANAGE_GUILD));
			let mut after = before.clone();
			after.topic = "Unsupported category topic".into();
			assert!(
				state
					.request_channel_action(
						Id(3),
						Action::Edit {
							before: before.clone(),
							after
						}
					)
					.is_none()
			);
			let mut after = before.clone();
			after.overwrites[0].deny |= VIEW_CHANNEL;
			assert!(
				state
					.request_channel_action(
						Id(3),
						Action::Edit {
							before: before.clone(),
							after: after.clone()
						}
					)
					.is_some()
			);
			state.cancel_channel_action();
			let load = state.request_channel_action(Id(3), Action::Load).unwrap();
			finish(&mut state, load, Ok(Outcome::Details(before.clone())));
			let mut stale = before.clone();
			stale.overwrites[0].allow = 0;
			assert!(
				state
					.request_channel_action(
						Id(3),
						Action::Edit {
							before: stale,
							after
						}
					)
					.is_none()
			);
			let mut invalid = before;
			invalid.overwrites.push(invalid.overwrites[0]);
			assert!(!invalid.valid());
		}
	}

	fn owned_post() -> (State, model::Channel, PostDetails) {
		let mut state = state();
		let thread = model::Channel {
			id: Id(4),
			guild: Some(Id(2)),
			parent_id: Some(Id(3)),
			kind: 11,
			name: "Videos higher than 1080p?".into(),
			position: 0,
			recipients: vec![],
			last_message: None,
			icon: None,
			member_list_id: None,
			tags: None,
			message_count: None,
		};
		state.channels.push(thread.clone());
		let details = PostDetails {
			owner: Some(Id(1)),
			..PostDetails::default()
		};
		let load = state
			.request_channel_action(Id(4), Action::PostLoad)
			.unwrap();
		finish(
			&mut state,
			load,
			Ok(Outcome::Post {
				channel: Box::new(thread.clone()),
				details,
			}),
		);
		// A post owner without Manage Threads may still rename their own post.
		let guild = state.permissions.guilds.get_mut(&Id(2)).unwrap();
		guild.owner = Some(Id(99));
		guild.roles = Some(vec![model::permissions::Role {
			icon: None,
			unicode_emoji: None,
			id: Id(2),
			name: String::new(),
			color: 0,
			secondary_color: None,
			tertiary_color: None,
			position: 0,
			hoist: false,
			bits: VIEW_CHANNEL,
		}]);
		guild.member = Some(model::permissions::Member {
			roles: vec![],
			timeout_until: None,
		});
		state.permissions.clear_cache();
		assert!(!state.can_manage_post(Id(4)));
		assert!(state.can_edit_post(Id(4)));
		(state, thread, details)
	}

	#[test]
	fn post_ownership_requires_current_view_access() {
		let (mut state, _, _) = owned_post();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Permissions(crate::permissions::Event::Channel {
				channel: Id(3),
				guild: Some(Id(2)),
				overwrites: Patch::Value(vec![model::permissions::Overwrite {
					id: Id(2),
					kind: 0,
					allow: 0,
					deny: VIEW_CHANNEL,
				}]),
			}),
		});
		assert!(!state.can_view(Id(4)));
		assert!(state.post_details(Id(4)).is_none());
		assert!(!state.can_edit_post(Id(4)));
		assert!(
			state
				.request_channel_action(Id(4), Action::PostRename("Renamed".into()))
				.is_none()
		);
	}

	/// Discord echoes this client's own rename as a thread update, often before the response.
	#[test]
	fn renaming_a_post_survives_the_echo_of_its_own_rename() {
		let (mut state, thread, details) = owned_post();
		let renamed = "FIXED Videos higher than 1080p?";
		let rename = state
			.request_channel_action(Id(4), Action::PostRename(renamed.into()))
			.unwrap();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ThreadChanged {
				guild: Id(2),
				patch: model::ChannelPatch {
					id: Id(4),
					name: Patch::Value(renamed.into()),
					icon: Patch::Absent,
					last_message: Patch::Absent,
					parent_id: Patch::Value(Id(3)),
					position: Patch::Absent,
					kind: Patch::Value(11),
					message_count: Patch::Absent,
					tags: Patch::Absent,
				},
			},
		});
		// The update dropped the loaded details, but the owner may still edit their post.
		assert!(state.can_edit_post(Id(4)));
		let mut updated = thread;
		updated.name = renamed.into();
		finish(
			&mut state,
			rename,
			Ok(Outcome::Post {
				channel: Box::new(updated),
				details,
			}),
		);
		assert!(
			state.channel_action_succeeded(Id(4)),
			"{:?}",
			state.channel_action_status(Id(4))
		);
		assert_eq!(state.channel(Id(4)).unwrap().name, renamed);
		// A same-name update can also carry a newer lock/archive state from another client.
		assert!(state.post_details(Id(4)).is_none());
		assert!(state.can_edit_post(Id(4)));
		let load = state
			.request_channel_action(Id(4), Action::PostLoad)
			.unwrap();
		let updated = state.channel(Id(4)).unwrap().clone();
		finish(
			&mut state,
			load,
			Ok(Outcome::Post {
				channel: Box::new(updated),
				details: PostDetails {
					locked: true,
					..details
				},
			}),
		);
		assert!(state.post_details(Id(4)).unwrap().locked);
	}

	#[test]
	fn renaming_a_post_preserves_a_later_name() {
		let (mut state, mut thread, details) = owned_post();
		let rename = state
			.request_channel_action(Id(4), Action::PostRename("Requested".into()))
			.unwrap();
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ChannelChanged(model::ChannelPatch {
				id: Id(4),
				name: Patch::Value("Later edit".into()),
				icon: Patch::Absent,
				last_message: Patch::Absent,
				parent_id: Patch::Absent,
				position: Patch::Absent,
				kind: Patch::Absent,
				message_count: Patch::Absent,
				tags: Patch::Absent,
			}),
		});
		thread.name = "Requested".into();
		finish(
			&mut state,
			rename,
			Ok(Outcome::Post {
				channel: Box::new(thread),
				details,
			}),
		);
		assert!(!state.channel_action_succeeded(Id(4)));
		assert_eq!(state.channel(Id(4)).unwrap().name, "Later edit");
		assert!(state.post_details(Id(4)).is_none());
	}

	#[test]
	fn thread_references_reuse_channel_loading_and_require_a_viewable_parent() {
		let mut valid = state();
		valid.selected = Some(Id(3));
		let command = valid.request_channel_reference(Id(2), Id(4)).unwrap();
		assert!(matches!(
			command,
			Command::ChannelAction {
				action: Action::Reference,
				..
			}
		));
		assert!(valid.request_channel_reference(Id(2), Id(4)).is_none());
		finish(
			&mut valid,
			command,
			Ok(Outcome::Channel {
				channel: Box::new(model::Channel {
					id: Id(4),
					guild: Some(Id(2)),
					parent_id: Some(Id(3)),
					kind: 11,
					name: "Synthetic thread".into(),
					position: 0,
					recipients: vec![],
					last_message: None,
					icon: None,
					member_list_id: None,
					tags: None,
					message_count: None,
				}),
				permissions: None,
			}),
		);
		assert_eq!(valid.channel(Id(4)).unwrap().name, "Synthetic thread");
		assert_eq!(
			valid.channel_reference_name(Id(4)),
			Some("Synthetic thread")
		);
		assert_eq!(valid.archived_thread, Some(Id(4)));
		valid.retire_archived_thread(None);
		assert!(valid.channel(Id(4)).is_none());
		assert_eq!(
			valid.channel_reference_name(Id(4)),
			Some("Synthetic thread")
		);

		let mut state = state();
		state.selected = Some(Id(3));
		let command = state.request_channel_reference(Id(2), Id(4)).unwrap();
		finish(
			&mut state,
			command,
			Ok(Outcome::Channel {
				channel: Box::new(model::Channel {
					id: Id(4),
					guild: Some(Id(2)),
					parent_id: Some(Id(99)),
					kind: 11,
					name: "Unknown parent".into(),
					position: 0,
					recipients: vec![],
					last_message: None,
					icon: None,
					member_list_id: None,
					tags: None,
					message_count: None,
				}),
				permissions: None,
			}),
		);
		assert!(state.channel(Id(4)).is_none());
		assert!(!state.channel_action_succeeded(Id(4)));
	}

	#[test]
	fn channel_writes_check_access_bounds_queue_failure_and_stale_completions() {
		let mut state = state();
		assert!(state.can_manage_channel(Id(3)));
		for action in [
			Action::Notifications(4),
			Action::Mute(Mute::For(1)),
			Action::Duplicate { name: " ".into() },
			Action::Edit {
				before: Edit {
					name: "general".into(),
					..Edit::default()
				},
				after: Edit {
					name: "valid".into(),
					topic: "x".repeat(1025),
					..Edit::default()
				},
			},
		] {
			assert!(state.request_channel_action(Id(3), action).is_none());
		}
		let pending = state.request_channel_action(Id(3), Action::Load).unwrap();
		assert!(
			state
				.request_channel_action(Id(3), Action::Delete)
				.is_none()
		);
		state.command_rejected(pending);
		assert!(!state.channel_action_pending());
		let pending = state.request_channel_action(Id(3), Action::Load).unwrap();
		state.cancel_channel_action();
		finish(
			&mut state,
			pending,
			Ok(Outcome::Details(Edit {
				name: "late".into(),
				..Edit::default()
			})),
		);
		assert!(state.channel_details(Id(3)).is_none());
		let pending = state
			.request_channel_action(
				Id(3),
				Action::CreateCategory {
					name: "projects".into(),
				},
			)
			.unwrap();
		let mut category = state.channels[0].clone();
		category.id = Id(4);
		category.kind = 4;
		category.name = "projects".into();
		finish(
			&mut state,
			pending,
			Ok(Outcome::Channel {
				channel: Box::new(category),
				permissions: None,
			}),
		);
		assert!(
			state
				.channel(Id(4))
				.is_some_and(|channel| channel.kind == 4)
		);
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(9));
		state.permissions.clear_cache();
		assert!(
			state
				.request_channel_action(Id(3), Action::Delete)
				.is_none()
		);
	}
	#[test]
	fn channel_moves_require_a_manageable_category_and_apply_the_confirmed_position() {
		{
			let mut state = state();
			let mut category = state.channels[0].clone();
			category.id = Id(4);
			category.kind = 4;
			category.name = "projects".into();
			state.channels.push(category);
			state.permissions.channels.insert(
				Id(4),
				model::permissions::Channel {
					id: Id(4),
					guild: Id(2),
					overwrites: Some(vec![]),
				},
			);
			state.permissions.clear_cache();
			let action = Action::Move {
				parent: Some(Id(4)),
				position: 2,
				lock_permissions: true,
				shifts: vec![(Id(4), 1)],
			};
			let command = state.request_channel_action(Id(3), action).unwrap();
			finish(&mut state, command, Ok(Outcome::Moved));
			assert_eq!(state.channel(Id(3)).unwrap().parent_id, Some(Id(4)));
			assert_eq!(state.channel(Id(3)).unwrap().position, 2);
			assert_eq!(state.channel(Id(4)).unwrap().position, 1);
		}
		{
			let mut state = state();
			let pending = state.request_channel_action(Id(3), Action::Delete).unwrap();
			let mut renamed = state.channels[0].clone();
			renamed.name = "renamed".into();
			state.observe_channel_action(&CoreEvent::ChannelChanged(model::ChannelPatch {
				id: Id(3),
				name: Patch::Value("renamed".into()),
				icon: Patch::Absent,
				last_message: Patch::Absent,
				parent_id: Patch::Absent,
				position: Patch::Absent,
				kind: Patch::Absent,
				message_count: Patch::Absent,
				tags: Patch::Absent,
			}));
			finish(&mut state, pending, Ok(Outcome::Deleted));
			assert!(state.channel(Id(3)).is_none());
			assert!(state.channel_action_succeeded(Id(3)));
			let mut state = self::state();
			state.channel_actions.details = Some((
				Id(3),
				Edit {
					name: "general".into(),
					..Edit::default()
				},
			));
			let pending = state
				.request_channel_action(
					Id(3),
					Action::Edit {
						before: Edit {
							name: "general".into(),
							..Edit::default()
						},
						after: Edit {
							name: "rename".into(),
							..Edit::default()
						},
					},
				)
				.unwrap();
			state.apply(Envelope {
				generation: state.generation,
				event: CoreEvent::Unavailable(Id(3)),
			});
			finish(
				&mut state,
				pending,
				Ok(Outcome::Channel {
					channel: Box::new(renamed),
					permissions: None,
				}),
			);
			assert!(state.channel(Id(3)).is_none());
		}
	}
	#[test]
	fn timed_mutes_expire_for_delivery_and_matching_gateway_echo_preserves_timer() {
		let mut state = state();
		let setting = crate::notifications::Setting {
			guild: Some(Id(2)),
			muted: Some(false),
			level: Some(0),
			channels: vec![(Id(3), Some(true), Some(3))],
			channel_mute_until: vec![(Id(3), State::permission_time() - 1)],
			..Default::default()
		};
		state
			.apply_notification_preferences(crate::notifications::Event::Settings {
				entries: vec![setting.clone()],
				replace: true,
			})
			.unwrap();
		state
			.apply_notification_preferences(crate::notifications::Event::Presence(Some(false)))
			.unwrap();
		state
			.confirm_channel_mute_timer(Id(3), Some(true), Some(State::permission_time() - 1))
			.unwrap();
		state
			.apply_notification_preferences(crate::notifications::Event::Settings {
				entries: vec![setting.clone()],
				replace: false,
			})
			.unwrap();
		assert_eq!(state.guild_channel_muted(Id(3)), Some(false));
		assert!(state.notification_allowed(Id(3)));
		let mut permanent = setting;
		permanent.channel_mute_until.clear();
		state
			.apply_notification_preferences(crate::notifications::Event::Settings {
				entries: vec![permanent.clone()],
				replace: false,
			})
			.unwrap();
		assert!(!state.notification_allowed(Id(3)));
		state
			.confirm_channel_mute_timer(Id(3), Some(true), Some(State::permission_time() - 1))
			.unwrap();
		state
			.apply_notification_preferences(crate::notifications::Event::Settings {
				entries: vec![permanent],
				replace: true,
			})
			.unwrap();
		assert_eq!(state.guild_channel_muted(Id(3)), Some(true));
		assert!(!state.notification_allowed(Id(3)));
	}
}

#[cfg(test)]
mod voice_edit_tests {
	use super::*;
	#[test]
	fn voice_and_stage_message_controls_require_management_and_keep_topics_text_only() {
		for kind in [2, 13] {
			for bits in [0, MANAGE_CHANNELS] {
				let mut state = crate::community_admin::tests::state(bits);
				state
					.channels
					.iter_mut()
					.find(|c| c.id == Id(10))
					.unwrap()
					.kind = kind;
				let before = Edit {
					name: "Voice".into(),
					..Default::default()
				};
				let mut after = before.clone();
				after.nsfw = true;
				after.slowmode = 21600;
				assert_eq!(
					state.channel_edit_allowed(Id(10), &before, &after),
					bits != 0
				);
				assert!(
					Action::Edit {
						before: before.clone(),
						after: after.clone()
					}
					.valid()
				);
				after.topic = "Text channel topic".into();
				assert!(!state.channel_edit_allowed(Id(10), &before, &after));
				after.topic.clear();
				after.slowmode = 21601;
				assert!(!Action::Edit { before, after }.valid());
			}
		}
	}
	#[test]
	fn voice_and_stage_limits_and_channel_management_are_admitted_locally() {
		let mut state = crate::community_admin::tests::state(MANAGE_CHANNELS);
		state
			.channels
			.iter_mut()
			.find(|c| c.id == Id(10))
			.unwrap()
			.kind = 2;
		let before = Edit {
			name: "Voice".into(),
			voice: Some(Box::new(VoiceEdit {
				bitrate: 64000,
				user_limit: 0,
				region: None,
				video_quality: 1,
				regions: vec![],
			})),
			..Default::default()
		};
		let mut after = before.clone();
		after.voice.as_mut().unwrap().user_limit = 99;
		assert!(state.channel_edit_allowed(Id(10), &before, &after));
		after.voice.as_mut().unwrap().user_limit = 100;
		assert!(!state.channel_edit_allowed(Id(10), &before, &after));
		state
			.channels
			.iter_mut()
			.find(|c| c.id == Id(10))
			.unwrap()
			.kind = 13;
		assert!(state.channel_edit_allowed(Id(10), &before, &after));
		after.voice.as_mut().unwrap().bitrate = 65000;
		assert!(!state.channel_edit_allowed(Id(10), &before, &after));
	}
	#[test]
	fn forum_guidelines_admit_the_documented_limit_without_allowing_oversize_text_topics() {
		let mut state = crate::community_admin::tests::state(MANAGE_CHANNELS);
		let before = Edit {
			name: "Forum".into(),
			..Default::default()
		};
		let mut after = before.clone();
		after.topic = "a".repeat(4096);
		assert!(!state.channel_edit_allowed(Id(10), &before, &after));
		state
			.channels
			.iter_mut()
			.find(|c| c.id == Id(10))
			.unwrap()
			.kind = 15;
		assert!(state.channel_edit_allowed(Id(10), &before, &after));
		after.topic.push('a');
		assert!(!state.channel_edit_allowed(Id(10), &before, &after));
	}
}
