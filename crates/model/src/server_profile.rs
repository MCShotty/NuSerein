//! The account's own server overrides. No profile metadata is persisted.
use crate::{Id, Patch, UserProfile};

pub const MAX_SNAPSHOT_BYTES: usize = crate::MAX_PROFILE_BYTES + 4096;
pub const MAX_EDIT_BYTES: usize = 4096 + 2 * crate::MAX_PROFILE_AVATAR_URI;

/// `None` inherits the account profile; an empty string is an explicit empty override.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Values {
	pub nick: Option<String>,
	pub avatar: Option<String>,
	pub banner: Option<String>,
	pub bio: Option<String>,
	pub pronouns: Option<String>,
	pub accent_color: Option<u32>,
	pub theme_colors: Option<[u32; 2]>,
	pub name_style: Option<crate::ProfileNameStyle>,
}
impl Values {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ [
				&self.nick,
				&self.avatar,
				&self.banner,
				&self.bio,
				&self.pronouns,
			]
			.into_iter()
			.map(|text| text.as_ref().map_or(0, String::capacity))
			.sum::<usize>()
			+ self
				.name_style
				.as_ref()
				.map_or(0, crate::ProfileNameStyle::bytes)
	}
	pub fn valid(&self) -> bool {
		self.bytes() <= 4096
			&& self
				.name_style
				.as_ref()
				.is_none_or(crate::ProfileNameStyle::valid_data)
			&& self
				.nick
				.as_deref()
				.is_none_or(|text| valid_text(text, 32, false))
			&& self
				.bio
				.as_deref()
				.is_none_or(|text| valid_text(text, 190, true))
			&& self
				.pronouns
				.as_deref()
				.is_none_or(|text| valid_text(text, 40, false))
			&& [&self.avatar, &self.banner]
				.into_iter()
				.all(|hash| hash.as_deref().is_none_or(crate::valid_avatar_hash))
			&& colors_valid(self.accent_color, self.theme_colors)
	}
}

#[derive(Clone)]
pub struct Snapshot {
	/// Contains the global fallback plus this server's display metadata.
	pub profile: UserProfile,
	pub values: Values,
}
impl Snapshot {
	pub fn guild(&self) -> Option<Id> {
		self.profile.guild.as_ref().map(|guild| guild.guild)
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.profile.bytes() + self.values.bytes()
	}
	pub fn valid(&self) -> bool {
		self.guild().is_some()
			&& !self.profile.private
			&& !self.profile.limited
			&& self.profile.valid()
			&& self.values.valid()
			&& self.bytes() <= MAX_SNAPSHOT_BYTES
	}
}

/// Only deliberate edits are sent; `Null` removes the override.
/// Debug output intentionally does not expose profile text or image data.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Edit {
	pub nick: Patch<String>,
	pub avatar: Patch<String>,
	pub banner: Patch<String>,
	pub bio: Patch<String>,
	pub pronouns: Patch<String>,
	pub accent_color: Patch<u32>,
	pub theme_colors: Patch<[u32; 2]>,
	pub name_style: Patch<crate::ProfileNameStyle>,
}
impl Edit {
	pub fn is_empty(&self) -> bool {
		self == &Self::default()
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ [
				&self.nick,
				&self.avatar,
				&self.banner,
				&self.bio,
				&self.pronouns,
			]
			.into_iter()
			.map(|value| match value {
				Patch::Value(text) => text.capacity(),
				_ => 0,
			})
			.sum::<usize>()
			+ match &self.name_style {
				Patch::Value(style) => style.bytes(),
				_ => 0,
			}
	}
	pub fn valid(&self) -> bool {
		let text = |value: &Patch<String>, limit, multiline| match value {
			Patch::Value(value) => valid_text(value, limit, multiline),
			_ => true,
		};
		self.bytes() <= MAX_EDIT_BYTES
			&& !matches!(&self.name_style,Patch::Value(style) if !style.valid())
			&& text(&self.nick, 32, false)
			&& !matches!(&self.nick, Patch::Value(nick) if nick.trim().is_empty())
			&& text(&self.bio, 190, true)
			&& text(&self.pronouns, 40, false)
			&& [&self.avatar, &self.banner]
				.into_iter()
				.all(|image| match image {
					Patch::Value(uri) => crate::valid_avatar_uri(uri),
					_ => true,
				}) && !matches!(self.accent_color, Patch::Value(color) if color > 0xff_ffff)
			&& !matches!(self.theme_colors, Patch::Value(colors) if colors.iter().any(|color| *color > 0xff_ffff))
	}
	/// Setting rich server overrides requires the full Nitro entitlement reported by READY.
	/// Removing old overrides remains possible after an entitlement expires.
	pub fn needs_nitro(&self) -> bool {
		matches!(self.avatar, Patch::Value(_))
			|| matches!(self.banner, Patch::Value(_))
			|| matches!(self.bio, Patch::Value(_))
			|| matches!(self.accent_color, Patch::Value(_))
			|| matches!(self.theme_colors, Patch::Value(_))
			|| matches!(self.name_style, Patch::Value(_))
	}
	pub fn matches(&self, saved: &Snapshot) -> bool {
		fn same<T: PartialEq>(edit: &Patch<T>, value: &Option<T>) -> bool {
			match edit {
				Patch::Absent => true,
				Patch::Null => value.is_none(),
				Patch::Value(expected) => value.as_ref() == Some(expected),
			}
		}
		fn image(edit: &Patch<String>, value: &Option<String>) -> bool {
			match edit {
				Patch::Absent => true,
				Patch::Null => value.is_none(),
				Patch::Value(_) => value.is_some(),
			}
		}
		let value = &saved.values;
		same(&self.nick, &value.nick)
			&& image(&self.avatar, &value.avatar)
			&& image(&self.banner, &value.banner)
			&& same(&self.bio, &value.bio)
			&& same(&self.pronouns, &value.pronouns)
			&& same(&self.accent_color, &value.accent_color)
			&& same(&self.theme_colors, &value.theme_colors)
			&& same(&self.name_style, &value.name_style)
	}
}

fn valid_text(value: &str, limit: usize, multiline: bool) -> bool {
	value.len() <= limit * 4
		&& value.chars().count() <= limit
		&& value
			.chars()
			.all(|c| !c.is_control() || (multiline && matches!(c, '\n' | '\r' | '\t')))
}
fn colors_valid(accent: Option<u32>, theme: Option<[u32; 2]>) -> bool {
	accent.is_none_or(|color| color <= 0xff_ffff)
		&& theme.is_none_or(|colors| colors.iter().all(|color| *color <= 0xff_ffff))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn edits_bound_text_images_and_entitlements_without_losing_clears() {
		let mut edit = Edit {
			nick: Patch::Value("🦀".repeat(32)),
			pronouns: Patch::Value("🦀".repeat(40)),
			..Edit::default()
		};
		assert!(edit.valid() && !edit.needs_nitro());
		edit.bio = Patch::Value("🦀".repeat(190));
		assert!(edit.valid() && edit.needs_nitro());
		edit.bio = Patch::Value("x".repeat(191));
		assert!(!edit.valid());
		edit.bio = Patch::Null;
		edit.avatar = Patch::Null;
		edit.banner = Patch::Null;
		edit.accent_color = Patch::Null;
		edit.theme_colors = Patch::Null;
		assert!(edit.valid() && !edit.needs_nitro());
		edit.avatar = Patch::Value("data:image/png;base64,iVBORw0KGgo=".into());
		assert!(edit.valid());
		edit.banner = Patch::Value("https://cdn.discordapp.com/not-an-upload".into());
		assert!(!edit.valid());
		edit.banner = Patch::Absent;
		edit.nick = Patch::Value(" ".into());
		assert!(!edit.valid());
		edit.nick = Patch::Null;
		edit.pronouns = Patch::Value("they\nthem".into());
		assert!(!edit.valid());
		edit.pronouns = Patch::Absent;
		edit.theme_colors = Patch::Value([0, 0x100_0000]);
		assert!(!edit.valid());
	}
}
