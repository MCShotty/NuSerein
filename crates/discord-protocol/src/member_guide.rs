//! Unofficial normal-account Server Guide progress schema, published by discord-userdoccers.
use crate::{DecodeError, community_admin as configuration};
use model::{
	Id,
	community_admin::{Data, Guide, Section},
	member_guide::Snapshot,
};
use serde_json::Value;
pub fn progress(guild: Id, user: Id, guide: &Guide, bytes: &[u8]) -> Result<Vec<Id>, DecodeError> {
	if bytes.is_empty() {
		return Ok(vec![]);
	}
	let raw = configuration::raw(bytes)?;
	for (key, expected) in [("guild_id", guild), ("user_id", user)] {
		if raw
			.get(key)
			.and_then(Value::as_str)
			.and_then(|s| s.parse::<Id>().ok())
			!= Some(expected)
		{
			return Err(DecodeError);
		}
	}
	let actions = raw
		.get("channel_actions")
		.and_then(Value::as_object)
		.ok_or(DecodeError)?;
	if actions.len() > 512 {
		return Err(DecodeError);
	}
	let mut completed = vec![];
	for (channel, progress) in actions {
		let channel = channel.parse::<Id>().map_err(|_| DecodeError)?;
		let done = progress
			.get("completed")
			.and_then(Value::as_bool)
			.ok_or(DecodeError)?;
		if done && guide.actions.iter().any(|r| r.channel == channel) {
			completed.push(channel);
		}
	}
	Ok(completed)
}
pub fn snapshot(
	guild: Id,
	user: Id,
	guide: &[u8],
	progress_bytes: &[u8],
) -> Result<Snapshot, DecodeError> {
	let raw = configuration::raw(guide)?;
	let data = configuration::parse(guild, Section::Guide, &raw, false, false)?;
	let Data::Guide(guide) = data.data else {
		return Err(DecodeError);
	};
	let completed = progress(guild, user, &guide, progress_bytes)?;
	let snapshot = Snapshot {
		guild,
		user,
		guide,
		completed,
		header_known: false,
	};
	if !snapshot.valid() {
		return Err(DecodeError);
	}
	Ok(snapshot)
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn member_guide_progress_is_identity_bound_and_drops_legacy_channels() {
		let guide = Guide {
			actions: vec![model::community_admin::GuideChannel {
				channel: Id(3),
				title: "Read".into(),
				..Default::default()
			}],
			..Default::default()
		};
		assert_eq!(progress(Id(1),Id(2),&guide,br#"{"guild_id":"1","user_id":"2","channel_actions":{"3":{"completed":true},"4":{"completed":true}}}"#).unwrap(),vec![Id(3)]);
		assert!(
			progress(
				Id(1),
				Id(2),
				&guide,
				br#"{"guild_id":"1","user_id":"9","channel_actions":{}}"#
			)
			.is_err()
		);
		assert!(
			progress(
				Id(1),
				Id(2),
				&guide,
				br#"{"channel_actions":{"3":{"completed":true}}}"#
			)
			.is_err()
		);
	}
}
