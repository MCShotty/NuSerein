//! Server/channel navigation records and sparse patches.
use super::{
	CustomEmoji, Embed, Id, Patch, Sticker, User, custom_emoji_bytes, forum, sticker_bytes,
	valid_avatar_hash,
};

/// Untrusted presentation hints, never evidence of current membership or permissions.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExpressionCatalog {
	pub guild: Id,
	pub emojis: Option<Vec<CustomEmoji>>,
	pub stickers: Option<Vec<Sticker>>,
}
impl ExpressionCatalog {
	pub fn bytes(&self) -> usize {
		self.emojis.as_ref().map_or(0, custom_emoji_bytes)
			+ self.stickers.as_ref().map_or(0, sticker_bytes)
			+ size_of::<Self>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.emojis.as_ref().is_none_or(|emojis| {
				crate::valid_custom_emojis(emojis)
					&& emojis
						.iter()
						.all(|emoji| !emoji.available && !emoji.managed && emoji.roles.is_none())
			}) && self.stickers.as_ref().is_none_or(|stickers| {
			crate::valid_stickers(stickers, crate::MAX_GUILD_STICKERS)
				&& stickers
					.iter()
					.all(|sticker| sticker.guild_id == Some(self.guild) && !sticker.available)
		}) && self.bytes() <= 768 * 1024
	}
}

#[derive(Clone)]
pub struct InvitePreview {
	pub guild: Id,
	pub embed: Embed,
}

impl InvitePreview {
	pub fn bytes(&self) -> usize {
		size_of::<Self>() - size_of::<Embed>() + self.embed.bytes()
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct Guild {
	/// Service default: 0 = all messages, 1 = mentions; absent/invalid stays unknown.
	pub default_message_notifications: Option<u8>,
	pub stickers: Option<Vec<Sticker>>,
	pub emojis: Option<Vec<CustomEmoji>>,
	pub id: Id,
	pub name: String,
	pub icon: Option<String>,
}

impl Guild {
	pub fn bytes(&self) -> usize {
		std::mem::size_of::<Self>()
			+ self.name.capacity()
			+ self.icon.as_ref().map_or(0, String::capacity)
			+ self.emojis.as_ref().map_or(0, custom_emoji_bytes)
			+ self.stickers.as_ref().map_or(0, sticker_bytes)
	}
	pub fn icon_key(&self) -> Option<String> {
		self.icon
			.as_deref()
			.filter(|hash| valid_avatar_hash(hash))
			.map(|hash| format!("guild-{}-{hash}", self.id))
	}
}

#[derive(Clone)]
pub struct GuildPatch {
	pub default_message_notifications: Patch<u8>,
	pub id: Id,
	pub name: Patch<String>,
	pub icon: Patch<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Channel {
	/// Group DM icon hash; absent for groups using the default icon.
	pub icon: Option<String>,
	pub last_message: Option<Id>,
	pub id: Id,
	pub guild: Option<Id>,
	pub parent_id: Option<Id>,
	pub position: i32,
	pub name: String,
	pub kind: u8,
	pub recipients: Vec<User>,
	/// Unofficial service member-list identity; absent when permission metadata is missing.
	pub member_list_id: Option<String>,
	/// Thread reply count reported by the service; None for non-threads or unknown.
	pub message_count: Option<u32>,
	/// Forum tags offered by a forum or media channel, or applied to one of its posts.
	pub tags: Option<Box<forum::Tags>>,
}

impl Channel {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.name.capacity()
			+ self.tags.as_ref().map_or(0, |tags| tags.bytes())
			+ self.icon.as_ref().map_or(0, String::capacity)
			+ self.member_list_id.as_ref().map_or(0, String::capacity)
			+ self.recipients.capacity() * size_of::<User>()
			+ self.recipients.iter().map(User::heap_bytes).sum::<usize>()
	}
	pub fn supports_text(&self) -> bool {
		matches!(self.kind, 0..=3 | 5 | 10..=12)
	}
}

#[derive(Clone)]
pub struct ChannelPatch {
	pub icon: Patch<String>,
	pub last_message: Patch<Id>,
	pub id: Id,
	pub name: Patch<String>,
	pub parent_id: Patch<Id>,
	pub position: Patch<i32>,
	pub kind: Patch<u8>,
	pub message_count: Patch<u32>,
	/// Channel updates carry whole objects, so present tags replace the known ones.
	pub tags: Patch<Box<forum::Tags>>,
}
