use crate::{DiscordApi, Failure};
use discord_protocol::{profile::MAX_PROFILE_WIRE, server_profile as wire};
use model::{
	Id,
	server_profile::{Edit, Snapshot},
};
use reqwest::Method;

impl DiscordApi {
	pub(super) async fn own_server_profile(
		&self,
		user: Id,
		guild: Id,
		edit: Option<Box<Edit>>,
	) -> Result<Box<Snapshot>, Failure> {
		if user.0 == 0
			|| guild.0 == 0
			|| edit
				.as_ref()
				.is_some_and(|edit| !edit.valid() || edit.is_empty())
		{
			return Err(Failure::Protocol);
		}
		if let Some(edit) = &edit {
			let (member, profile) = wire::encode(edit).map_err(|_| Failure::Protocol)?;
			let mut written = false;
			for (member_route, body) in [(true, member), (false, profile)] {
				if body.as_object().is_some_and(|body| body.is_empty()) {
					continue;
				}
				let path = if member_route {
					format!("/guilds/{guild}/members/@me")
				} else {
					format!("/guilds/{guild}/profile/@me")
				};
				// At most one attempt per changed route. The routes are not transactional:
				// any failure after the first successful write requires explicit reload.
				let bytes = zeroize::Zeroizing::new(
					self.request_limited(Method::PATCH, &path, Some(body), MAX_PROFILE_WIRE)
						.await
						.map_err(|failure| {
							if failure.ends_session() {
								failure
							} else if written || failure == Failure::Capacity {
								Failure::Ambiguous
							} else {
								failure
							}
						})?,
				);
				if member_route {
					wire::confirm_member(&bytes, user)
				} else {
					wire::confirm_profile(&bytes, guild)
				}
				.map_err(|_| Failure::Ambiguous)?;
				written = true;
			}
		}
		let bytes = self.request_limited(Method::GET, &format!("/users/{user}/profile?guild_id={guild}&with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false"), None, MAX_PROFILE_WIRE).await?;
		let saved = wire::decode(&bytes, user, guild).map_err(|_| {
			Failure::ProtocolAt("Full server profile is unavailable; reload before editing")
		})?;
		if edit.as_ref().is_some_and(|edit| !edit.matches(&saved)) {
			return Err(Failure::ProtocolAt(
				"Server profile changes were not confirmed; reload before retrying",
			));
		}
		Ok(Box::new(saved))
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use client_core::{Command, Event, auth::SessionSecret};
	use model::Patch;
	use serde_json::{Value, json};
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};

	pub(crate) async fn reply(
		listener: &TcpListener,
		route: &str,
		body: Option<Value>,
		status: u16,
		response: &str,
	) {
		let (mut socket, _) = listener.accept().await.unwrap();
		let mut bytes = Vec::new();
		loop {
			let mut chunk = [0; 2048];
			let count = socket.read(&mut chunk).await.unwrap();
			assert!(count > 0);
			bytes.extend_from_slice(&chunk[..count]);
			assert!(bytes.len() < 16384);
			if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
				let headers = std::str::from_utf8(&bytes[..end]).unwrap();
				let length = headers
					.lines()
					.find_map(|line| {
						line.to_ascii_lowercase()
							.strip_prefix("content-length: ")
							.and_then(|value| value.parse::<usize>().ok())
					})
					.unwrap_or_default();
				if bytes.len() < end + 4 + length {
					continue;
				}
				assert!(headers.starts_with(&format!("{route} HTTP/1.1\r\n")));
				let actual = (length > 0).then(|| {
					serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length]).unwrap()
				});
				assert_eq!(actual, body);
				break;
			}
		}
		socket
			.write_all(
				format!(
					"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
					response.len()
				)
				.as_bytes(),
			)
			.await
			.unwrap();
	}

	#[tokio::test]
	async fn edits_use_distinct_routes_confirm_readback_and_never_repeat_partial_writes() {
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let make_api = || {
				let mut api = DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_SERVER_PROFILE_TOKEN".into()).unwrap())).unwrap();
				api.base = format!("http://{}", listener.local_addr().unwrap());
				api
			};
			let get = "GET /users/1/profile?guild_id=2&with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false";
			let member = "PATCH /guilds/2/members/@me";
			let profile = "PATCH /guilds/2/profile/@me";
			let user = r#"{"user":{"id":"1","username":"synthetic"}}"#;
			let saved = json!({"user":{"id":"1","username":"synthetic"},"user_profile":{"bio":"global"},"guild_member":{"nick":"Server name"},"guild_member_profile":{"guild_id":"2","bio":"server bio","pronouns":null}}).to_string();
			let edit = Edit { nick: Patch::Value("Server name".into()), bio: Patch::Value("server bio".into()), pronouns: Patch::Null, ..Edit::default() };
			let server = async {
				reply(&listener, member, Some(json!({"nick":"Server name"})), 200, user).await;
				reply(&listener, profile, Some(json!({"bio":"server bio","pronouns":null})), 200, r#"{"guild_id":"2"}"#).await;
				reply(&listener, get, None, 200, &saved).await;
			};
			let api = make_api();
			let (event, ()) = tokio::join!(api.execute(Command::ServerProfile { user: Id(1), guild: Id(2), request: 42, edit: Some(Box::new(edit.clone())) }), server);
			let Event::ServerProfile(event) = event else { panic!("wrong event"); };
			assert_eq!((event.user, event.guild, event.request), (Id(1), Id(2), 42));
			assert!(edit.matches(&event.result.unwrap()));
			let server = async {
				reply(&listener, member, Some(json!({"nick":"Server name"})), 200, user).await;
				reply(&listener, profile, Some(json!({"bio":"server bio","pronouns":null})), 403, "{}").await;
			};
			let (result, ()) = tokio::join!(api.own_server_profile(Id(1), Id(2), Some(Box::new(edit.clone()))), server);
			assert_eq!(result.err(), Some(Failure::Ambiguous));
			for (status, expected) in [(403, Failure::Forbidden), (429, Failure::RateLimited), (500, Failure::Ambiguous)] {
				let api = make_api();
				let (result, ()) = tokio::join!(api.own_server_profile(Id(1), Id(2), Some(Box::new(edit.clone()))), reply(&listener, member, Some(json!({"nick":"Server name"})), status, "{}"));
				assert_eq!(result.err(), Some(expected));
			}
			let pronouns = Edit { pronouns: Patch::Value("they/them".into()), ..Edit::default() };
			let server = async {
				reply(&listener, profile, Some(json!({"pronouns":"they/them"})), 200, r#"{"guild_id":"2"}"#).await;
				reply(&listener, get, None, 200, &saved).await;
			};
			let (result, ()) = tokio::join!(api.own_server_profile(Id(1), Id(2), Some(Box::new(pronouns))), server);
			assert!(matches!(result, Err(Failure::ProtocolAt(_))));
			let (result, ()) = tokio::join!(api.own_server_profile(Id(1), Id(2), None), reply(&listener, get, None, 200, &saved));
			assert_eq!(result.unwrap().values.bio.as_deref(), Some("server bio"));
			assert!(api.own_server_profile(Id(0), Id(2), None).await.is_err());
			assert!(api.own_server_profile(Id(1), Id(2), Some(Box::default())).await.is_err());
			assert!(tokio::time::timeout(Duration::from_millis(20), listener.accept()).await.is_err(), "writes and failed readbacks must not automatically retry");
		}).await.unwrap();
	}
}
