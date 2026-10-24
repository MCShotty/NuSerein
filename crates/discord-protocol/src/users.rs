//! User wire projection and validated primary-server identity.
use super::{Deserialize, Id, User, collectibles, lossy};

#[derive(Clone, Deserialize)]
pub struct UserDto {
	#[serde(default)]
	pub premium_type: model::Patch<u8>,
	pub id: Id,
	pub username: String,
	#[serde(default)]
	pub global_name: Option<String>,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub bot: bool,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub public_flags: u64,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub flags: u64,
	#[serde(default)]
	pub avatar: Option<String>,
	#[serde(default, deserialize_with = "lossy::null_default")]
	pub discriminator: String,
	#[serde(default)]
	pub primary_guild: Option<PrimaryGuildDto>,
	/// Older normal-session payloads duplicated `primary_guild` under this name.
	#[serde(default)]
	pub clan: Option<PrimaryGuildDto>,
	#[serde(default)]
	pub avatar_decoration_data: collectibles::DecorationField,
	#[serde(default)]
	pub collectibles: collectibles::CollectiblesField,
	#[serde(default)]
	pub display_name_styles: collectibles::NameStyleField,
}

#[derive(Clone, Deserialize)]
pub struct PrimaryGuildDto {
	pub identity_guild_id: Option<Id>,
	pub identity_enabled: Option<bool>,
	pub tag: Option<String>,
	pub badge: Option<String>,
}

impl PrimaryGuildDto {
	fn into_model(self) -> Option<model::ClanTag> {
		let tag = self.tag?;
		if self.identity_enabled == Some(false)
			|| tag.trim().is_empty()
			|| tag.chars().count() > 4
			|| tag.len() > 16
		{
			return None;
		}
		Some(model::ClanTag {
			guild: self.identity_guild_id?,
			tag,
			badge: self.badge.filter(|hash| model::valid_avatar_hash(hash)),
		})
	}
}

impl UserDto {
	pub fn into_model(self) -> User {
		let primary_guild = self
			.primary_guild
			.and_then(PrimaryGuildDto::into_model)
			.or_else(|| self.clan.and_then(PrimaryGuildDto::into_model))
			.map(Box::new);
		let cosmetics = collectibles::cosmetics(
			self.avatar_decoration_data,
			self.collectibles,
			self.display_name_styles,
		);
		let username: String = self
			.username
			.chars()
			.filter(|c| !c.is_control())
			.take(128)
			.collect();
		User {
			kind: if self.bot && (self.public_flags | self.flags) & (1 << 16) != 0 {
				model::AccountKind::VerifiedBot
			} else if self.bot {
				model::AccountKind::Bot
			} else {
				model::AccountKind::Human
			},
			id: self.id,
			name: self
				.global_name
				.unwrap_or_else(|| username.clone())
				.chars()
				.take(128)
				.collect(),
			username: (!username.is_empty()).then(|| username.into_boxed_str()),
			avatar: self.avatar.filter(|hash| model::valid_avatar_hash(hash)),
			webhook: false,
			discriminator: self
				.discriminator
				.parse::<u16>()
				.ok()
				.filter(|n| *n <= 9999)
				.unwrap_or(0),
			primary_guild,
			cosmetics,
		}
	}
}
