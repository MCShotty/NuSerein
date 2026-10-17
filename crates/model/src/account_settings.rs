//! Private own-account metadata. Session RAM only; never exported to extensions.
use crate::User;

pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_CONNECTION_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct Overview {
	pub user: User,
	pub username: String,
	pub email: Option<String>,
	pub phone: Option<String>,
	pub verified: Option<bool>,
	pub mfa_enabled: Option<bool>,
	pub premium_type: Option<u8>,
}
impl Overview {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.user.heap_bytes()
			+ self.username.capacity()
			+ self.email.as_ref().map_or(0, String::capacity)
			+ self.phone.as_ref().map_or(0, String::capacity)
	}
	pub fn valid(&self) -> bool {
		self.user.id.0 != 0
			&& self.bytes() <= 4096
			&& text(&self.username, 128)
			&& !self.username.is_empty()
			&& self.email.as_deref().is_none_or(|value| text(value, 320))
			&& self.phone.as_deref().is_none_or(|value| text(value, 64))
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionKey {
	pub kind: String,
	pub id: String,
}
impl ConnectionKey {
	pub fn valid(&self) -> bool {
		!self.kind.is_empty()
			&& self.kind.len() <= 32
			&& self.kind.bytes().all(|byte| {
				byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
			}) && !self.id.is_empty()
			&& !matches!(self.id.as_str(), "." | "..")
			&& text(&self.id, 128)
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.kind.capacity() + self.id.capacity()
	}
}
#[derive(Clone)]
pub struct Connection {
	pub key: ConnectionKey,
	pub name: String,
	pub verified: bool,
	pub revoked: bool,
	pub visibility: Option<u32>,
	pub metadata_visibility: Option<u32>,
	pub show_activity: Option<bool>,
	pub friend_sync: Option<bool>,
}
impl Connection {
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.key.bytes() + self.name.capacity()
	}
	pub fn valid(&self) -> bool {
		self.key.valid() && text(&self.name, 256) && self.bytes() <= 4096
	}
}
#[derive(Clone, Default)]
pub struct Connections {
	pub items: Vec<Connection>,
}
impl Connections {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.items.capacity() * size_of::<Connection>()
			+ self.items.iter().map(Connection::bytes).sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.items.capacity() <= MAX_CONNECTIONS
			&& self.bytes() <= MAX_CONNECTION_BYTES
			&& self.items.iter().enumerate().all(|(index, item)| {
				item.valid()
					&& !self.items[..index]
						.iter()
						.any(|before| before.key == item.key)
			})
	}
	pub fn find(&self, key: &ConnectionKey) -> Option<&Connection> {
		self.items.iter().find(|item| item.key == *key)
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionSetting {
	DisplayOnProfile(bool),
	DisplayMetadata(bool),
	DisplayActivity(bool),
	SyncFriends(bool),
	Remove,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionChange {
	pub key: ConnectionKey,
	pub setting: ConnectionSetting,
}
impl ConnectionChange {
	pub fn valid(&self) -> bool {
		self.key.valid()
	}
	pub fn supported(&self, connection: &Connection) -> bool {
		connection.key == self.key
			&& (!connection.revoked || self.setting == ConnectionSetting::Remove)
			&& match self.setting {
				ConnectionSetting::DisplayOnProfile(_) => {
					matches!(connection.visibility, Some(0 | 1))
				}
				ConnectionSetting::DisplayMetadata(_) => {
					matches!(connection.metadata_visibility, Some(0 | 1))
				}
				ConnectionSetting::DisplayActivity(_) => connection.show_activity.is_some(),
				ConnectionSetting::SyncFriends(_) => connection.friend_sync.is_some(),
				ConnectionSetting::Remove => true,
			}
	}
	pub fn matches(&self, saved: &Connections) -> bool {
		let Some(connection) = saved.find(&self.key) else {
			return self.setting == ConnectionSetting::Remove;
		};
		match self.setting {
			ConnectionSetting::DisplayOnProfile(value) => {
				connection.visibility == Some(u32::from(value))
			}
			ConnectionSetting::DisplayMetadata(value) => {
				connection.metadata_visibility == Some(u32::from(value))
			}
			ConnectionSetting::DisplayActivity(value) => connection.show_activity == Some(value),
			ConnectionSetting::SyncFriends(value) => connection.friend_sync == Some(value),
			ConnectionSetting::Remove => false,
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.key.bytes()
	}
}

fn text(value: &str, chars: usize) -> bool {
	value.len() <= chars * 4
		&& value.chars().count() <= chars
		&& !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn connection_keys_allow_external_ids_without_ever_becoming_paths() {
		let mut key = ConnectionKey {
			kind: "reddit".into(),
			id: "run&hide/../name?x=1".into(),
		};
		assert!(key.valid());
		key.kind = "../reddit".into();
		assert!(!key.valid());
		key.kind = "reddit".into();
		key.id = "x".repeat(513);
		assert!(!key.valid());
	}
}
