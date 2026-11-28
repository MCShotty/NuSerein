use crate::{DiscordApi, Failure};
use client_core::soundboard::{
	MAX_WIRE_BYTES, SettingsAction, SettingsResult, SettingsScope, SettingsWork,
};
use discord_protocol::soundboard_settings::{Entrance, EntranceTarget, Favorites, MAX_RESPONSE};
use model::{Id, soundboard::Sound};
use reqwest::Method;

const UNSUPPORTED: Failure = Failure::ProtocolAt("voice-soundboard-settings-unavailable");
const UNCONFIRMED: Failure = Failure::ProtocolAt("voice-soundboard-settings-unconfirmed");
const CHANGED: Failure = Failure::ProtocolAt("voice-soundboard-entrance-changed");

impl DiscordApi {
	pub(super) async fn soundboard_settings(
		&self,
		work: SettingsWork,
	) -> Result<SettingsResult, Failure> {
		use SettingsAction as A;
		if work.request == 0 {
			return Err(Failure::Protocol);
		}
		let target = match (work.scope, work.action) {
			(SettingsScope::Account(id), A::EntranceCatalog(source)) if id.0 != 0 => {
				return self
					.soundboard_catalog(source)
					.await
					.map(SettingsResult::Catalog);
			}
			(SettingsScope::Account(id), A::LoadGlobalEntrance | A::GlobalEntrance { .. })
				if id.0 != 0 =>
			{
				EntranceTarget::Global
			}
			(
				SettingsScope::Call {
					guild,
					channel,
					call,
				},
				A::LoadFavorites | A::Favorite { .. } | A::LoadEntrance | A::Entrance { .. },
			) if guild.0 != 0 && channel.0 != 0 && call != 0 => EntranceTarget::Server(guild),
			_ => return Err(Failure::Protocol),
		};
		let route = match work.action {
			SettingsAction::LoadFavorites | SettingsAction::Favorite { .. } => {
				"/users/@me/settings-proto/2"
			}
			_ => "/users/@me/settings-proto/1",
		};
		let bytes = self
			.request_limited(Method::GET, route, None, MAX_RESPONSE)
			.await?;
		match work.action {
			SettingsAction::LoadFavorites | SettingsAction::Favorite { .. } => {
				let (version, mut current) = Favorites::decode(&bytes).map_err(|_| UNSUPPORTED)?;
				if let SettingsAction::Favorite { key, enabled } = work.action {
					let before = current.clone();
					current.change(key.1, enabled).map_err(|_| UNSUPPORTED)?;
					if current != before {
						let payload = current.patch().map_err(|_| UNSUPPORTED)?;
						let saved = self
							.write_soundboard_settings(route, version, payload)
							.await?;
						let (confirmed, returned) =
							Favorites::decode(&saved).map_err(|_| UNCONFIRMED)?;
						if confirmed <= version || current != returned {
							return Err(UNCONFIRMED);
						}
					}
				}
				Ok(SettingsResult::Favorites(current.favorite_ids()))
			}
			A::LoadEntrance
			| A::Entrance { .. }
			| A::LoadGlobalEntrance
			| A::GlobalEntrance { .. } => {
				let (version, mut current, fallback) =
					Entrance::decode(&bytes, target).map_err(|_| UNSUPPORTED)?;
				if let A::Entrance { before, after } | A::GlobalEntrance { before, after } =
					work.action
				{
					if current.sound == after {
						return Ok(SettingsResult::Entrance(after, fallback));
					}
					if current.sound != before {
						return Err(CHANGED);
					}
					current.change(after).map_err(|_| UNSUPPORTED)?;
					let payload = current.patch(target).map_err(|_| UNSUPPORTED)?;
					let saved = self
						.write_soundboard_settings(route, version, payload)
						.await?;
					let (confirmed, returned, fallback) =
						Entrance::decode(&saved, target).map_err(|_| UNCONFIRMED)?;
					if confirmed <= version || current != returned {
						return Err(UNCONFIRMED);
					}
					return Ok(SettingsResult::Entrance(returned.sound, fallback));
				}
				Ok(SettingsResult::Entrance(current.sound, fallback))
			}
			A::EntranceCatalog(_) => Err(Failure::Protocol),
		}
	}

	pub(super) async fn soundboard_catalog(
		&self,
		source: Option<Id>,
	) -> Result<Vec<Sound>, Failure> {
		let route = match source {
			None => "/soundboard-default-sounds".into(),
			Some(guild) if guild.0 != 0 => format!("/guilds/{guild}/soundboard-sounds"),
			Some(_) => return Err(Failure::Protocol),
		};
		let bytes = self
			.request_limited(Method::GET, &route, None, MAX_WIRE_BYTES)
			.await?;
		match source {
			None => discord_protocol::soundboard::default_sounds(&bytes),
			Some(guild) => discord_protocol::soundboard::guild_sounds(&bytes, guild),
		}
		.map_err(|_| Failure::Protocol)
	}

	async fn write_soundboard_settings(
		&self,
		route: &'static str,
		version: u32,
		payload: String,
	) -> Result<Vec<u8>, Failure> {
		let reply = self
			.request_limited(
				Method::PATCH,
				route,
				Some(serde_json::json!({"settings": payload, "required_data_version": version})),
				MAX_RESPONSE,
			)
			.await?;
		// A partial PATCH response is not proof that an absent field was cleared.
		#[derive(serde::Deserialize)]
		struct Ack {
			#[serde(default)]
			out_of_date: bool,
		}
		if serde_json::from_slice::<Ack>(&reply)
			.map_err(|_| UNCONFIRMED)?
			.out_of_date
		{
			return Err(UNCONFIRMED);
		}
		self.request_limited(Method::GET, route, None, MAX_RESPONSE)
			.await
			.map_err(|error| {
				if error.ends_session() {
					error
				} else {
					UNCONFIRMED
				}
			})
	}
}
