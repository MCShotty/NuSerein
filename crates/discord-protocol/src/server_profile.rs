//! Unofficial own server-profile fields from Discord Userdoccers' guild resource.
use crate::{DecodeError, UserDto, profile};
use model::{
	Id, Patch,
	server_profile::{Edit, Snapshot, Values},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize, Default)]
struct Metadata {
	#[serde(default)]
	guild_id: Option<Id>,
	#[serde(default)]
	banner: Option<String>,
	#[serde(default)]
	bio: Option<String>,
	#[serde(default)]
	pronouns: Option<String>,
	#[serde(default)]
	accent_color: Option<u32>,
	#[serde(default)]
	theme_colors: Option<[u32; 2]>,
}
#[derive(Deserialize)]
struct Member {
	#[serde(default)]
	display_name_styles: Option<NameStyle>,
	#[serde(default)]
	user: Option<UserDto>,
	#[serde(default)]
	nick: Option<String>,
	#[serde(default)]
	avatar: Option<String>,
	#[serde(default)]
	banner: Option<String>,
	#[serde(default)]
	bio: Option<String>,
	#[serde(default)]
	pronouns: Option<String>,
}
#[derive(Deserialize)]
struct NameStyle {
	font_id: u32,
	effect_id: u32,
	colors: Vec<u32>,
}
impl NameStyle {
	fn model(self) -> model::ProfileNameStyle {
		let mut colors = self.colors;
		colors.shrink_to_fit();
		model::ProfileNameStyle {
			font: self.font_id,
			effect: self.effect_id,
			colors,
		}
	}
}

pub fn decode(bytes: &[u8], user: Id, guild: Id) -> Result<Snapshot, DecodeError> {
	#[derive(Deserialize)]
	struct Own {
		guild_member: Member,
		#[serde(default)]
		guild_member_profile: Option<Metadata>,
	}
	if user.0 == 0 || guild.0 == 0 || bytes.len() > profile::MAX_PROFILE_WIRE {
		return Err(DecodeError);
	}
	let own: Own = crate::decode(bytes)?;
	if own
		.guild_member
		.user
		.as_ref()
		.is_some_and(|member| member.id != user)
		|| own
			.guild_member_profile
			.as_ref()
			.and_then(|metadata| metadata.guild_id)
			.is_some_and(|id| id != guild)
	{
		return Err(DecodeError);
	}
	let display = profile::decode_profile_appearance(bytes, Some(guild), false, false)?;
	if display.user.id != user
		|| display
			.guild
			.as_ref()
			.is_none_or(|member| member.guild != guild)
	{
		return Err(DecodeError);
	}
	let metadata = own.guild_member_profile.unwrap_or(Metadata {
		banner: own.guild_member.banner,
		bio: own.guild_member.bio,
		pronouns: own.guild_member.pronouns,
		..Metadata::default()
	});
	let snapshot = Snapshot {
		profile: display,
		values: Values {
			nick: own.guild_member.nick,
			avatar: own.guild_member.avatar,
			banner: metadata.banner,
			bio: metadata.bio,
			pronouns: metadata.pronouns,
			accent_color: metadata.accent_color,
			theme_colors: metadata.theme_colors,
			name_style: own.guild_member.display_name_styles.map(NameStyle::model),
		},
	};
	if !snapshot.valid() {
		return Err(DecodeError);
	}
	Ok(snapshot)
}

/// Member and profile routes are separate writes; untouched keys stay absent.
pub fn encode(edit: &Edit) -> Result<(Value, Value), DecodeError> {
	fn field<T: serde::Serialize>(fields: &mut Map<String, Value>, key: &str, value: &Patch<T>) {
		match value {
			Patch::Absent => {}
			Patch::Null => {
				fields.insert(key.into(), Value::Null);
			}
			Patch::Value(value) => {
				fields.insert(key.into(), json!(value));
			}
		}
	}
	if !edit.valid() {
		return Err(DecodeError);
	}
	let mut member = Map::new();
	let mut profile = Map::new();
	field(&mut member, "nick", &edit.nick);
	field(&mut member, "avatar", &edit.avatar);
	match &edit.name_style {
		Patch::Absent => {}
		Patch::Null => profile::encode_name_style(&mut member, None),
		Patch::Value(style) => profile::encode_name_style(&mut member, Some(style)),
	}
	field(&mut profile, "banner", &edit.banner);
	field(&mut profile, "bio", &edit.bio);
	field(&mut profile, "pronouns", &edit.pronouns);
	field(&mut profile, "accent_color", &edit.accent_color);
	field(&mut profile, "theme_colors", &edit.theme_colors);
	Ok((Value::Object(member), Value::Object(profile)))
}

pub fn confirm_member(bytes: &[u8], user: Id) -> Result<(), DecodeError> {
	if bytes.len() > profile::MAX_PROFILE_WIRE {
		return Err(DecodeError);
	}
	let member: Member = crate::decode(bytes)?;
	if member
		.user
		.is_none_or(|value| value.id != user || value.bot)
	{
		return Err(DecodeError);
	}
	Ok(())
}
pub fn confirm_profile(bytes: &[u8], guild: Id) -> Result<(), DecodeError> {
	if bytes.len() > profile::MAX_PROFILE_WIRE {
		return Err(DecodeError);
	}
	let metadata: Metadata = crate::decode(bytes)?;
	if metadata.guild_id != Some(guild) {
		return Err(DecodeError);
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn server_profile_theme_and_style_use_scoped_viewing_and_global_editor_fallback() {
		let wire=json!({"user":{"id":"1","username":"synthetic","display_name_styles":{"font_id":11,"effect_id":1,"colors":[1]}},"user_profile":{"theme_colors":[1,2]},"guild_member":{"display_name_styles":{"font_id":3,"effect_id":2,"colors":[3,4]}},"guild_member_profile":{"guild_id":"2","theme_colors":[3,4]}}).to_string();
		let display = profile::decode_profile(wire.as_bytes(), Some(Id(2)), false).unwrap();
		assert_eq!(display.theme_colors, Some([3, 4]));
		assert_eq!(
			model::ProfileNameStyle::from_user(&display.user)
				.unwrap()
				.font,
			3
		);
		let snapshot = decode(wire.as_bytes(), Id(1), Id(2)).unwrap();
		assert_eq!(snapshot.profile.theme_colors, Some([1, 2]));
		assert_eq!(
			model::ProfileNameStyle::from_user(&snapshot.profile.user)
				.unwrap()
				.font,
			11
		);
		assert_eq!(snapshot.values.name_style.as_ref().unwrap().font, 3);
		let edit = Edit {
			name_style: Patch::Value(snapshot.values.name_style.unwrap()),
			..Default::default()
		};
		let (member, profile) = encode(&edit).unwrap();
		assert_eq!(
			member,
			json!({"display_name_font_id":3,"display_name_effect_id":2,"display_name_colors":[3,4]})
		);
		assert_eq!(profile, json!({}));
		let clear = encode(&Edit {
			name_style: Patch::Null,
			..Default::default()
		})
		.unwrap()
		.0;
		assert_eq!(
			clear,
			json!({"display_name_font_id":null,"display_name_effect_id":null,"display_name_colors":null})
		);
	}
	#[test]
	fn own_server_values_keep_inheritance_and_reject_wrong_or_incomplete_scopes() {
		let base = json!({
			"user":{"id":"1","username":"synthetic"},
			"user_profile":{"bio":"global bio","pronouns":"global pronouns"},
			"guild_member":{"user":{"id":"1","username":"synthetic"},"nick":null,"avatar":null},
			"guild_member_profile":{"guild_id":"2","bio":"","pronouns":null}
		});
		let read = |value: &Value| decode(value.to_string().as_bytes(), Id(1), Id(2));
		let saved = read(&base).unwrap();
		assert_eq!(saved.values.bio, Some(String::new()));
		assert_eq!(saved.values.pronouns, None);
		assert_eq!(saved.profile.bio, "global bio");
		for field in ["guild_member_profile", "guild_member"] {
			let mut value = base.clone();
			value[field] = Value::Null;
			if field == "guild_member_profile" {
				assert!(read(&value).is_ok(), "no overrides inherits global profile");
			} else {
				assert!(read(&value).is_err());
			}
		}
		for (field, value) in [
			("guild_member_profile", json!({"guild_id":"3"})),
			(
				"guild_member",
				json!({"user":{"id":"3","username":"other"}}),
			),
			("user_profile", Value::Null),
			("private", json!(true)),
			(
				"guild_member_profile",
				json!({"guild_id":"2","bio":"x".repeat(191)}),
			),
			(
				"guild_member_profile",
				json!({"guild_id":"2","theme_colors":[0, 0x1000000]}),
			),
		] {
			let mut wrong = base.clone();
			wrong[field] = value;
			assert!(read(&wrong).is_err());
		}
	}
	#[test]
	fn edits_partition_routes_and_preserve_explicit_nulls_and_empty_text() {
		let edit = Edit {
			nick: Patch::Null,
			avatar: Patch::Null,
			bio: Patch::Value(String::new()),
			pronouns: Patch::Null,
			theme_colors: Patch::Value([0x123456, 0xabcdef]),
			..Edit::default()
		};
		let (member, profile) = encode(&edit).unwrap();
		assert_eq!(member, json!({"nick":null,"avatar":null}));
		assert_eq!(
			profile,
			json!({"bio":"","pronouns":null,"theme_colors":[0x123456,0xabcdef]})
		);
		assert!(confirm_member(br#"{"user":{"id":"1","username":"synthetic"}}"#, Id(1)).is_ok());
		assert!(confirm_member(br#"{"user":{"id":"2","username":"other"}}"#, Id(1)).is_err());
		assert!(confirm_profile(br#"{"guild_id":"2"}"#, Id(2)).is_ok());
		assert!(confirm_profile(br#"{}"#, Id(2)).is_err());
	}
}
