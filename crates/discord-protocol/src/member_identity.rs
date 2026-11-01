//! Bounded presentation updates, separate from the strict self-permission decoder.
use crate::{DecodeError, UserDto, decode};
use model::{Id, MemberIdentity, Patch};
use serde::Deserialize;
use serde_json::value::RawValue;

pub const MAX_WIRE: usize = 32 * 1024;

/// Invalid optional artwork does not discard a usable member identity.
pub fn avatar_patch<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> Result<Patch<String>, D::Error> {
	let raw = Box::<RawValue>::deserialize(deserializer)?;
	if raw.get() == "null" {
		return Ok(Patch::Null);
	}
	Ok(serde_json::from_str::<String>(raw.get())
		.ok()
		.filter(|hash| model::valid_avatar_hash(hash))
		.map_or(Patch::Absent, Patch::Value))
}

#[derive(Deserialize)]
struct Fields<'a> {
	#[serde(default, borrow)]
	user: Option<&'a RawValue>,
	#[serde(default)]
	user_id: Option<Id>,
	#[serde(default, borrow)]
	nick: Patch<&'a RawValue>,
	#[serde(default, borrow)]
	roles: Option<&'a RawValue>,
	#[serde(default, borrow)]
	avatar: Patch<&'a RawValue>,
}

/// `user` is optional in embedded members; the enclosing typing/message identity is authoritative.
pub fn embedded(bytes: &[u8], guild: Id, user: Id) -> Result<MemberIdentity, DecodeError> {
	if bytes.len() > MAX_WIRE {
		return Err(DecodeError);
	}
	let fields: Fields<'_> = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	if fields.user_id.is_some_and(|id| id != user) {
		return Err(DecodeError);
	}
	let (profile, global_name) = match fields.user {
		Some(raw) => {
			let identity: crate::UserIdentity = decode(raw.get().as_bytes())?;
			if identity.id != user {
				return Err(DecodeError);
			}
			decode::<UserDto>(raw.get().as_bytes())
				.ok()
				.map(|dto| {
					let change = match &dto.global_name {
						Patch::Absent => model::GlobalNameUpdate::Unchanged,
						Patch::Null => model::GlobalNameUpdate::Cleared,
						Patch::Value(_) => model::GlobalNameUpdate::Set,
					};
					(Some(dto.into_model()), change)
				})
				.unwrap_or((None, model::GlobalNameUpdate::Unchanged))
		}
		None => (None, model::GlobalNameUpdate::Unchanged),
	};
	// Invalid optional presentation fields are ignored, never converted into a clearing update.
	let nick = match fields.nick {
		Patch::Value(raw) => decode::<String>(raw.get().as_bytes())
			.ok()
			.filter(|nick| {
				nick.len() <= 512
					&& nick.chars().count() <= 128
					&& !nick.chars().any(char::is_control)
			})
			.map_or(Patch::Absent, Patch::Value),
		Patch::Null => Patch::Null,
		Patch::Absent => Patch::Absent,
	};
	#[derive(Deserialize)]
	struct Roles(#[serde(deserialize_with = "crate::permissions::member_roles")] Vec<Id>);
	let roles = fields
		.roles
		.and_then(|raw| decode::<Roles>(raw.get().as_bytes()).ok())
		.map_or(Patch::Absent, |roles| Patch::Value(roles.0));
	let avatar = match fields.avatar {
		Patch::Value(raw) => decode::<String>(raw.get().as_bytes())
			.ok()
			.filter(|hash| model::valid_avatar_hash(hash))
			.map_or(Patch::Absent, Patch::Value),
		Patch::Null => Patch::Null,
		Patch::Absent => Patch::Absent,
	};
	let update = MemberIdentity {
		global_name,
		avatar,
		guild,
		user,
		profile,
		nick,
		roles,
		departed: false,
	};
	if !update.valid() {
		return Err(DecodeError);
	}
	Ok(update)
}

pub fn update(bytes: &[u8]) -> Result<MemberIdentity, DecodeError> {
	#[derive(Deserialize)]
	struct Scope {
		guild_id: Id,
		user: crate::UserIdentity,
	}
	if bytes.len() > MAX_WIRE {
		return Err(DecodeError);
	}
	let scope: Scope = decode(bytes)?;
	embedded(bytes, scope.guild_id, scope.user.id)
}

pub fn removed(bytes: &[u8]) -> Result<MemberIdentity, DecodeError> {
	let mut update = update(bytes)?;
	update.departed = true;
	update.profile = None;
	update.nick = Patch::Null;
	update.roles = Patch::Value(vec![]);
	update.avatar = Patch::Null;
	Ok(update)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn partial_user_names_distinguish_absent_null_and_value() {
		for (wire, expected_name, change) in [
			(
				br#"{"guild_id":"1","user":{"id":"2","username":"new_user"}}"#.as_slice(),
				"Existing Display",
				model::GlobalNameUpdate::Unchanged,
			),
			(
				br#"{"guild_id":"1","user":{"id":"2","username":"new_user","global_name":null}}"#.as_slice(),
				"new_user",
				model::GlobalNameUpdate::Cleared,
			),
			(
				br#"{"guild_id":"1","user":{"id":"2","username":"new_user","global_name":"Updated Display"}}"#.as_slice(),
				"Updated Display",
				model::GlobalNameUpdate::Set,
			),
		] {
			let update = update(wire).unwrap();
			assert_eq!(update.global_name, change);
			let mut user = update.profile.as_ref().unwrap().clone();
			user.name = "Existing Display".into();
			user.username = Some("old_user".into());
			update.apply_profile(&mut user);
			assert_eq!(user.name, expected_name);
			assert_eq!(user.username.as_deref(), Some("new_user"));
		}
	}

	#[test]
	fn member_avatar_updates_keep_absent_null_and_scoped_value_distinct() {
		let value = update(br#"{"guild_id":"1","user":{"id":"2","username":"account"},"avatar":"0123456789abcdef0123456789abcdef"}"#).unwrap();
		assert_eq!(
			value.avatar,
			Patch::Value("0123456789abcdef0123456789abcdef".into())
		);
		let cleared = update(br#"{"guild_id":"1","user":{"id":"2"},"avatar":null}"#).unwrap();
		assert_eq!(cleared.avatar, Patch::Null);
		let invalid = update(br#"{"guild_id":"1","user":{"id":"2"},"avatar":"../wrong"}"#).unwrap();
		assert_eq!(invalid.avatar, Patch::Absent);
		assert_eq!(
			update(br#"{"guild_id":"1","user":{"id":"2"}}"#)
				.unwrap()
				.avatar,
			Patch::Absent
		);
	}
	#[test]
	fn identity_preserves_partial_and_explicit_clear_semantics() {
		let partial =
			update(br#"{"guild_id":"1","user":{"id":"2"},"nick":"Guild nickname"}"#).unwrap();
		assert!(partial.profile.is_none());
		assert_eq!(partial.roles, Patch::Absent);
		assert_eq!(partial.nick, Patch::Value("Guild nickname".into()));
		let cleared = update(br#"{"guild_id":"1","user":{"id":"2","username":"account","global_name":"Display"},"nick":null,"roles":[]}"#).unwrap();
		assert_eq!(cleared.nick, Patch::Null);
		assert_eq!(cleared.roles, Patch::Value(vec![]));
		assert_eq!(
			cleared.profile.unwrap().username.as_deref(),
			Some("account")
		);
		let absent = embedded(br#"{}"#, Id(1), Id(2)).unwrap();
		assert_eq!(absent.nick, Patch::Absent);
		assert_eq!(absent.roles, Patch::Absent);
		let invalid =
			update(br#"{"guild_id":"1","user":{"id":"2"},"nick":{},"roles":["3","3"]}"#).unwrap();
		assert_eq!(invalid.nick, Patch::Absent);
		assert_eq!(invalid.roles, Patch::Absent);
		assert!(embedded(br#"{"user":{"id":"3"}}"#, Id(1), Id(2)).is_err());
		assert!(embedded(&vec![b' '; MAX_WIRE + 1], Id(1), Id(2)).is_err());
		assert!(
			removed(br#"{"guild_id":"1","user":{"id":"2"}}"#)
				.unwrap()
				.departed
		);
	}
}
