//! Service user identity and saved-account projections.
use super::{ClanTag, Cosmetics, Deserialize, Id, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
	#[serde(default)]
	pub kind: AccountKind,
	/// Set only for message authors with a service-provided webhook_id.
	#[serde(default)]
	pub webhook: bool,
	pub id: Id,
	pub name: String,
	/// Actual service username, separate from global display names and guild nicknames.
	/// Session display metadata only; older disk history has no authoritative username.
	#[serde(default, skip)]
	pub username: Option<Box<str>>,
	pub avatar: Option<String>,
	pub discriminator: u16,
	/// Service-supplied server identity displayed beside this user's name.
	#[serde(default, skip_serializing)]
	pub primary_guild: Option<Box<ClanTag>>,
	/// Service-supplied avatar decoration and nameplate; display-only, never persisted.
	#[serde(default, skip)]
	pub cosmetics: Option<Box<Cosmetics>>,
}

impl User {
	pub fn account_label(&self) -> Option<&'static str> {
		match (self.kind, self.webhook) {
			(AccountKind::App, _) => Some("APP"),
			(_, true) => Some("WEBHOOK"),
			(AccountKind::VerifiedBot, _) => Some("APP"),
			(AccountKind::Bot, _) => Some("BOT"),
			_ => None,
		}
	}
	pub fn heap_bytes(&self) -> usize {
		self.name.capacity()
			+ self.username.as_deref().map_or(0, str::len)
			+ self.avatar.as_ref().map_or(0, String::capacity)
			+ self.primary_guild.as_ref().map_or(0, |guild| {
				std::mem::size_of::<ClanTag>()
					+ guild.tag.capacity()
					+ guild.badge.as_ref().map_or(0, String::capacity)
			}) + self.cosmetics.as_ref().map_or(0, |cosmetics| {
			std::mem::size_of::<Cosmetics>() + cosmetics.heap_bytes()
		})
	}
	pub fn avatar_key(&self) -> String {
		if let Some(hash) = self
			.avatar
			.as_deref()
			.filter(|hash| valid_avatar_hash(hash))
		{
			format!("{}-{hash}", self.id)
		} else {
			let index = if self.discriminator == 0 {
				(self.id.0 >> 22) % 6
			} else {
				u64::from(self.discriminator % 5)
			};
			format!("default-{index}")
		}
	}
	pub fn avatar_url(&self) -> String {
		let key = self.avatar_key();
		if let Some(index) = key.strip_prefix("default-") {
			format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
		} else {
			let (_, hash) = key.split_once('-').expect("avatar key");
			let ext = if hash.starts_with("a_") { "gif" } else { "png" };
			format!(
				"https://cdn.discordapp.com/avatars/{}/{hash}.{ext}?size=128",
				self.id
			)
		}
	}
}

/// Explicit service metadata, never inferred from a name or a failed profile request.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AccountKind {
	#[default]
	Human = 0,
	Bot = 1,
	App = 2,
	VerifiedBot = 3,
}

/// Locally remembered account for the switcher: identity only, never a token.
/// Tokens stay in the OS credential store under their own per-account entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedAccount {
	pub id: Id,
	pub name: String,
	/// Global display name, when the account has one.
	pub display: Option<String>,
	pub avatar: Option<String>,
	pub discriminator: u16,
	/// Whether the OS credential store holds this account's own entry. Read-only output of
	/// the roster: writes go through `set_account_token`, so an identity refresh cannot
	/// claim a token exists. Keeps the client from rewriting an entry it already wrote,
	/// which on macOS is an access-controlled operation that can prompt for the keychain.
	pub has_token: bool,
}

impl SavedAccount {
	pub const MAX_LABEL_BYTES: usize = 64;
	/// Preserve a readable identity within the existing switcher/storage budget.
	/// Account IDs and credential ownership are independent of this display projection.
	pub fn label_projection(value: &str) -> &str {
		let mut end = value.len().min(Self::MAX_LABEL_BYTES);
		while !value.is_char_boundary(end) {
			end -= 1;
		}
		&value[..end]
	}
	pub fn is_valid(&self) -> bool {
		self.id.0 != 0
			&& (1..=Self::MAX_LABEL_BYTES).contains(&self.name.len())
			&& self
				.display
				.as_ref()
				.is_none_or(|display| (1..=Self::MAX_LABEL_BYTES).contains(&display.len()))
			&& self.avatar.as_deref().is_none_or(valid_avatar_hash)
			&& self.discriminator <= 9999
	}
	/// What the switcher shows: display name when set, otherwise the username.
	pub fn label(&self) -> &str {
		self.display
			.as_deref()
			.filter(|display| !display.is_empty())
			.unwrap_or(&self.name)
	}
	/// Avatar lookups and name rows reuse the ordinary user widgets.
	pub fn user(&self) -> User {
		User {
			username: None,
			kind: AccountKind::Human,
			webhook: false,
			id: self.id,
			name: self.name.clone(),
			avatar: self.avatar.clone(),
			discriminator: self.discriminator,
			primary_guild: None,
			cosmetics: None,
		}
	}
	pub fn heap_bytes(&self) -> usize {
		self.name.capacity()
			+ self.display.as_ref().map_or(0, String::capacity)
			+ self.avatar.as_ref().map_or(0, String::capacity)
	}
}

pub fn valid_avatar_hash(hash: &str) -> bool {
	let hash = hash.strip_prefix("a_").unwrap_or(hash);
	hash.len() == 32 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
