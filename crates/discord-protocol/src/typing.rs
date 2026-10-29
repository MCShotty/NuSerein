//! Ephemeral typing with optional bounded guild identity, never nested media or profiles.
use crate::DecodeError;
use model::Id;
use serde::Deserialize;

pub const MAX_WIRE: usize = 16 * 1024;

pub struct TypingStart {
	pub channel_id: Id,
	pub user_id: Id,
	/// Service timestamp in Unix seconds; freshness belongs to the active session.
	pub timestamp: u64,
	pub member: Option<Box<model::MemberIdentity>>,
}

pub fn decode(bytes: &[u8]) -> Result<TypingStart, DecodeError> {
	if bytes.len() > MAX_WIRE {
		return Err(DecodeError);
	}
	#[derive(Deserialize)]
	struct Wire<'a> {
		channel_id: Id,
		user_id: Id,
		timestamp: u64,
		#[serde(default)]
		guild_id: Option<Id>,
		#[serde(default, borrow)]
		member: Option<&'a serde_json::value::RawValue>,
	}
	let wire: Wire<'_> = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	let member = wire
		.guild_id
		.zip(wire.member)
		.and_then(|(guild, member)| {
			crate::member_identity::embedded(member.get().as_bytes(), guild, wire.user_id).ok()
		})
		.map(Box::new);
	Ok(TypingStart {
		channel_id: wire.channel_id,
		user_id: wire.user_id,
		timestamp: wire.timestamp,
		member,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn typing_retains_matching_member_and_ignores_bad_optional_identity() {
		let wire = br#"{"guild_id":"1","channel_id":"2","user_id":"3","timestamp":100,"member":{"user":{"id":"3","username":"account"},"nick":"Guild nickname","roles":["4"]}}"#;
		let typing = decode(wire).unwrap();
		let member = typing.member.unwrap();
		assert_eq!(member.nick, model::Patch::Value("Guild nickname".into()));
		assert_eq!(member.roles, model::Patch::Value(vec![Id(4)]));
		assert_eq!((member.guild, member.user), (Id(1), Id(3)));
		let mut wire: serde_json::Value = serde_json::from_slice(wire).unwrap();
		wire["member"]["user"]["id"] = "9".into();
		assert!(
			decode(&serde_json::to_vec(&wire).unwrap())
				.unwrap()
				.member
				.is_none()
		);
		wire["member"] = serde_json::json!({"nick":"Guild nickname"});
		assert_eq!(
			decode(&serde_json::to_vec(&wire).unwrap())
				.unwrap()
				.member
				.unwrap()
				.nick,
			model::Patch::Value("Guild nickname".into())
		);
	}

	#[test]
	fn typing_retains_only_bounded_precise_identity_and_timestamp() {
		let wire = br#"{"channel_id":"18446744073709551615","user_id":"2","timestamp":18446744073709551615,"guild_id":"3","member":{"user":{"username":"discarded synthetic profile"},"roles":["4"]}}"#;
		let typing = decode(wire).unwrap();
		assert_eq!(
			(typing.channel_id, typing.user_id, typing.timestamp),
			(Id(u64::MAX), Id(2), u64::MAX)
		);
		let mut padded = wire.to_vec();
		padded.resize(MAX_WIRE, b' ');
		assert!(decode(&padded).is_ok());
		padded.push(b' ');
		assert!(decode(&padded).is_err());
		for invalid in [
			r#"{"channel_id":"0","user_id":"2","timestamp":1}"#,
			r#"{"channel_id":"1","user_id":"0","timestamp":1}"#,
			r#"{"channel_id":1,"user_id":"2","timestamp":1}"#,
			r#"{"channel_id":"1","user_id":"2"}"#,
			r#"{"channel_id":"1","user_id":"2","timestamp":null}"#,
			r#"{"channel_id":"1","user_id":"2","timestamp":-1}"#,
			r#"{"channel_id":"1","user_id":"2","timestamp":1.5}"#,
			r#"{"channel_id":"1","user_id":"2","timestamp":"1"}"#,
			r#"{"channel_id":"1","user_id":"2","timestamp":18446744073709551616}"#,
			"{",
		] {
			assert!(decode(invalid.as_bytes()).is_err());
		}
	}
}
