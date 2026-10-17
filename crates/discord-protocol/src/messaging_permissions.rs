//! Unofficial settings-proto/1 interoperability; schema extracted by discord-userdoccers.
//! https://github.com/discord-userdoccers/discord-protos/blob/master/discord_protos/discord_users/v1/PreloadedUserSettings.proto
pub use crate::guild_folders::MAX_SETTINGS_RESPONSE;
use crate::{
	DecodeError,
	guild_folders::{fields, integer_wrapper, message},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use model::{
	Id,
	messaging_permissions::{Change, MAX_GUILDS, Snapshot},
	privacy_preferences::{self, ProfileVisibility, Redaction},
};
use serde::Deserialize;
const MAX_SUBTREE: usize = 128 * 1024;
pub struct Settings {
	pub version: u64,
	pub snapshot: Snapshot,
	text: Vec<u8>,
	privacy: Vec<u8>,
}
pub fn decode_response(bytes: &[u8]) -> Result<Settings, DecodeError> {
	#[derive(Deserialize)]
	struct Response {
		settings: String,
		#[serde(default)]
		out_of_date: bool,
	}
	if bytes.len() > MAX_SETTINGS_RESPONSE {
		return Err(DecodeError);
	}
	let response: Response = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	if response.out_of_date {
		return Err(DecodeError);
	}
	let wire = STANDARD
		.decode(response.settings)
		.map_err(|_| DecodeError)?;
	let (mut version, mut text, mut privacy) = (None, None, None);
	for field in fields(&wire)? {
		match field.number {
			1 => {
				if version.is_some() {
					return Err(DecodeError);
				}
				let mut data = None;
				for field in fields(field.message()?)? {
					if field.number == 3 {
						if data.is_some() {
							return Err(DecodeError);
						}
						data = Some(u32::try_from(field.integer()?).map_err(|_| DecodeError)?);
					}
				}
				version = Some(u64::from(data.unwrap_or_default()));
			}
			6 | 8 => {
				let target = if field.number == 6 {
					&mut text
				} else {
					&mut privacy
				};
				if target.is_some() {
					return Err(DecodeError);
				}
				let value = field.message()?;
				if value.len() > MAX_SUBTREE {
					return Err(DecodeError);
				}
				*target = Some(value);
			}
			_ => {}
		}
	}
	let text = text.unwrap_or_default();
	let privacy = privacy.unwrap_or_default();
	let mut snapshot = Snapshot::default();
	let mut seen = 0u64;
	for field in fields(text)? {
		if matches!(field.number, 27 | 29 | 30 | 32 | 35) {
			unique(&mut seen, field.number)?;
		}
		if field.number == 27 {
			snapshot.spam_filter = u32::try_from(field.integer()?).map_err(|_| DecodeError)?;
		}
		if matches!(field.number, 29 | 32 | 35) {
			let values = match field.number {
				29 => &mut snapshot.privacy.sensitive,
				32 => &mut snapshot.privacy.gore,
				_ => &mut snapshot.privacy.self_harm,
			};
			let mut seen = 0;
			for item in fields(field.message()?)? {
				if (1..=3).contains(&item.number) {
					unique(&mut seen, item.number)?;
					values[item.number as usize - 1] = Redaction::from_wire(
						u32::try_from(item.integer()?).map_err(|_| DecodeError)?,
					);
				}
			}
		}
		if field.number == 30 {
			let mut seen = 0;
			for item in fields(field.message()?)? {
				if (1..=3).contains(&item.number) {
					unique(&mut seen, item.number)?;
					snapshot.privacy.keywords[item.number as usize - 1] =
						Some(boolean(u64::from(wrapper(item.message()?)?))?);
				}
			}
		}
	}
	seen = 0;
	let mut restricted_v2 = None;
	for field in fields(privacy)? {
		if matches!(
			field.number,
			4 | 8 | 11 | 17 | 19 | 23 | 26 | 27 | 28 | 31 | 32 | 33
		) {
			unique(&mut seen, field.number)?;
		}
		match field.number {
			3 | 16 => {
				let ids = if field.number == 3 {
					&mut snapshot.restricted_guilds
				} else {
					&mut snapshot.unfiltered_guilds
				};
				// Accept both packed and unpacked repeated fixed64 fields.
				let value = if field.raw[0] & 7 == 1 {
					&field.raw[field.raw.len() - 8..]
				} else {
					field.message()?
				};
				if value.len() % 8 != 0 || ids.len() + value.len() / 8 > MAX_GUILDS {
					return Err(DecodeError);
				}
				for bytes in value.as_chunks::<8>().0 {
					let id = Id(u64::from_le_bytes(*bytes));
					if id.0 == 0 {
						return Err(DecodeError);
					}
					ids.push(id);
				}
			}
			4 => snapshot.default_allow_dms = !boolean(field.integer()?)?,
			8 | 19 | 23 | 31 => {
				let value = Some(boolean(u64::from(wrapper(field.message()?)?))?);
				match field.number {
					8 => snapshot.privacy.detect_platform_accounts = value,
					19 => snapshot.privacy.non_spam_training = value,
					23 => snapshot.privacy.conversation_warnings = value,
					_ => snapshot.privacy.show_local_time = value,
				}
			}
			11 => snapshot.friend_source_flags = wrapper(field.message()?)?,
			17 => {
				snapshot.default_filter_requests = !boolean(u64::from(wrapper(field.message()?)?))?
			}
			26 => snapshot.game_friend_dms = boolean(u64::from(wrapper(field.message()?)?))?,
			27 => restricted_v2 = Some(boolean(u64::from(wrapper(field.message()?)?))?),
			28 => snapshot.game_dms = u32::try_from(field.integer()?).map_err(|_| DecodeError)?,
			32 => {
				snapshot.privacy.profile_visibility = ProfileVisibility::from_wire(
					u32::try_from(field.integer()?).map_err(|_| DecodeError)?,
				)
			}
			33 => snapshot.personalized_requests = !boolean(u64::from(wrapper(field.message()?)?))?,
			_ => {}
		}
	}
	if let Some(restricted) = restricted_v2 {
		snapshot.default_allow_dms = !restricted;
	}
	for ids in [
		&mut snapshot.restricted_guilds,
		&mut snapshot.unfiltered_guilds,
	] {
		ids.sort_unstable();
		ids.dedup();
		ids.shrink_to_fit();
	}
	if !snapshot.valid() {
		return Err(DecodeError);
	}
	Ok(Settings {
		version: version.ok_or(DecodeError)?,
		snapshot,
		text: text.to_vec(),
		privacy: privacy.to_vec(),
	})
}
fn unique(seen: &mut u64, number: u64) -> Result<(), DecodeError> {
	let bit = 1 << number;
	if *seen & bit != 0 {
		return Err(DecodeError);
	}
	*seen |= bit;
	Ok(())
}
fn wrapper(bytes: &[u8]) -> Result<u32, DecodeError> {
	let mut value = None;
	for field in fields(bytes)? {
		if field.number == 1 {
			if value.is_some() {
				return Err(DecodeError);
			}
			value = Some(u32::try_from(field.integer()?).map_err(|_| DecodeError)?);
		}
	}
	Ok(value.unwrap_or_default())
}
fn boolean(value: u64) -> Result<bool, DecodeError> {
	match value {
		0 => Ok(false),
		1 => Ok(true),
		_ => Err(DecodeError),
	}
}
fn integer(number: u64, value: u32, output: &mut Vec<u8>) {
	for mut value in [number << 3, u64::from(value)] {
		while value >= 128 {
			output.push(value as u8 | 0x80);
			value >>= 7;
		}
		output.push(value as u8);
	}
}
pub fn encode_patch(current: &Settings, change: &Change) -> Result<String, DecodeError> {
	if !change.valid() {
		return Err(DecodeError);
	}
	if let Change::Privacy(change) = change {
		return encode_privacy_patch(current, *change);
	}
	let mut wanted = current.snapshot.clone();
	change.apply(&mut wanted);
	if !wanted.valid() {
		return Err(DecodeError);
	}
	let numbers: &[u64] = match change {
		Change::Privacy(_) => return Err(DecodeError),
		Change::SpamFilter(_) => &[27],
		Change::DefaultAllowDms(_) => &[4, 27],
		Change::AllowGuildDms(..) => &[3],
		Change::AllowAllDms { .. } => &[3, 4, 27],
		Change::DefaultFilterRequests(_) => &[17],
		Change::FilterGuildRequests(..) => &[16],
		Change::FilterAllRequests { .. } => &[16, 17],
		Change::Everyone(_) | Change::FriendsOfFriends(_) | Change::ServerMembers(_) => &[11],
		Change::PersonalizedRequests(_) => &[33],
		Change::GameFriendDms(_) => &[26],
		Change::GameDms(_) => &[28],
	};
	let text = matches!(change, Change::SpamFilter(_));
	let mut subtree = Vec::new();
	for field in fields(if text {
		&current.text
	} else {
		&current.privacy
	})? {
		if !numbers.contains(&field.number) {
			subtree.extend_from_slice(field.raw);
		}
	}
	for &number in numbers {
		match number {
			3 | 16 => {
				let ids = if number == 3 {
					&wanted.restricted_guilds
				} else {
					&wanted.unfiltered_guilds
				};
				let packed: Vec<_> = ids.iter().flat_map(|id| id.0.to_le_bytes()).collect();
				message(number, &packed, &mut subtree);
			}
			4 => integer(number, u32::from(!wanted.default_allow_dms), &mut subtree),
			27 if text => integer(number, wanted.spam_filter, &mut subtree),
			28 => integer(number, wanted.game_dms, &mut subtree),
			_ => {
				let value = match number {
					11 => wanted.friend_source_flags,
					17 => u32::from(!wanted.default_filter_requests),
					26 => u32::from(wanted.game_friend_dms),
					27 => u32::from(!wanted.default_allow_dms),
					33 => u32::from(!wanted.personalized_requests),
					_ => return Err(DecodeError),
				};
				integer_wrapper(number, u64::from(value), &mut subtree);
			}
		}
	}
	if subtree.len() > MAX_SUBTREE {
		return Err(DecodeError);
	}
	let mut patch = Vec::new();
	message(if text { 6 } else { 8 }, &subtree, &mut patch);
	Ok(STANDARD.encode(patch))
}

fn encode_privacy_patch(
	current: &Settings,
	change: privacy_preferences::Change,
) -> Result<String, DecodeError> {
	use privacy_preferences::Change as C;
	let (text, number) = change.field();
	let source = if text {
		&current.text
	} else {
		&current.privacy
	};
	let mut subtree = Vec::new();
	let mut previous = None;
	for field in fields(source)? {
		if field.number != number {
			subtree.extend_from_slice(field.raw);
		} else if text {
			previous = Some(field.message()?);
		}
	}
	match change {
		C::Content { .. } | C::Keyword { .. } => {
			let child = match change {
				C::Content { audience, .. } => audience.index() + 1,
				C::Keyword { keyword, .. } => keyword.index() + 1,
				_ => return Err(DecodeError),
			} as u64;
			let mut nested = Vec::new();
			for field in fields(previous.unwrap_or_default())? {
				if field.number != child {
					nested.extend_from_slice(field.raw);
				}
			}
			match change {
				C::Content { level, .. } => integer(child, level.wire(), &mut nested),
				C::Keyword { enabled, .. } => {
					integer_wrapper(child, u64::from(enabled), &mut nested)
				}
				_ => return Err(DecodeError),
			}
			message(number, &nested, &mut subtree);
		}
		C::ProfileVisibility(value) => integer(number, value.wire(), &mut subtree),
		C::ConversationWarnings(value)
		| C::ShowLocalTime(value)
		| C::DetectPlatformAccounts(value)
		| C::NonSpamTraining(value) => integer_wrapper(number, u64::from(value), &mut subtree),
	}
	if subtree.len() > MAX_SUBTREE {
		return Err(DecodeError);
	}
	let mut patch = Vec::new();
	message(if text { 6 } else { 8 }, &subtree, &mut patch);
	Ok(STANDARD.encode(patch))
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::privacy_preferences::{Audience, Change as C, ContentKind, Keyword};

	fn response(text: &[u8], privacy: &[u8]) -> Vec<u8> {
		let mut root = vec![10, 2, 24, 7];
		message(6, text, &mut root);
		message(8, privacy, &mut root);
		serde_json::to_vec(&serde_json::json!({"settings":STANDARD.encode(root)})).unwrap()
	}
	#[test]
	fn privacy_edits_preserve_unknown_fields_and_other_audiences() {
		let mut nested = Vec::new();
		integer(1, 1, &mut nested);
		integer(2, 2, &mut nested);
		integer(3, 91, &mut nested);
		message(9, b"future child", &mut nested);
		let mut keywords = Vec::new();
		integer_wrapper(1, 0, &mut keywords);
		integer_wrapper(2, 1, &mut keywords);
		message(9, b"future keyword", &mut keywords);
		let mut text = Vec::new();
		for kind in [
			ContentKind::Sensitive,
			ContentKind::Gore,
			ContentKind::SelfHarm,
		] {
			message(kind.field(), &nested, &mut text);
		}
		message(30, &keywords, &mut text);
		message(40, b"future text", &mut text);
		let mut privacy = Vec::new();
		integer(32, 91, &mut privacy);
		integer_wrapper(19, 0, &mut privacy);
		message(40, b"future privacy", &mut privacy);
		let current = decode_response(&response(&text, &privacy)).unwrap();
		assert_eq!(
			current.snapshot.privacy.profile_visibility,
			ProfileVisibility::Unknown(91)
		);
		assert_eq!(
			current.snapshot.privacy.sensitive[2],
			Redaction::Unknown(91)
		);
		let mut changes = vec![
			C::ProfileVisibility(ProfileVisibility::Friends),
			C::ConversationWarnings(true),
			C::ShowLocalTime(false),
			C::DetectPlatformAccounts(false),
			C::NonSpamTraining(true),
		];
		for kind in [
			ContentKind::Sensitive,
			ContentKind::Gore,
			ContentKind::SelfHarm,
		] {
			for audience in [Audience::Servers, Audience::Friends, Audience::Others] {
				changes.push(C::Content {
					kind,
					audience,
					level: Redaction::Block,
				});
			}
		}
		for keyword in [Keyword::Profanity, Keyword::SexualContent, Keyword::Slurs] {
			changes.push(C::Keyword {
				keyword,
				enabled: true,
			});
		}
		for change in changes {
			let encoded = encode_patch(&current, &Change::Privacy(change)).unwrap();
			let patch = STANDARD.decode(encoded).unwrap();
			let top = fields(&patch).unwrap();
			assert_eq!(top.len(), 1, "only one changed subtree is sent");
			let (is_text, number) = change.field();
			assert_eq!(top[0].number, if is_text { 6 } else { 8 });
			let subtree = top[0].message().unwrap();
			assert!(
				fields(subtree)
					.unwrap()
					.iter()
					.any(|field| field.number == 40),
				"unknown sibling retained"
			);
			if is_text {
				let inner = fields(subtree)
					.unwrap()
					.into_iter()
					.find(|field| field.number == number)
					.unwrap();
				assert!(
					fields(inner.message().unwrap())
						.unwrap()
						.iter()
						.any(|field| field.number == 9),
					"unknown nested sibling retained"
				);
			}
			let saved = if is_text {
				decode_response(&response(subtree, &privacy))
			} else {
				decode_response(&response(&text, subtree))
			}
			.unwrap();
			let mut wanted = current.snapshot.clone();
			change.apply(&mut wanted.privacy);
			assert_eq!(saved.snapshot, wanted);
		}
		for change in [
			C::ProfileVisibility(ProfileVisibility::Unknown(99)),
			C::ProfileVisibility(ProfileVisibility::Default),
			C::Content {
				kind: ContentKind::Sensitive,
				audience: Audience::Friends,
				level: Redaction::Default,
			},
		] {
			assert!(encode_patch(&current, &Change::Privacy(change)).is_err());
		}
	}
	#[test]
	fn duplicate_or_malformed_known_preferences_fail_closed() {
		let mut nested = Vec::new();
		integer(1, 2, &mut nested);
		integer(1, 3, &mut nested);
		let mut text = Vec::new();
		message(29, &nested, &mut text);
		assert!(decode_response(&response(&text, &[])).is_err());
		let mut privacy = Vec::new();
		integer(32, 1, &mut privacy);
		integer(32, 2, &mut privacy);
		assert!(decode_response(&response(&[], &privacy)).is_err());
		privacy.clear();
		integer_wrapper(23, 2, &mut privacy);
		assert!(decode_response(&response(&[], &privacy)).is_err());
	}
}
