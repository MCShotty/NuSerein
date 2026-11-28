//! Unofficial settings-proto Soundboard fields. Only send the complete touched subtree.
//! Schema: discord-userdoccers/discord-protos@71b702ac90b90d6d75dbd29d67b209b6b2ba7c94.
use crate::{
	DecodeError,
	guild_folders::{Field, fields, fixed64_field, message},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use model::{Id, soundboard::Favorite};
use std::collections::{BTreeMap, HashSet};

pub const MAX_RESPONSE: usize = 6 * 1024 * 1024;
pub const MAX_REMOTE_FAVORITES: usize = 256;
const MAX_SUBTREE: usize = 512 * 1024;
const FAVORITES_FIELD: u64 = 8;
const GUILDS_FIELD: u64 = 3;

#[derive(serde::Deserialize)]
struct Response {
	settings: String,
	#[serde(default)]
	out_of_date: bool,
}

fn unique<'a>(list: &[Field<'a>], number: u64) -> Result<Option<&'a [u8]>, DecodeError> {
	let mut found = list.iter().filter(|field| field.number == number);
	let first = found.next().map(Field::message).transpose()?;
	if found.next().is_some() {
		return Err(DecodeError);
	}
	Ok(first)
}

fn decode(bytes: &[u8], number: u64) -> Result<(u32, Vec<u8>), DecodeError> {
	if bytes.len() > MAX_RESPONSE {
		return Err(DecodeError);
	}
	let response: Response = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	if response.out_of_date {
		return Err(DecodeError);
	}
	let wire = STANDARD
		.decode(response.settings)
		.map_err(|_| DecodeError)?;
	let top = fields(&wire)?;
	let versions = fields(unique(&top, 1)?.ok_or(DecodeError)?)?;
	let mut versions = versions.iter().filter(|field| field.number == 3);
	let version =
		u32::try_from(versions.next().ok_or(DecodeError)?.integer()?).map_err(|_| DecodeError)?;
	if versions.next().is_some() {
		return Err(DecodeError);
	}
	let subtree = unique(&top, number)?.unwrap_or_default();
	if subtree.len() > MAX_SUBTREE {
		return Err(DecodeError);
	}
	Ok((version, subtree.to_vec()))
}

fn encode(number: u64, subtree: &[u8]) -> Result<String, DecodeError> {
	if subtree.len() > MAX_SUBTREE {
		return Err(DecodeError);
	}
	let mut patch = Vec::new();
	message(number, subtree, &mut patch);
	Ok(STANDARD.encode(patch))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favorites {
	ids: Vec<Id>,
	ordered: Vec<Id>,
	other: Vec<u8>,
}
impl Favorites {
	pub fn decode(bytes: &[u8]) -> Result<(u32, Self), DecodeError> {
		let (version, raw) = decode(bytes, FAVORITES_FIELD)?;
		Ok((version, Self::parse(&raw)?))
	}
	fn parse(raw: &[u8]) -> Result<Self, DecodeError> {
		let mut result = Self::default();
		for field in fields(raw)? {
			let into = match field.number {
				1 => &mut result.ids,
				2 => &mut result.ordered,
				_ => {
					result.other.extend_from_slice(field.raw);
					continue;
				}
			};
			if let Ok(id) = field.fixed64() {
				if into.len() == MAX_REMOTE_FAVORITES {
					return Err(DecodeError);
				}
				into.push(Id(id));
			} else {
				let packed = field.message()?;
				let (ids, remainder) = packed.as_chunks::<8>();
				if !remainder.is_empty() || into.len() + ids.len() > MAX_REMOTE_FAVORITES {
					return Err(DecodeError);
				}
				into.extend(ids.iter().map(|id| Id(u64::from_le_bytes(*id))));
			}
		}
		for ids in [&result.ids, &result.ordered] {
			let mut seen = HashSet::new();
			if ids.iter().any(|&id| id.0 == 0 || !seen.insert(id)) {
				return Err(DecodeError);
			}
		}
		if result.favorite_ids().len() > MAX_REMOTE_FAVORITES {
			return Err(DecodeError);
		}
		Ok(result)
	}
	pub fn favorite_ids(&self) -> Vec<Id> {
		let mut ids = self.ordered.clone();
		ids.extend(
			self.ids
				.iter()
				.copied()
				.filter(|id| !self.ordered.contains(id)),
		);
		ids
	}
	pub fn change(&mut self, id: Id, favorite: bool) -> Result<(), DecodeError> {
		if id.0 == 0 {
			return Err(DecodeError);
		}
		if favorite {
			let ids = self.favorite_ids();
			if !ids.contains(&id) {
				if ids.len() == MAX_REMOTE_FAVORITES {
					return Err(DecodeError);
				}
				// Honor whichever representations the service actually supplied. A new list uses both.
				let empty = self.ids.is_empty() && self.ordered.is_empty();
				if empty || !self.ids.is_empty() {
					self.ids.push(id);
				}
				if empty || !self.ordered.is_empty() {
					self.ordered.push(id);
				}
			}
		} else {
			self.ids.retain(|value| *value != id);
			self.ordered.retain(|value| *value != id);
		}
		Ok(())
	}
	pub fn patch(&self) -> Result<String, DecodeError> {
		let mut subtree = self.other.clone();
		for (number, ids) in [(1, &self.ids), (2, &self.ordered)] {
			if !ids.is_empty() {
				let packed: Vec<_> = ids.iter().flat_map(|id| id.0.to_le_bytes()).collect();
				message(number, &packed, &mut subtree);
			}
		}
		encode(FAVORITES_FIELD, &subtree)
	}
}

/// The zero key is a documented-by-client account sentinel, never a server ID.
#[derive(Clone, Copy)]
pub enum EntranceTarget {
	Global,
	Server(Id),
}
impl EntranceTarget {
	fn key(self) -> Result<Id, DecodeError> {
		match self {
			Self::Global => Ok(Id(0)),
			Self::Server(id) if id.0 != 0 => Ok(id),
			Self::Server(_) => Err(DecodeError),
		}
	}
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entrance {
	pub sound: Option<Favorite>,
	other_guilds: BTreeMap<Id, Vec<u8>>,
	other_top_fields: Vec<u8>,
	other_guild_fields: Vec<u8>,
	map_entry_fields: Vec<u8>,
	other_sound_fields: Vec<u8>,
}
impl Entrance {
	pub fn decode(
		bytes: &[u8],
		target: EntranceTarget,
	) -> Result<(u32, Self, Option<Favorite>), DecodeError> {
		let (version, raw) = decode(bytes, GUILDS_FIELD)?;
		let selected = Self::parse(&raw, target)?;
		let fallback = selected
			.other_guilds
			.get(&Id(0))
			.map(|raw| Self::parse(raw, EntranceTarget::Global).map(|global| global.sound))
			.transpose()?
			.flatten();
		Ok((version, selected, fallback))
	}
	fn parse(raw: &[u8], target: EntranceTarget) -> Result<Self, DecodeError> {
		let guild = target.key()?;
		let mut result = Self::default();
		let mut seen = HashSet::new();
		for field in fields(raw)? {
			if field.number != 1 {
				result.other_top_fields.extend_from_slice(field.raw);
				continue;
			}
			let entry = fields(field.message()?)?;
			let key = fixed(&entry, 1)?.unwrap_or(Id(0));
			if !seen.insert(key) {
				return Err(DecodeError);
			}
			if key != guild {
				result.other_guilds.insert(key, field.raw.to_vec());
				continue;
			}
			let value = unique(&entry, 2)?.unwrap_or_default();
			for field in entry.iter().filter(|field| !matches!(field.number, 1 | 2)) {
				result.map_entry_fields.extend_from_slice(field.raw);
			}
			let settings = fields(value)?;
			if let Some(sound) = unique(&settings, 6)? {
				let sound = fields(sound)?;
				let id = fixed(&sound, 1)?
					.filter(|id| id.0 != 0)
					.ok_or(DecodeError)?;
				let source = fixed(&sound, 2)?.filter(|id| id.0 != 0);
				result.sound = Some((source, id));
				for field in sound.iter().filter(|field| !matches!(field.number, 1 | 2)) {
					result.other_sound_fields.extend_from_slice(field.raw);
				}
			}
			for field in settings.iter().filter(|field| field.number != 6) {
				result.other_guild_fields.extend_from_slice(field.raw);
			}
		}
		Ok(result)
	}
	pub fn change(&mut self, sound: Option<Favorite>) -> Result<(), DecodeError> {
		if sound.is_some_and(|(source, id)| id.0 == 0 || source.is_some_and(|id| id.0 == 0))
			|| (sound.is_none() && !self.other_sound_fields.is_empty())
		{
			return Err(DecodeError);
		}
		self.sound = sound;
		Ok(())
	}
	pub fn patch(&self, target: EntranceTarget) -> Result<String, DecodeError> {
		let guild = target.key()?;
		let mut guild_value = self.other_guild_fields.clone();
		if let Some((source, id)) = self.sound {
			let mut sound = self.other_sound_fields.clone();
			fixed64_field(1, id.0, &mut sound);
			if let Some(source) = source {
				fixed64_field(2, source.0, &mut sound);
			}
			message(6, &sound, &mut guild_value);
		}
		let mut top = self.other_top_fields.clone();
		for entry in self.other_guilds.values() {
			top.extend_from_slice(entry);
		}
		if !guild_value.is_empty() || !self.map_entry_fields.is_empty() {
			let mut entry = self.map_entry_fields.clone();
			fixed64_field(1, guild.0, &mut entry);
			message(2, &guild_value, &mut entry);
			message(1, &entry, &mut top);
		}
		encode(GUILDS_FIELD, &top)
	}
}

fn fixed(list: &[Field<'_>], number: u64) -> Result<Option<Id>, DecodeError> {
	let mut matches = list.iter().filter(|field| field.number == number);
	let value = matches
		.next()
		.map(|field| field.fixed64().map(Id))
		.transpose()?;
	if matches.next().is_some() {
		return Err(DecodeError);
	}
	Ok(value)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn response(top: &[u8]) -> Vec<u8> {
		let mut wire = Vec::new();
		message(1, &[24, 7], &mut wire); // Versions.data_version = 7
		wire.extend_from_slice(top);
		serde_json::to_vec(&serde_json::json!({"settings": STANDARD.encode(wire)})).unwrap()
	}
	fn guild_entry(guild: Id, contents: &[u8], top: &mut Vec<u8>) {
		let mut entry = Vec::new();
		fixed64_field(1, guild.0, &mut entry);
		message(2, contents, &mut entry);
		message(1, &entry, top);
	}

	#[test]
	fn favorite_change_preserves_order_and_unknown_data() {
		let mut raw = Vec::new();
		fixed64_field(1, 41, &mut raw);
		message(1, &90u64.to_le_bytes(), &mut raw);
		message(2, &90u64.to_le_bytes(), &mut raw);
		fixed64_field(2, 41, &mut raw);
		message(22, b"future", &mut raw);
		let mut container = Vec::new();
		message(FAVORITES_FIELD, &raw, &mut container);
		let (version, mut favorites) = Favorites::decode(&response(&container)).unwrap();
		assert_eq!(version, 7);
		assert_eq!(favorites.favorite_ids(), [Id(90), Id(41)]);
		favorites.change(Id(41), false).unwrap();
		favorites.change(Id(80), true).unwrap();
		let encoded = STANDARD.decode(favorites.patch().unwrap()).unwrap();
		let (_, after) = Favorites::decode(&response(&encoded)).unwrap();
		assert_eq!(after.ids, [Id(90), Id(80)]);
		assert_eq!(after.ordered, [Id(90), Id(80)]);
		assert_eq!(after, favorites);
		fixed64_field(1, 41, &mut raw);
		assert!(Favorites::parse(&raw).is_err());
		assert!(Favorites::decode(br#"{"settings":"","out_of_date":true}"#).is_err());
		assert!(Favorites::decode(br#"{"settings":""}"#).is_err());
	}

	#[test]
	fn only_the_requested_guilds_entrance_sound_is_changed() {
		let (first, second) = (Id(100), Id(200));
		let mut join = Vec::new();
		fixed64_field(1, 42, &mut join);
		fixed64_field(2, second.0, &mut join);
		let mut first_guild = Vec::new();
		message(6, &join, &mut first_guild);
		message(14, b"retained", &mut first_guild);
		let mut nested = Vec::new();
		guild_entry(second, &first_guild, &mut nested);
		guild_entry(first, &first_guild, &mut nested);
		let mut entrance = Entrance::parse(&nested, EntranceTarget::Server(first)).unwrap();
		assert_eq!(entrance.sound, Some((Some(second), Id(42))));
		for after in [Some((None, Id(32))), None] {
			entrance.change(after).unwrap();
			let patched = STANDARD
				.decode(entrance.patch(EntranceTarget::Server(first)).unwrap())
				.unwrap();
			let (_, readback, _) =
				Entrance::decode(&response(&patched), EntranceTarget::Server(first)).unwrap();
			assert_eq!(readback.sound, after);
			assert_eq!(readback, entrance);
			assert_eq!(
				Entrance::decode(&response(&patched), EntranceTarget::Server(second))
					.unwrap()
					.1
					.sound,
				Some((Some(second), Id(42)))
			);
		}
		guild_entry(first, &[], &mut nested);
		assert!(Entrance::parse(&nested, EntranceTarget::Server(first)).is_err());
	}
	#[test]
	fn global_default_and_explicit_server_override_are_independent() {
		let server = EntranceTarget::Server(Id(100));
		let mut raw = Vec::new();
		for (guild, sound) in [(Id(0), Id(17)), (Id(100), Id(18))] {
			let mut value = Vec::new();
			let mut join = Vec::new();
			fixed64_field(1, sound.0, &mut join);
			message(6, &join, &mut value);
			message(20, b"retain", &mut value);
			guild_entry(guild, &value, &mut raw);
		}
		let mut container = Vec::new();
		message(GUILDS_FIELD, &raw, &mut container);
		let (_, mut local, fallback) = Entrance::decode(&response(&container), server).unwrap();
		assert_eq!(local.sound.or(fallback), Some((None, Id(18))));
		local.change(None).unwrap();
		let patched = STANDARD.decode(local.patch(server).unwrap()).unwrap();
		let (_, local, fallback) = Entrance::decode(&response(&patched), server).unwrap();
		assert_eq!(local.sound, None);
		assert_eq!(local.sound.or(fallback), Some((None, Id(17))));
		assert!(!local.other_guild_fields.is_empty());
		let (_, mut global, _) =
			Entrance::decode(&response(&patched), EntranceTarget::Global).unwrap();
		assert_eq!(global.sound, Some((None, Id(17))));
		global.change(Some((Some(Id(200)), Id(19)))).unwrap();
		let updated = STANDARD
			.decode(global.patch(EntranceTarget::Global).unwrap())
			.unwrap();
		let (_, local, fallback) = Entrance::decode(&response(&updated), server).unwrap();
		assert_eq!(local.sound, None);
		assert_eq!(fallback, Some((Some(Id(200)), Id(19))));
		assert!(!local.other_guild_fields.is_empty());
		assert!(Entrance::decode(&response(&updated), EntranceTarget::Server(Id(0))).is_err());
	}
}
