//! UI-neutral session entities. No filesystem or network dependencies.
/// User-facing product name. Storage, executable and OS identifiers retain their legacy names.
pub const APP_NAME: &str = "NuSerein";

mod users;
pub use users::{AccountKind, GuildAvatar, SavedAccount, User, valid_avatar_hash};
mod navigation;
pub use navigation::{Channel, ChannelPatch, ExpressionCatalog, Guild, GuildPatch, InvitePreview};
mod messages;
pub use messages::{Interaction, Message, MessagePatch, valid_mention_roles};
mod presence;
pub use presence::{
	ActivityImage, ClientPlatforms, MemberPresence, OwnPresence, PresenceStatus, RichActivity,
};
mod members;
pub use members::{
	GlobalNameUpdate, Member, MemberDisplay, MemberIdentity, MemberList, MemberSlot,
	MissingMemberFields,
};
use presence::valid_presence_text;
pub mod account;
pub mod account_settings;
pub mod application_commands;
pub mod automod;
pub mod command_permissions;
pub mod community_admin;
mod image_sharing;
pub mod moderation;

pub mod inbox;
pub mod polls;
pub mod public_upload;
pub mod registered_games;
pub mod reporting;
pub use image_sharing::ImageShare;
pub mod archives;
mod channel_preferences;
pub mod channel_visibility;
pub mod guild_preferences;
pub mod keybinds;
pub mod member_guide;
pub mod messaging_permissions;
pub mod notification_preferences;
pub mod onboarding;
pub mod privacy_preferences;
pub mod profile_cosmetics;
pub mod voice_diagnostics;
pub mod voice_settings;
pub use channel_preferences::{ChannelPreferences, PreferenceEdit, Shortcut};
pub use keybinds::{KeyChord, KeybindAction, Keybinds};
pub mod forum;
pub mod gifs;
mod graphics;
pub use graphics::{GpuPreference, VideoAdapter, VideoAdapterIdentity};
mod time_format;
pub use time_format::TimeFormat;
mod time_zone;
pub use time_zone::TimeZonePreference;
pub mod guild_folders;
pub mod message_options;
pub mod permissions;
mod reading_preferences;
pub mod server_access;
pub mod server_admin;
pub mod server_audit_log;
pub mod server_integrations;
pub mod server_invites;
pub mod server_profile;
pub mod server_roles;
pub mod server_settings;
pub mod soundboard;
pub use reading_preferences::ReadingPreferences;
mod reply_preview;
pub use reply_preview::{MAX_REPLY_PREVIEW_BYTES, ReplyPreview};
mod cache_preferences;
pub use cache_preferences::*;
pub mod badges;
mod cosmetics;
pub use cosmetics::*;
mod profile;
mod system_messages;
pub use profile::*;
pub use system_messages::{Segment, SystemMessage};
mod attachments;
pub use attachments::*;
mod components;
pub use components::*;
mod embeds;
pub use embeds::*;
mod extra_content;
pub use extra_content::{ExtraContent, ExtraContentPatch};
mod mentions;
pub use mentions::*;
mod stickers;
pub use stickers::*;
mod reactions;
pub use reactions::*;
mod search;
pub use gifs::*;
pub use search::*;
pub mod community;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub u64);
impl fmt::Display for Id {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}
impl FromStr for Id {
	type Err = &'static str;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
			return Err("Invalid Discord ID");
		}
		s.parse::<u64>()
			.ok()
			.filter(|v| *v != 0)
			.map(Self)
			.ok_or("Invalid Discord ID")
	}
}
impl Serialize for Id {
	fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(&self.to_string())
	}
}
impl<'de> Deserialize<'de> for Id {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		struct IdVisitor;
		impl serde::de::Visitor<'_> for IdVisitor {
			type Value = Id;
			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("a Discord ID string")
			}
			fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Id, E> {
				value.parse().map_err(E::custom)
			}
		}
		d.deserialize_str(IdVisitor)
	}
}
/// Bounded roster: enough for people juggling alternates, small enough to stay readable.
pub const MAX_SAVED_ACCOUNTS: usize = 8;
pub const MAX_MENTION_ROLES: usize = 100;
/// Missing differs from explicit null in partial service updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Patch<T> {
	#[default]
	Absent,
	Null,
	Value(T),
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		Ok(Option::<T>::deserialize(d)?.map_or(Self::Null, Self::Value))
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
	Loading,
	Fresh,
	Stale,
	Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
	Sending,
	Confirmed,
	Rejected,
	Ambiguous,
}

pub const MAX_RICH_ACTIVITIES: usize = 4;

pub const MAX_ACTIVITY_TIMESTAMP: u64 = 9_007_199_254_740_991;

#[cfg(test)]
mod presence_tests {
	use super::*;

	#[test]
	fn own_presence_bounds_unicode_and_allows_explicit_clear() {
		let mut presence = OwnPresence::default();
		assert!(presence.valid());
		assert_eq!(
			PresenceStatus::ALL.map(PresenceStatus::wire),
			["online", "idle", "dnd", "invisible"]
		);
		for text in [
			" x".into(),
			"x ".into(),
			"line\nfeed".into(),
			"\0".into(),
			"x".repeat(129),
			"🦀".repeat(129),
		] {
			presence.custom_status = text;
			assert!(!presence.valid());
		}
		presence.custom_status = "🦀".repeat(128);
		assert!(presence.valid());
		assert_eq!(presence.custom_status.len(), 512);
		presence.custom_status.clear();
		assert!(presence.valid());
	}

	#[test]
	fn rich_presence_validates_retained_fields_and_accounts_for_allocations() {
		let activity = RichActivity {
			kind: 0,
			name: "Synthetic".into(),
			details: Some("Level 2".into()),
			state: Some("In a party".into()),
			image: None,
			small_image: None,
			ends_at: None,
			started_at: None,
		};
		let mut presence = MemberPresence {
			user: Id(2),
			status: None,
			custom_status: None,
			activities: vec![activity.clone(); MAX_RICH_ACTIVITIES],
			clients: ClientPlatforms::default(),
		};
		assert!(presence.valid());
		assert_eq!(
			presence.heap_bytes(),
			presence.activities.capacity() * size_of::<RichActivity>()
				+ presence
					.activities
					.iter()
					.map(RichActivity::heap_bytes)
					.sum::<usize>()
		);
		presence.activities.push(activity.clone());
		assert!(!presence.valid());
		for text in [
			"",
			" padded",
			"control\n",
			&"x".repeat(129),
			&"🌙".repeat(129),
		] {
			let mut invalid = activity.clone();
			invalid.name = text.into();
			assert!(!invalid.valid());
			invalid.name = activity.name.clone();
			invalid.details = Some(text.into());
			assert!(!invalid.valid());
			invalid.details = None;
			invalid.state = Some(text.into());
			assert!(!invalid.valid());
		}
		for kind in [4, 6, 255] {
			assert!(
				!RichActivity {
					kind,
					..activity.clone()
				}
				.valid()
			);
		}
		let mut allocated = activity;
		allocated.name.reserve(100);
		let mut path = String::from("external/synthetic-hash-01/https/example.com/art.png");
		path.reserve(2048);
		allocated.image = Some(ActivityImage::Proxy(path));
		let mut small_path = String::from("external/synthetic-small/https/example.com/badge.png");
		small_path.reserve(1024);
		allocated.small_image = Some(ActivityImage::Proxy(small_path));
		assert!(allocated.valid());
		assert_eq!(
			allocated.heap_bytes(),
			allocated.name.capacity()
				+ allocated.details.as_ref().unwrap().capacity()
				+ allocated.state.as_ref().unwrap().capacity()
				+ allocated.image.as_ref().unwrap().heap_bytes()
				+ allocated.small_image.as_ref().unwrap().heap_bytes()
		);
		allocated.started_at = Some(MAX_ACTIVITY_TIMESTAMP);
		assert!(allocated.valid());
		allocated.started_at = Some(MAX_ACTIVITY_TIMESTAMP + 1);
		assert!(!allocated.valid());
		allocated.started_at = None;
		allocated.ends_at = Some(1000);
		assert!(allocated.valid());
		allocated.started_at = Some(1000);
		assert!(!allocated.valid());
		allocated.started_at = None;
		allocated.ends_at = Some(MAX_ACTIVITY_TIMESTAMP + 1);
		assert!(!allocated.valid());
		allocated.ends_at = None;
		allocated.small_image = Some(ActivityImage::Proxy("external/../secret".into()));
		assert!(!allocated.valid());
	}

	#[test]
	fn activity_image_keys_preserve_only_bounded_service_references() {
		for (image, key) in [
			(
				ActivityImage::Asset {
					application: Id(10),
					asset: Id(20),
				},
				"activity-10-20",
			),
			(ActivityImage::Application(Id(10)), "app-icon-10"),
			(
				ActivityImage::Proxy("external/synthetic-hash-01/https/example.com/art.png".into()),
				"embed:https://media.discordapp.net/external/synthetic-hash-01/https/example.com/art.png",
			),
		] {
			assert!(image.valid());
			assert_eq!(image.key(), key);
		}
		assert!(!ActivityImage::Application(Id(0)).valid());
		assert!(
			!ActivityImage::Asset {
				application: Id(1),
				asset: Id(0)
			}
			.valid()
		);
		assert!(!ActivityImage::Proxy("external/../secret".into()).valid());
		assert!(!ActivityImage::Proxy("x".repeat(1025)).valid());
	}
}

#[cfg(test)]
mod notification_metadata_tests {
	use super::*;
	#[test]
	fn role_mentions_bound_identity_count_and_reserved_allocation() {
		assert!(valid_mention_roles(&Vec::new()));
		assert!(valid_mention_roles(&(1..=100).map(Id).collect()));
		assert!(!valid_mention_roles(&vec![Id(0)]));
		assert!(!valid_mention_roles(&vec![Id(1), Id(1)]));
		assert!(!valid_mention_roles(&(1..=101).map(Id).collect()));
		assert!(!valid_mention_roles(&Vec::with_capacity(101)));
	}
}

#[cfg(test)]
mod saved_account_tests {
	use super::*;

	#[test]
	fn switcher_labels_preserve_unicode_within_the_existing_storage_bound() {
		for name in ["Synthetic".into(), "界".repeat(22), "🦀".repeat(32)] {
			let projected = SavedAccount::label_projection(&name);
			assert!(name.starts_with(projected));
			assert!(!projected.is_empty());
			assert!(projected.len() <= SavedAccount::MAX_LABEL_BYTES);
			assert_eq!(SavedAccount::label_projection(projected), projected);
			let account = SavedAccount {
				id: Id(9),
				name: projected.to_owned(),
				display: Some(projected.to_owned()),
				avatar: None,
				discriminator: 0,
				has_token: true,
			};
			assert!(account.is_valid());
			assert_eq!(account.user().id, Id(9));
			assert!(account.has_token);
		}
		let split_scalar = format!("{}é", "x".repeat(63));
		assert_eq!(
			SavedAccount::label_projection(&split_scalar),
			"x".repeat(63)
		);
		assert_eq!(SavedAccount::label_projection(""), "");
	}
}
