//! Channel wire projection and absent/null/value patch handling.
use super::{CHANNEL_FLAG_SPAM, Channel, Deserialize, Id, Overwrite, Patch, UserDto, forum, lossy};

#[derive(Deserialize)]
pub struct ChannelDto {
	#[serde(default)]
	pub icon: Option<String>,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub flags: u64,
	#[serde(default)]
	pub last_message_id: Option<Id>,
	pub id: Id,
	#[serde(default)]
	pub guild_id: Option<Id>,
	#[serde(default)]
	pub parent_id: Option<Id>,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub position: i32,
	#[serde(default)]
	pub name: Option<String>,
	#[serde(rename = "type")]
	pub kind: u8,
	/// An unreadable recipient is dropped without losing the conversation.
	#[serde(default, deserialize_with = "lossy::recipients")]
	pub recipients: Vec<UserDto>,
	#[serde(default)]
	pub permission_overwrites: Option<Vec<Overwrite>>,
	#[serde(default)]
	pub message_count: Option<u32>,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub is_message_request: bool,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub is_spam: bool,
	#[serde(default)]
	pub available_tags: Option<forum::TagList>,
	#[serde(default)]
	pub applied_tags: Option<forum::AppliedTags>,
	#[serde(default)]
	pub default_reaction_emoji: Option<forum::DefaultReaction>,
	#[serde(default)]
	pub default_forum_layout: Option<u8>,
	#[serde(default)]
	pub default_sort_order: Option<u8>,
	#[serde(default)]
	pub default_tag_setting: Option<String>,
}

impl ChannelDto {
	pub fn is_obfuscated(&self) -> bool {
		self.flags & (1 << 17) != 0
	}

	fn spam_folder(&self) -> bool {
		self.is_spam || self.flags & CHANNEL_FLAG_SPAM != 0
	}

	pub fn pending_message_request(&self) -> bool {
		self.guild_id.is_none() && self.kind == 1 && self.is_message_request && !self.spam_folder()
	}

	pub fn pending_spam_direct(&self) -> bool {
		self.guild_id.is_none() && self.kind == 1 && self.spam_folder()
	}

	pub fn into_model(self) -> Channel {
		let recipients: Vec<_> = self
			.recipients
			.into_iter()
			.take(64)
			.map(UserDto::into_model)
			.collect();
		Channel {
			icon: self.icon.filter(|hash| model::valid_avatar_hash(hash)),
			last_message: self.last_message_id,
			id: self.id,
			guild: self.guild_id,
			parent_id: self.parent_id,
			position: self.position,
			name: self.name.unwrap_or_else(|| {
				recipients
					.iter()
					.map(|u| u.name.as_str())
					.collect::<Vec<_>>()
					.join(", ")
			}),
			tags: forum::tags(
				self.kind,
				self.available_tags,
				self.applied_tags,
				self.flags,
				forum::Defaults {
					reaction: self.default_reaction_emoji,
					layout: self.default_forum_layout,
					sort: self.default_sort_order,
					tag_setting: self.default_tag_setting,
				},
			),
			kind: self.kind,
			recipients,
			member_list_id: None,
			message_count: self.message_count,
		}
	}
}

#[derive(Deserialize)]
pub struct ChannelPatchDto {
	#[serde(default)]
	pub icon: Patch<String>,
	#[serde(default)]
	pub last_message_id: Patch<Id>,
	pub id: Id,
	#[serde(default)]
	pub name: Patch<String>,
	#[serde(default)]
	pub parent_id: Patch<Id>,
	#[serde(default)]
	pub position: Patch<i32>,
	#[serde(rename = "type", default)]
	pub kind: Patch<u8>,
	#[serde(default)]
	pub permission_overwrites: Patch<Vec<Overwrite>>,
	#[serde(default)]
	pub flags: Patch<u64>,
	#[serde(default)]
	pub message_count: Patch<u32>,
	#[serde(default)]
	pub is_message_request: Patch<bool>,
	#[serde(default)]
	pub is_spam: Patch<bool>,
	#[serde(default)]
	pub available_tags: Patch<forum::TagList>,
	#[serde(default)]
	pub applied_tags: Patch<forum::AppliedTags>,
	#[serde(default)]
	pub default_reaction_emoji: Option<forum::DefaultReaction>,
	#[serde(default)]
	pub default_forum_layout: Option<u8>,
	#[serde(default)]
	pub default_sort_order: Option<u8>,
	#[serde(default)]
	pub default_tag_setting: Option<String>,
}

impl ChannelPatchDto {
	pub fn is_obfuscated(&self) -> bool {
		matches!(self.flags, Patch::Value(flags) if flags & (1 << 17) != 0)
	}

	fn spam_folder(&self) -> Option<bool> {
		let flagged = match self.flags {
			Patch::Value(flags) => Some(flags & CHANNEL_FLAG_SPAM != 0),
			Patch::Null => Some(false),
			Patch::Absent => None,
		};
		let marked = match self.is_spam {
			Patch::Value(marked) => Some(marked),
			Patch::Null => Some(false),
			Patch::Absent => None,
		};
		match (marked, flagged) {
			(Some(true), _) | (_, Some(true)) => Some(true),
			(None, None) => None,
			_ => Some(false),
		}
	}

	pub fn pending_message_request(&self) -> Option<bool> {
		let request = match self.is_message_request {
			Patch::Value(v) => Some(v),
			Patch::Null => Some(false),
			Patch::Absent => None,
		};
		match (request, self.spam_folder()) {
			(_, Some(true)) => Some(false),
			(Some(false), _) => Some(false),
			(Some(true), _) => Some(true),
			(None, _) => None,
		}
	}

	pub fn merged_inbox(&self, prior: (bool, bool)) -> (bool, bool) {
		if matches!(self.kind, Patch::Value(kind) if kind != 1) {
			return (false, false);
		}
		let request = match self.is_message_request {
			Patch::Value(v) => v,
			Patch::Null => false,
			Patch::Absent => prior.0,
		};
		(request, self.spam_folder().unwrap_or(prior.1))
	}

	pub fn pending_spam_direct(&self) -> Option<bool> {
		match self.is_spam {
			Patch::Value(_) | Patch::Null => self.spam_folder(),
			Patch::Absent => match self.flags {
				Patch::Value(flags) if flags & CHANNEL_FLAG_SPAM != 0 => Some(true),
				_ => None,
			},
		}
	}

	pub fn into_model(self) -> model::ChannelPatch {
		model::ChannelPatch {
			icon: match self.icon {
				Patch::Value(hash) if model::valid_avatar_hash(&hash) => Patch::Value(hash),
				Patch::Value(_) | Patch::Null => Patch::Null,
				Patch::Absent => Patch::Absent,
			},
			last_message: self.last_message_id,
			id: self.id,
			name: self.name,
			parent_id: self.parent_id,
			position: self.position,
			tags: forum::patched_tags(
				self.kind.clone(),
				self.available_tags,
				self.applied_tags,
				&self.flags,
				forum::Defaults {
					reaction: self.default_reaction_emoji,
					layout: self.default_forum_layout,
					sort: self.default_sort_order,
					tag_setting: self.default_tag_setting,
				},
			),
			kind: self.kind,
			message_count: self.message_count,
		}
	}
}
