//! Own-account routes: bounded projections drop tokens, integrations and service metadata.
use crate::{DecodeError, UserDto};
use model::{
	Id, Patch,
	account_settings::{
		Connection, ConnectionChange, ConnectionKey, ConnectionSetting, Connections,
		MAX_CONNECTIONS, Overview,
	},
};
use serde::{
	Deserialize, Deserializer,
	de::{SeqAccess, Visitor},
};

pub const MAX_OVERVIEW_WIRE: usize = 64 * 1024;
pub const MAX_CONNECTIONS_WIRE: usize = 256 * 1024;

pub fn overview(bytes: &[u8], user: Id) -> Result<Overview, DecodeError> {
	#[derive(Deserialize)]
	struct Dto {
		#[serde(default)]
		email: Option<String>,
		#[serde(default)]
		phone: Option<String>,
		#[serde(default)]
		verified: Option<bool>,
		#[serde(default)]
		mfa_enabled: Option<bool>,
	}
	if bytes.len() > MAX_OVERVIEW_WIRE {
		return Err(DecodeError);
	}
	let dto: Dto = crate::decode(bytes)?;
	// Avoid flatten's temporary catch-all map: ignored token fields must not
	// become ordinary String allocations outside the zeroized HTTP body.
	let identity: UserDto = crate::decode(bytes)?;
	if identity.id != user || identity.bot {
		return Err(DecodeError);
	}
	let premium_type = match identity.premium_type {
		Patch::Value(value) => Some(value),
		_ => None,
	};
	let overview = Overview {
		username: identity.username.clone(),
		user: identity.into_model(),
		email: dto.email,
		phone: dto.phone,
		verified: dto.verified,
		mfa_enabled: dto.mfa_enabled,
		premium_type,
	};
	if !overview.valid() {
		return Err(DecodeError);
	}
	Ok(overview)
}

#[derive(Deserialize)]
struct ConnectionDto {
	#[serde(rename = "type")]
	kind: String,
	id: String,
	name: String,
	#[serde(default)]
	verified: bool,
	#[serde(default)]
	revoked: bool,
	#[serde(default)]
	visibility: Option<u32>,
	#[serde(default)]
	metadata_visibility: Option<u32>,
	#[serde(default)]
	show_activity: Option<bool>,
	#[serde(default)]
	friend_sync: Option<bool>,
}
impl ConnectionDto {
	fn into_model(self) -> Result<Connection, DecodeError> {
		let item = Connection {
			key: ConnectionKey {
				kind: self.kind,
				id: self.id,
			},
			name: self.name,
			verified: self.verified,
			revoked: self.revoked,
			visibility: self.visibility,
			metadata_visibility: self.metadata_visibility,
			show_activity: self.show_activity,
			friend_sync: self.friend_sync,
		};
		if !item.valid() {
			return Err(DecodeError);
		}
		Ok(item)
	}
}
struct List(Connections);
impl<'de> Deserialize<'de> for List {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct Items;
		impl<'de> Visitor<'de> for Items {
			type Value = List;
			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("bounded account connections")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<List, A::Error> {
				let mut connections = Connections::default();
				while let Some(dto) = seq.next_element::<ConnectionDto>()? {
					if connections.items.len() == MAX_CONNECTIONS {
						return Err(serde::de::Error::custom("connection limit"));
					}
					connections.items.push(
						dto.into_model()
							.map_err(|_| serde::de::Error::custom("invalid connection"))?,
					);
					if connections.bytes() > model::account_settings::MAX_CONNECTION_BYTES {
						return Err(serde::de::Error::custom("connection byte limit"));
					}
				}
				connections.items.shrink_to_fit();
				if !connections.valid() {
					return Err(serde::de::Error::custom("invalid connection list"));
				}
				Ok(List(connections))
			}
		}
		deserializer.deserialize_seq(Items)
	}
}
pub fn connections(bytes: &[u8]) -> Result<Connections, DecodeError> {
	if bytes.len() > MAX_CONNECTIONS_WIRE {
		return Err(DecodeError);
	}
	let List(connections) = crate::decode(bytes)?;
	Ok(connections)
}
pub fn confirm_connection(bytes: &[u8], key: &ConnectionKey) -> Result<(), DecodeError> {
	if bytes.len() > MAX_CONNECTIONS_WIRE {
		return Err(DecodeError);
	}
	let dto: ConnectionDto = crate::decode(bytes)?;
	if dto.into_model()?.key != *key {
		return Err(DecodeError);
	}
	Ok(())
}
pub fn connection_path(key: &ConnectionKey) -> Result<String, DecodeError> {
	if !key.valid() {
		return Err(DecodeError);
	}
	let mut path = format!("/users/@me/connections/{}/", key.kind);
	for byte in key.id.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~') {
			path.push(char::from(byte));
		} else {
			path.push('%');
			path.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
			path.push(char::from(b"0123456789ABCDEF"[(byte & 15) as usize]));
		}
	}
	Ok(path)
}
pub fn connection_body(
	change: &ConnectionChange,
) -> Result<Option<serde_json::Value>, DecodeError> {
	use serde_json::json;
	if !change.valid() {
		return Err(DecodeError);
	}
	Ok(match change.setting {
		ConnectionSetting::DisplayOnProfile(value) => Some(json!({"visibility":u32::from(value)})),
		ConnectionSetting::DisplayMetadata(value) => {
			Some(json!({"metadata_visibility":u32::from(value)}))
		}
		ConnectionSetting::DisplayActivity(value) => Some(json!({"show_activity":value})),
		ConnectionSetting::SyncFriends(value) => Some(json!({"friend_sync":value})),
		ConnectionSetting::Remove => None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	#[test]
	fn private_overview_is_account_bound_and_discards_unknown_secret_fields() {
		let value = json!({"id":"1","username":"synthetic","email":"synthetic@example.invalid","phone":"+10000000000","verified":true,"mfa_enabled":true,"premium_type":2,"token":"SYNTHETIC_SECRET_NOT_RETAINED"});
		let bytes = serde_json::to_vec(&value).unwrap();
		let current = overview(&bytes, Id(1)).unwrap();
		assert_eq!(current.email.as_deref(), Some("synthetic@example.invalid"));
		assert!(overview(&bytes, Id(2)).is_err());
		let mut bad = value;
		bad["email"] = json!("x".repeat(1281));
		assert!(overview(&serde_json::to_vec(&bad).unwrap(), Id(1)).is_err());
	}
	#[test]
	fn connections_bound_collections_and_encode_each_external_id_as_one_path_segment() {
		let item = json!({"type":"reddit","id":"run&hide/../name?x=1","name":"Synthetic","verified":true,"show_activity":false,"visibility":1,"access_token":"SYNTHETIC_SECRET_NOT_RETAINED","integrations":[{"token":"ignored"}]});
		let current = connections(&serde_json::to_vec(&json!([item])).unwrap()).unwrap();
		assert_eq!(current.items.len(), 1);
		let key = current.items[0].key.clone();
		assert_eq!(
			connection_path(&key).unwrap(),
			"/users/@me/connections/reddit/run%26hide%2F..%2Fname%3Fx%3D1"
		);
		let change = ConnectionChange {
			key,
			setting: ConnectionSetting::DisplayOnProfile(false),
		};
		assert_eq!(
			connection_body(&change).unwrap(),
			Some(json!({"visibility":0}))
		);
		assert!(connections(&serde_json::to_vec(&vec![item.clone(); 65]).unwrap()).is_err());
		assert!(
			connections(&serde_json::to_vec(&vec![item.clone(); 2]).unwrap()).is_err(),
			"duplicate keys are not an editable list"
		);
		assert!(connections(br#"null"#).is_err());
		assert!(
			connection_path(&ConnectionKey {
				kind: "reddit".into(),
				id: "..".into()
			})
			.is_err()
		);
	}
}
