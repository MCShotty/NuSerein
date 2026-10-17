//! Owned inventory and equip fields from Userdoccers and the public client's profile
//! update builder (`ProfileUpdateRequestUtils.collectibles_sku_ids`). Unofficial.
use crate::{DecodeError, UserDto};
use model::{
	Id,
	profile_cosmetics::{
		Change, Current, Equipped, Inventory, Item, Kind, MAX_EQUIPPED, MAX_ITEMS,
	},
};
use serde::{
	Deserialize, Deserializer,
	de::{SeqAccess, Visitor},
};
use serde_json::{Value, json};

pub const MAX_INVENTORY_WIRE: usize = 2 * 1024 * 1024;
pub const MAX_CURRENT_WIRE: usize = crate::profile::MAX_PROFILE_WIRE;

struct Limited<T, const N: usize>(Vec<T>);
impl<T, const N: usize> Default for Limited<T, N> {
	fn default() -> Self {
		Self(Vec::new())
	}
}
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Limited<T, N> {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct List<T, const N: usize>(std::marker::PhantomData<T>);
		impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for List<T, N> {
			type Value = Limited<T, N>;
			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("bounded collectibles list")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
				let mut values = Vec::new();
				while let Some(value) = seq.next_element()? {
					if values.len() == N {
						return Err(serde::de::Error::custom("collectibles limit"));
					}
					values.push(value);
				}
				values.shrink_to_fit();
				Ok(Limited(values))
			}
		}
		deserializer.deserialize_seq(List::<T, N>(std::marker::PhantomData))
	}
}

pub fn inventory(bytes: &[u8]) -> Result<Inventory, DecodeError> {
	#[derive(Deserialize)]
	struct Product {
		name: String,
		#[serde(default)]
		expires_at: Option<String>,
		#[serde(default)]
		purchase_type: Option<u32>,
		#[serde(default)]
		items: Limited<ProductItem, 8>,
	}
	#[derive(Deserialize)]
	struct ProductItem {
		#[serde(rename = "type")]
		kind: u32,
		sku_id: Id,
	}
	if bytes.len() > MAX_INVENTORY_WIRE {
		return Err(DecodeError);
	}
	let Limited(products) = crate::decode::<Limited<Product, MAX_ITEMS>>(bytes)?;
	let mut inventory = Inventory::default();
	for product in products {
		let expires_at = product
			.expires_at
			.map(|value| {
				crate::Timestamp::try_from(value)
					.map(|stamp| u64::try_from(stamp.0 / 1_000_000_000).unwrap_or_default())
					.map_err(|_| DecodeError)
			})
			.transpose()?;
		let name: String = product
			.name
			.chars()
			.filter(|ch| !ch.is_control())
			.take(128)
			.collect();
		if product.items.0.is_empty() {
			inventory.unsupported = true;
		}
		for item in product.items.0 {
			let Some(kind) = Kind::from_wire(item.kind) else {
				inventory.unsupported = true;
				continue;
			};
			if inventory.items.len() == MAX_ITEMS {
				return Err(DecodeError);
			}
			inventory.items.push(Item {
				sku: item.sku_id,
				kind,
				name: if name.is_empty() {
					kind.label().into()
				} else {
					name.clone()
				},
				expires_at,
				subscription: product.purchase_type == Some(7),
			});
			if inventory.bytes() > model::profile_cosmetics::MAX_BYTES {
				return Err(DecodeError);
			}
		}
	}
	inventory.items.shrink_to_fit();
	if !inventory.valid() {
		return Err(DecodeError);
	}
	Ok(inventory)
}

#[derive(Deserialize)]
struct Reference {
	sku_id: Id,
}
#[derive(Deserialize, Default)]
struct MemberCollectibles {
	#[serde(default)]
	nameplate: Option<Reference>,
}
#[derive(Deserialize)]
struct User {
	id: Id,
	#[serde(default)]
	bot: bool,
	#[serde(default)]
	avatar_decoration_data: Option<Reference>,
	#[serde(default)]
	collectibles: Option<MemberCollectibles>,
}
#[derive(Deserialize, Default)]
struct Member {
	#[serde(default)]
	user: Option<UserDto>,
	#[serde(default)]
	avatar_decoration_data: Option<Reference>,
	#[serde(default)]
	collectibles: Option<MemberCollectibles>,
}
#[derive(Deserialize)]
struct ProfileItem {
	#[serde(rename = "type")]
	kind: u32,
	sku_id: Id,
}
#[derive(Deserialize)]
struct LegacyEffect {
	id: Id,
}
#[derive(Deserialize, Default)]
struct Metadata {
	#[serde(default)]
	guild_id: Option<Id>,
	#[serde(default)]
	collectibles: Option<Limited<ProfileItem, MAX_EQUIPPED>>,
	#[serde(default)]
	profile_effect: Option<LegacyEffect>,
}
pub fn current(bytes: &[u8], user: Id, guild: Option<Id>) -> Result<Current, DecodeError> {
	#[derive(Deserialize)]
	struct Profile {
		user: User,
		user_profile: Metadata,
		#[serde(default)]
		guild_member: Option<Member>,
		#[serde(default)]
		guild_member_profile: Option<Metadata>,
		#[serde(default)]
		private: bool,
		#[serde(default)]
		premium_type: Option<u32>,
	}
	if bytes.len() > MAX_CURRENT_WIRE {
		return Err(DecodeError);
	}
	let dto: Profile = crate::decode(bytes)?;
	if dto.user.id != user || dto.user.bot || dto.private {
		return Err(DecodeError);
	}
	let (decoration, nameplate, metadata) = if let Some(guild) = guild {
		let member = dto.guild_member.ok_or(DecodeError)?;
		if member
			.user
			.is_some_and(|member| member.id != user || member.bot)
		{
			return Err(DecodeError);
		}
		let metadata = dto.guild_member_profile.unwrap_or_default();
		if metadata.guild_id.is_some_and(|id| id != guild) {
			return Err(DecodeError);
		}
		(member.avatar_decoration_data, member.collectibles, metadata)
	} else {
		if dto.user_profile.guild_id.is_some() {
			return Err(DecodeError);
		}
		(
			dto.user.avatar_decoration_data,
			dto.user.collectibles,
			dto.user_profile,
		)
	};
	let mut profile: Vec<_> = metadata
		.collectibles
		.unwrap_or_default()
		.0
		.into_iter()
		.map(|item| Equipped {
			kind: item.kind,
			sku: item.sku_id,
		})
		.collect();
	if let Some(effect) = metadata.profile_effect {
		if let Some(current) = profile.iter().find(|item| item.kind == 1) {
			if current.sku != effect.id {
				return Err(DecodeError);
			}
		} else {
			profile.push(Equipped {
				kind: 1,
				sku: effect.id,
			});
		}
	}
	profile.shrink_to_fit();
	let current = Current {
		user,
		guild,
		premium_type: dto.premium_type,
		decoration: decoration.map(|value| value.sku_id),
		nameplate: nameplate
			.and_then(|value| value.nameplate)
			.map(|value| value.sku_id),
		profile,
	};
	if !current.valid() {
		return Err(DecodeError);
	}
	Ok(current)
}

pub fn encode(current: &Current, change: Change) -> Result<(String, Value, Current), DecodeError> {
	if !current.valid() || change.sku.is_some_and(|sku| sku.0 == 0) {
		return Err(DecodeError);
	}
	let member = match current.guild {
		Some(guild) => format!("/guilds/{guild}/members/@me"),
		None => "/users/@me".into(),
	};
	let profile = match current.guild {
		Some(guild) => format!("/guilds/{guild}/profile/@me"),
		None => "/users/@me/profile".into(),
	};
	let mut expected = current.clone();
	change.apply(&mut expected);
	if !expected.valid() {
		return Err(DecodeError);
	}
	let (path, body) = match change.kind {
		Kind::Decoration => (member, json!({"avatar_decoration_sku_id":change.sku})),
		Kind::Nameplate if current.guild.is_none() => {
			(member, json!({"nameplate_sku_id":change.sku}))
		}
		Kind::Nameplate => (
			member,
			json!({"collectibles":{"nameplate":change.sku.map(|sku|json!({"sku_id":sku}))}}),
		),
		Kind::Effect | Kind::Frame => (
			profile,
			json!({"collectibles_sku_ids":expected.profile.iter().map(|item|item.sku).collect::<Vec<_>>() }),
		),
	};
	Ok((path, body, expected))
}
pub fn confirm(bytes: &[u8], user: Id, guild: Option<Id>, kind: Kind) -> Result<(), DecodeError> {
	if bytes.len() > MAX_CURRENT_WIRE {
		return Err(DecodeError);
	}
	if matches!(kind, Kind::Effect | Kind::Frame) {
		let metadata: Metadata = crate::decode(bytes)?;
		if metadata.guild_id != guild {
			return Err(DecodeError);
		}
	} else if guild.is_some() {
		crate::server_profile::confirm_member(bytes, user)?;
	} else {
		let saved: UserDto = crate::decode(bytes)?;
		if saved.id != user || saved.bot {
			return Err(DecodeError);
		}
	}
	Ok(())
}
pub fn matches(expected: &Current, saved: &Current) -> bool {
	let mut before = expected.profile.clone();
	let mut after = saved.profile.clone();
	before.sort_unstable_by_key(|item| (item.kind, item.sku));
	after.sort_unstable_by_key(|item| (item.kind, item.sku));
	expected.user == saved.user
		&& expected.guild == saved.guild
		&& expected.decoration == saved.decoration
		&& expected.nameplate == saved.nameplate
		&& before == after
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn purchased_inventory_keeps_expiry_and_subscription_authority_and_bounds_items() {
		let value = json!([{ "name":"Owned decoration","purchase_type":1,"items":[{"type":0,"sku_id":"7"}] },{"name":"Expired frame","purchase_type":7,"expires_at":"2020-01-01T00:00:00Z","items":[{"type":3,"sku_id":"8"}]}]);
		let inventory = inventory(&serde_json::to_vec(&value).unwrap()).unwrap();
		assert!(
			inventory
				.find(Kind::Decoration, Id(7))
				.unwrap()
				.available(false, 1_800_000_000)
		);
		assert!(
			!inventory
				.find(Kind::Frame, Id(8))
				.unwrap()
				.available(true, 1_800_000_000)
		);
		assert!(
			super::inventory(&serde_json::to_vec(&vec![value[0].clone(); MAX_ITEMS + 1]).unwrap())
				.is_err()
		);
		assert!(
			super::inventory(br#"[{"name":"bad expiry","expires_at":"tomorrow","items":[]}]"#)
				.is_err()
		);
	}
	#[test]
	fn profile_equips_preserve_unknown_ids_and_reject_conflicting_scopes() {
		let value = json!({"user":{"id":"1"},"user_profile":{"collectibles":[{"type":91,"sku_id":"10"},{"type":1,"sku_id":"11"},{"type":3,"sku_id":"12"}]}});
		let current = current(&serde_json::to_vec(&value).unwrap(), Id(1), None).unwrap();
		let (path, body, expected) = encode(
			&current,
			Change {
				kind: Kind::Effect,
				sku: Some(Id(13)),
			},
		)
		.unwrap();
		assert_eq!(path, "/users/@me/profile");
		assert_eq!(body, json!({"collectibles_sku_ids":["10","12","13"]}));
		assert!(expected.profile.contains(&Equipped {
			kind: 91,
			sku: Id(10)
		}));
		assert!(super::current(&serde_json::to_vec(&value).unwrap(), Id(2), None).is_err());
		let mut invalid = value.clone();
		invalid["user_profile"]["profile_effect"] = json!({"id":"99"});
		assert!(super::current(&serde_json::to_vec(&invalid).unwrap(), Id(1), None).is_err());
		let server = json!({"user":{"id":"1"},"user_profile":{},"guild_member":{},"guild_member_profile":{"guild_id":"2"}});
		let current =
			super::current(&serde_json::to_vec(&server).unwrap(), Id(1), Some(Id(2))).unwrap();
		assert_eq!(
			encode(
				&current,
				Change {
					kind: Kind::Nameplate,
					sku: None
				}
			)
			.unwrap()
			.1,
			json!({"collectibles":{"nameplate":null}})
		);
		assert!(super::current(&serde_json::to_vec(&server).unwrap(), Id(1), Some(Id(3))).is_err());
	}
}
