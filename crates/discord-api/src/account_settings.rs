use crate::{DiscordApi, Failure};
use discord_protocol::account_settings as wire;
use model::{
	Id,
	account_settings::{ConnectionChange, ConnectionSetting, Connections, Overview},
};
use reqwest::Method;

impl DiscordApi {
	pub(super) async fn account_overview(&self, user: Id) -> Result<Box<Overview>, Failure> {
		if user.0 == 0 {
			return Err(Failure::Protocol);
		}
		// A current-user response can include token-bearing fields. They are ignored by
		// the typed decoder and the owned response allocation is wiped after decoding.
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(Method::GET, "/users/@me", None, wire::MAX_OVERVIEW_WIRE)
				.await?,
		);
		wire::overview(&bytes, user)
			.map(Box::new)
			.map_err(|_| Failure::ProtocolAt("Account details are unavailable or unsupported"))
	}
	async fn read_account_connections(&self) -> Result<Box<Connections>, Failure> {
		// Own connections may contain third-party access tokens; never retain them.
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(
				Method::GET,
				"/users/@me/connections",
				None,
				wire::MAX_CONNECTIONS_WIRE,
			)
			.await?,
		);
		wire::connections(&bytes)
			.map(Box::new)
			.map_err(|_| Failure::ProtocolAt("Account connections are incomplete or unsupported"))
	}
	pub(super) async fn account_connections(
		&self,
		change: Option<ConnectionChange>,
	) -> Result<Box<Connections>, Failure> {
		if change.as_ref().is_some_and(|change| !change.valid()) {
			return Err(Failure::Protocol);
		}
		let current = self.read_account_connections().await?;
		let Some(change) = change else {
			return Ok(current);
		};
		if current
			.find(&change.key)
			.is_none_or(|connection| !change.supported(connection))
		{
			return Err(Failure::ProtocolAt(
				"That connection changed or is unavailable; reload to continue",
			));
		}
		if change.matches(&current) {
			return Ok(current);
		}
		let path = wire::connection_path(&change.key).map_err(|_| Failure::Protocol)?;
		let body = wire::connection_body(&change).map_err(|_| Failure::Protocol)?;
		let deleting = change.setting == ConnectionSetting::Remove;
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(
				if deleting {
					Method::DELETE
				} else {
					Method::PATCH
				},
				&path,
				body,
				wire::MAX_CONNECTIONS_WIRE,
			)
			.await?,
		);
		if !deleting {
			wire::confirm_connection(&bytes, &change.key).map_err(|_| Failure::Ambiguous)?;
		}
		let saved = self.read_account_connections().await?;
		if !change.matches(&saved) {
			return Err(Failure::ProtocolAt(
				"Connection change was not confirmed; reload before applying again",
			));
		}
		Ok(saved)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::server_profile::tests::reply;
	use client_core::{Command, Event, auth::SessionSecret};
	use model::account_settings::ConnectionKey;
	use serde_json::json;
	use std::{sync::Arc, time::Duration};
	use tokio::net::TcpListener;
	#[tokio::test]
	async fn own_account_and_connection_changes_confirm_identity_and_never_retry_writes() {
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let make_api = || {
				let mut api = DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_ACCOUNT_SETTINGS_TOKEN".into()).unwrap())).unwrap();
				api.base = format!("http://{}", listener.local_addr().unwrap()); api
			};
			let api = make_api();
			let (event, ()) = tokio::join!(api.execute(Command::AccountOverview { user: Id(1), request: 4 }), reply(&listener, "GET /users/@me", None, 200, r#"{"id":"1","username":"synthetic","email":"preview@example.invalid","token":"SYNTHETIC_IGNORED"}"#));
			let Event::AccountOverview { user, request, result } = event else { panic!("wrong event"); };
			assert_eq!((user, request), (Id(1), 4));
			assert_eq!(result.unwrap().email.as_deref(), Some("preview@example.invalid"));
			let item = json!({"type":"reddit","id":"run&hide","name":"Synthetic","visibility":1,"show_activity":false});
			let current = json!([item]).to_string();
			let mut hidden = item.clone(); hidden["visibility"] = json!(0);
			let saved = json!([hidden]).to_string();
			let get = "GET /users/@me/connections";
			let patch = "PATCH /users/@me/connections/reddit/run%26hide";
			let change = ConnectionChange { key: ConnectionKey { kind: "reddit".into(), id: "run&hide".into() }, setting: ConnectionSetting::DisplayOnProfile(false) };
			let server = async {
				reply(&listener, get, None, 200, &current).await;
				reply(&listener, patch, Some(json!({"visibility":0})), 200, &hidden.to_string()).await;
				reply(&listener, get, None, 200, &saved).await;
			};
			let (result, ()) = tokio::join!(api.account_connections(Some(change.clone())), server);
			assert!(change.matches(&result.unwrap()));
			for (status, expected) in [(403, Failure::Forbidden), (429, Failure::RateLimited), (500, Failure::Ambiguous)] {
				let api = make_api();
				let server = async {
					reply(&listener, get, None, 200, &current).await;
					reply(&listener, patch, Some(json!({"visibility":0})), status, "{}").await;
				};
				let (result, ()) = tokio::join!(api.account_connections(Some(change.clone())), server);
				assert_eq!(result.err(), Some(expected));
			}
			let remove = ConnectionChange { setting: ConnectionSetting::Remove, ..change };
			let server = async {
				reply(&listener, get, None, 200, &current).await;
				reply(&listener, "DELETE /users/@me/connections/reddit/run%26hide", None, 204, "").await;
				reply(&listener, get, None, 200, "[]").await;
			};
			let (result, ()) = tokio::join!(api.account_connections(Some(remove)), server);
			assert!(result.unwrap().items.is_empty());
			assert!(tokio::time::timeout(Duration::from_millis(20), listener.accept()).await.is_err());
		}).await.unwrap();
	}
}
