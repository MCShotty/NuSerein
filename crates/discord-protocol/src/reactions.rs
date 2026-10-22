use model::{Id, Patch, Reaction, ReactionEmoji};
use serde::{
	Deserialize, Deserializer,
	de::{SeqAccess, Visitor},
};

/// Unofficial FrecencyUserSettings: reaction-specific ranking (13), or legacy emoji ranking (6).
/// Keys are Discord friendly names or custom-emoji snowflakes, resolved by the UI catalog.
pub fn account_frecency(bytes: &[u8]) -> Result<Vec<String>, crate::DecodeError> {
	use crate::{DecodeError, guild_folders::fields};
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	#[derive(Deserialize)]
	struct Response {
		settings: String,
		#[serde(default)]
		out_of_date: bool,
	}
	if bytes.len() > crate::gif_favorites::MAX_RESPONSE {
		return Err(DecodeError);
	}
	let response: Response = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	if response.out_of_date {
		return Err(DecodeError);
	}
	let wire = STANDARD
		.decode(response.settings)
		.map_err(|_| DecodeError)?;
	let mut reactions = None;
	let mut legacy = None;
	for field in fields(&wire)? {
		let target = match field.number {
			13 => &mut reactions,
			6 => &mut legacy,
			_ => continue,
		};
		if target.is_some() {
			return Err(DecodeError);
		}
		*target = Some(field.message()?);
	}
	let source = reactions.or(legacy).unwrap_or_default();
	if source.len() > 512 * 1024 {
		return Err(DecodeError);
	}
	let mut ranked = Vec::new();
	let mut keys = std::collections::BTreeSet::new();
	for field in fields(source)? {
		if field.number != 1 {
			continue;
		}
		if ranked.len() >= 2048 {
			return Err(DecodeError);
		}
		let entry = field.message()?;
		if entry.len() > 2048 {
			return Err(DecodeError);
		}
		let (mut key, mut value) = (None, None);
		for field in fields(entry)? {
			match field.number {
				1 if key.is_none() => {
					key = Some(std::str::from_utf8(field.message()?).map_err(|_| DecodeError)?)
				}
				2 if value.is_none() => value = Some(field.message()?),
				1 | 2 => return Err(DecodeError),
				_ => {}
			}
		}
		let key = key.ok_or(DecodeError)?;
		if key.is_empty()
			|| key.len() > 128
			|| key.chars().any(char::is_control)
			|| !keys.insert(key)
		{
			return Err(DecodeError);
		}
		let (mut total, mut last, mut frecency, mut score) = (0u32, 0u64, None, None);
		let mut seen = 0u8;
		for field in fields(value.unwrap_or_default())? {
			if matches!(field.number, 1 | 3 | 4) {
				let bit = 1 << field.number;
				if seen & bit != 0 {
					return Err(DecodeError);
				}
				seen |= bit;
			}
			match field.number {
				1 => total = u32::try_from(field.integer()?).map_err(|_| DecodeError)?,
				2 => {
					if let Ok(mut packed) = field.message() {
						if packed.len() > 640 {
							return Err(DecodeError);
						}
						while !packed.is_empty() {
							last = last.max(frecency_varint(&mut packed)?);
						}
					} else {
						last = last.max(field.integer()?);
					}
				}
				3 => frecency = Some(field.integer()? as u32 as i32),
				4 => score = Some(field.integer()? as u32 as i32),
				_ => {}
			}
		}
		ranked.push((
			key,
			score.or(frecency.filter(|v| *v >= 0)).unwrap_or(0),
			last,
			total,
		));
	}
	ranked.sort_by(|a, b| {
		b.1.cmp(&a.1)
			.then(b.2.cmp(&a.2))
			.then(b.3.cmp(&a.3))
			.then(a.0.cmp(b.0))
	});
	Ok(ranked
		.into_iter()
		.take(128)
		.map(|(key, ..)| key.to_owned())
		.collect())
}

fn frecency_varint(bytes: &mut &[u8]) -> Result<u64, crate::DecodeError> {
	let mut result = 0;
	for shift in (0..70).step_by(7) {
		let (&byte, rest) = bytes.split_first().ok_or(crate::DecodeError)?;
		*bytes = rest;
		if shift == 63 && byte > 1 {
			return Err(crate::DecodeError);
		}
		result |= u64::from(byte & 0x7f) << shift;
		if byte & 0x80 == 0 {
			return Ok(result);
		}
	}
	Err(crate::DecodeError)
}

#[derive(Default)]
pub struct ReactionList(pub Vec<Reaction>);
impl<'de> Deserialize<'de> for ReactionList {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct List;
		impl<'de> Visitor<'de> for List {
			type Value = ReactionList;
			fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
				f.write_str("a bounded list of reactions")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
				let mut items = Vec::new();
				while let Some(item) = seq.next_element::<Reaction>()? {
					if items.len() >= model::MAX_REACTIONS || !item.emoji.valid() || item.count == 0
					{
						return Err(serde::de::Error::custom("Invalid reaction list"));
					}
					items.push(item);
				}
				if !model::valid_reactions(&items) {
					return Err(serde::de::Error::custom("Duplicate reaction emoji"));
				}
				Ok(ReactionList(items))
			}
		}
		deserializer.deserialize_seq(List)
	}
}

#[derive(Deserialize)]
pub struct ReactionTarget {
	pub channel_id: model::Id,
	pub message_id: model::Id,
}

fn reaction_emoji<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ReactionEmoji, D::Error> {
	let mut emoji = ReactionEmoji::deserialize(deserializer)?;
	if !emoji.valid() {
		return Err(serde::de::Error::custom("Invalid reaction emoji"));
	}
	// Retain only the validated name, never spare capacity from the wire allocation.
	emoji.name = emoji.name.map(|name| name.as_str().to_owned());
	Ok(emoji)
}

#[derive(Deserialize)]
pub struct ReactionEmojiTarget {
	pub channel_id: Id,
	pub message_id: Id,
	#[serde(deserialize_with = "reaction_emoji")]
	pub emoji: ReactionEmoji,
}

#[derive(Deserialize)]
#[serde(try_from = "ReactionDeltaWire")]
pub struct ReactionDelta {
	pub channel_id: Id,
	pub message_id: Id,
	pub user_id: Id,
	pub emoji: ReactionEmoji,
	pub burst: bool,
}

#[derive(Deserialize)]
struct ReactionDeltaWire {
	channel_id: Id,
	message_id: Id,
	user_id: Id,
	#[serde(deserialize_with = "reaction_emoji")]
	emoji: ReactionEmoji,
	#[serde(default, rename = "type")]
	kind: Patch<u8>,
	#[serde(default)]
	burst: Patch<bool>,
}
impl TryFrom<ReactionDeltaWire> for ReactionDelta {
	type Error = &'static str;
	fn try_from(wire: ReactionDeltaWire) -> Result<Self, Self::Error> {
		// Documented types are NORMAL (0) and BURST (1). Older events omit type;
		// use their explicit burst flag, or normal when both fields are absent.
		let burst = match (wire.kind, wire.burst) {
			(Patch::Absent | Patch::Value(0), Patch::Absent | Patch::Value(false)) => false,
			(Patch::Absent | Patch::Value(1), Patch::Value(true))
			| (Patch::Value(1), Patch::Absent) => true,
			_ => return Err("Unsupported or conflicting reaction type"),
		};
		Ok(Self {
			channel_id: wire.channel_id,
			message_id: wire.message_id,
			user_id: wire.user_id,
			emoji: wire.emoji,
			burst,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::{ReactionDelta, ReactionEmojiTarget, ReactionTarget};
	use crate::{MessageDto, PatchDto, decode};
	use model::{Id, Patch};
	use serde_json::json;

	#[test]
	fn account_reaction_ranking_uses_synced_scores_and_preserves_legacy_fallback() {
		use crate::guild_folders::message;
		use base64::{Engine as _, engine::general_purpose::STANDARD};
		let entry = |key: &str, score: u8| {
			let mut e = Vec::new();
			message(1, key.as_bytes(), &mut e);
			message(2, &[8, 4, 18, 2, 42, 80, 24, 1, 32, score], &mut e);
			let mut map = Vec::new();
			message(1, &e, &mut map);
			map
		};
		let response =
			|wire: &[u8]| serde_json::to_vec(&json!({"settings": STANDARD.encode(wire)})).unwrap();
		let mut wire = Vec::new();
		message(6, &entry("fire", 110), &mut wire);
		assert_eq!(super::account_frecency(&response(&wire)).unwrap(), ["fire"]);
		let reactions = [entry("skull_crossbones", 20), entry("9001", 80)].concat();
		message(13, &reactions, &mut wire);
		assert_eq!(
			super::account_frecency(&response(&wire)).unwrap(),
			["9001", "skull_crossbones"]
		);
		let mut empty = Vec::new();
		message(13, &[], &mut empty);
		assert!(
			super::account_frecency(&response(&empty))
				.unwrap()
				.is_empty()
		);
		let mut duplicate = Vec::new();
		message(
			13,
			&[entry("9001", 1), entry("9001", 2)].concat(),
			&mut duplicate,
		);
		assert!(super::account_frecency(&response(&duplicate)).is_err());
		let mut invalid = Vec::new();
		message(13, &entry("bad\nkey", 2), &mut invalid);
		assert!(super::account_frecency(&response(&invalid)).is_err());
		assert!(super::account_frecency(br#"{"settings":"!"}"#).is_err());
		assert!(super::account_frecency(br#"{"settings":"","out_of_date":true}"#).is_err());
		assert!(super::account_frecency(&vec![0; crate::gif_favorites::MAX_RESPONSE + 1]).is_err());
		let many: Vec<_> = (1..=200).flat_map(|i| entry(&format!("{i}"), 1)).collect();
		let mut bounded = Vec::new();
		message(13, &many, &mut bounded);
		assert_eq!(
			super::account_frecency(&response(&bounded)).unwrap().len(),
			128
		);
	}

	#[test]
	fn reaction_deltas_validate_type_identity_and_bounded_emoji() {
		{
			let wire = json!({"channel_id":"2","message_id":"3","user_id":"4","emoji":{"id":null,"name":"x"}});
			for (fields, burst) in [
				(json!({}), false),
				(json!({"type":0,"burst":false}), false),
				(json!({"type":1,"burst":true}), true),
				(json!({"type":1}), true),
				(json!({"burst":true}), true),
			] {
				let mut value = wire.clone();
				value
					.as_object_mut()
					.unwrap()
					.extend(fields.as_object().unwrap().clone());
				let event: ReactionDelta = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
				assert_eq!(
					(event.channel_id, event.message_id, event.user_id),
					(Id(2), Id(3), Id(4))
				);
				assert_eq!(event.burst, burst);
				assert_eq!(event.emoji.name.as_deref(), Some("x"));
			}
			for fields in [
				json!({"type":2}),
				json!({"type":256}),
				json!({"type":-1}),
				json!({"type":"1"}),
				json!({"type":null}),
				json!({"burst":null}),
				json!({"type":0,"burst":true}),
				json!({"type":1,"burst":false}),
				json!({"user_id":"0"}),
				json!({"channel_id":"0"}),
				json!({"message_id":"0"}),
				json!({"emoji":{"id":"0","name":"x"}}),
				json!({"emoji":{"id":null,"name":null}}),
				json!({"emoji":{"id":null,"name":""}}),
				json!({"emoji":{"id":null,"name":"x\n"}}),
				json!({"emoji":{"id":null,"name":"x".repeat(129)}}),
				json!({"emoji":{"id":null,"name":"é".repeat(65)}}),
			] {
				let mut value = wire.clone();
				value
					.as_object_mut()
					.unwrap()
					.extend(fields.as_object().unwrap().clone());
				assert!(decode::<ReactionDelta>(&serde_json::to_vec(&value).unwrap()).is_err());
			}
			for emoji in [
				json!({"id":"5","name":null}),
				json!({"id":null,"name":"é".repeat(64)}),
			] {
				let mut value = wire.clone();
				value["emoji"] = emoji;
				let event: ReactionDelta = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
				assert!(
					event
						.emoji
						.name
						.as_ref()
						.is_none_or(|name| name.capacity() <= 128)
				);
				let cleared: ReactionEmojiTarget =
					decode(&serde_json::to_vec(&value).unwrap()).unwrap();
				assert_eq!(cleared.emoji, event.emoji);
			}
			assert!(
				decode::<ReactionEmojiTarget>(br#"{"channel_id":"2","message_id":"3"}"#).is_err()
			);
			assert!(decode::<ReactionTarget>(br#"{"channel_id":"2","message_id":"3"}"#).is_ok());
		}
		{
			let raw = serde_json::json!({"id":"1","channel_id":"2","author":{"id":"3","username":"Test"},
	            "reactions":[{"emoji":{"id":null,"name":"👍"},"count":3,"me":true,"me_burst":true},
	            {"emoji":{"id":"4","name":null},"count":1,"me":false}]});
			let message = decode::<MessageDto>(&serde_json::to_vec(&raw).unwrap())
				.unwrap()
				.into_model();
			let reactions = message.reactions.unwrap();
			assert!(reactions[0].me && reactions[0].me_burst);
			assert_eq!(reactions[1].emoji.id, Some(Id(4)));
			assert!(matches!(
				decode::<PatchDto>(br#"{"id":"1","channel_id":"2"}"#)
					.unwrap()
					.into_model()
					.reactions,
				Patch::Absent
			));
			assert!(matches!(
				decode::<PatchDto>(br#"{"id":"1","channel_id":"2","reactions":null}"#)
					.unwrap()
					.into_model()
					.reactions,
				Patch::Null
			));
			for values in [
				vec![raw["reactions"][0].clone(); 65],
				vec![raw["reactions"][0].clone(); 2],
				vec![
					serde_json::json!({"emoji":{"id":null,"name":"x".repeat(129)},"count":1,"me":false}),
				],
			] {
				let mut bad = raw.clone();
				bad["reactions"] = serde_json::json!(values);
				assert!(decode::<MessageDto>(&serde_json::to_vec(&bad).unwrap()).is_err());
			}
		}
	}
}

/// Decode with the catalog's retained item and allocation budgets before enqueueing.
pub struct CustomEmojiList(pub Vec<model::CustomEmoji>);
impl<'de> Deserialize<'de> for CustomEmojiList {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct List;
		impl<'de> Visitor<'de> for List {
			type Value = CustomEmojiList;
			fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
				f.write_str("a bounded guild emoji catalog")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
				let mut items = Vec::new();
				let mut bytes = 0;
				let mut ids = std::collections::BTreeSet::new();
				while let Some(item) = seq.next_element::<model::CustomEmoji>()? {
					bytes += item.heap_bytes();
					if items.len() >= model::MAX_GUILD_EMOJIS
						|| !item.valid() || !ids.insert(item.id)
					{
						return Err(serde::de::Error::custom("Invalid guild emoji catalog"));
					}
					items.push(item);
					if bytes + items.capacity() * std::mem::size_of::<model::CustomEmoji>()
						> model::MAX_GUILD_EMOJI_BYTES
					{
						return Err(serde::de::Error::custom(
							"Guild emoji catalog exceeds capacity",
						));
					}
				}
				Ok(CustomEmojiList(items))
			}
		}
		deserializer.deserialize_seq(List)
	}
}
#[derive(Deserialize)]
pub struct GuildEmojisUpdate {
	pub guild_id: model::Id,
	pub emojis: CustomEmojiList,
}

#[cfg(test)]
mod catalog_tests {
	use super::*;
	use crate::{GuildDto, Ready, decode};
	use serde_json::json;

	#[test]
	fn guild_catalog_preserves_availability_roles_and_unknown_vs_empty() {
		{
			let emoji =
				json!({"id":"4","name":"party_parrot","animated":true,"available":true,"roles":[]});
			let mut ready: Ready = decode(&serde_json::to_vec(&json!({
	            "user":{"id":"1","username":"Synthetic"},"session_id":"synthetic","resume_gateway_url":"wss://gateway.discord.gg",
	            "guilds":[{"id":"2","emojis":[emoji.clone()]},{"id":"3"},{"id":"5","emojis":[]}]
	        })).unwrap()).unwrap();
			let (guilds, _) = ready.navigation().unwrap();
			let custom = &guilds[0].emojis.as_ref().unwrap()[0];
			assert_eq!(custom.markup(), "<a:party_parrot:4>");
			assert!(custom.usable());
			assert!(guilds[1].emojis.is_none());
			assert_eq!(guilds[2].emojis.as_deref(), Some([].as_slice()));
			for (field, value) in [
				("roles", json!(["6"])),
				("roles", json!(null)),
				("available", json!(false)),
				("managed", json!(true)),
			] {
				let mut restricted = emoji.clone();
				restricted[field] = value;
				let guild: GuildDto =
					decode(&serde_json::to_vec(&json!({"id":"2","emojis":[restricted]})).unwrap())
						.unwrap();
				assert!(!guild.emojis.unwrap().0[0].usable());
			}
			let update: GuildEmojisUpdate = decode(br#"{"guild_id":"2","emojis":[]}"#).unwrap();
			assert!(update.emojis.0.is_empty());
		}
		{
			let emoji = json!({"id":"4","name":"wave","available":true,"roles":[]});
			let oversized: Vec<_> = (1..=model::MAX_GUILD_EMOJIS + 1)
				.map(|id| {
					let mut e = emoji.clone();
					e["id"] = json!(id.to_string());
					e
				})
				.collect();
			let large_roles: Vec<_> = (1..=256).map(|id| id.to_string()).collect();
			let over_bytes: Vec<_> = (1..=256)
				.map(|id| json!({"id":id.to_string(),"name":"wave","roles":large_roles}))
				.collect();
			for values in [
				vec![emoji.clone(), emoji],
				oversized,
				over_bytes,
				vec![json!({"id":"4","name":"../unsafe"})],
				vec![json!({"id":"4","name":"x".repeat(33)})],
				vec![json!({"id":"4","name":"wave","roles":vec!["5";257]})],
			] {
				assert!(
					decode::<GuildEmojisUpdate>(
						&serde_json::to_vec(&json!({"guild_id":"2","emojis":values})).unwrap()
					)
					.is_err()
				);
			}
		}
	}
}
