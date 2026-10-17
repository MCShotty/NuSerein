//! Unofficial normal-account Server Guide routes, published by discord-userdoccers.
use crate::{DiscordApi, Failure};
use client_core::member_guide::Event;
use discord_protocol::{community_admin::MAX_WIRE, member_guide as wire};
use model::{
	Id,
	member_guide::{Action, Snapshot},
};
use reqwest::Method;
impl DiscordApi {
	async fn member_guide_snapshot(&self, guild: Id, user: Id) -> Result<Snapshot, Failure> {
		let guide = self
			.request_optional_limited(&format!("/guilds/{guild}/new-member-welcome"), MAX_WIRE)
			.await?
			.filter(|b| !b.is_empty())
			.unwrap_or_else(|| b"{}".to_vec());
		let progress = self
			.request_optional_limited(&format!("/guilds/{guild}/new-member-actions"), MAX_WIRE)
			.await?
			.unwrap_or_default();
		wire::snapshot(guild, user, &guide, &progress)
			.map_err(|_| Failure::ProtocolAt("Server Guide response was unsupported"))
	}
	pub(super) async fn member_guide(
		&self,
		guild: Id,
		user: Id,
		request: u64,
		action: Action,
	) -> Event {
		let result = async {
			if guild.0 == 0 || user.0 == 0 {
				return Err(Failure::Protocol);
			}
			if let Action::Complete { channel, kind } = action {
				if channel.0 == 0 || kind > 1 {
					return Err(Failure::Protocol);
				}
				let current = self.member_guide_snapshot(guild, user).await?;
				if !current.guide.enabled
					|| !current
						.guide
						.actions
						.iter()
						.any(|r| r.channel == channel && r.action == kind)
				{
					return Err(Failure::Forbidden);
				}
				if current.completed.contains(&channel) {
					return Ok(current);
				}
				let bytes = self
					.request_limited(
						Method::POST,
						&format!("/guilds/{guild}/new-member-action/{channel}"),
						None,
						MAX_WIRE,
					)
					.await
					.map_err(|f| match f {
						Failure::Network | Failure::Protocol | Failure::ProtocolAt(_) => {
							Failure::Ambiguous
						}
						other => other,
					})?;
				let completed = wire::progress(guild, user, &current.guide, &bytes)
					.map_err(|_| Failure::Ambiguous)?;
				if !completed.contains(&channel) {
					return Err(Failure::Ambiguous);
				}
				return Ok(Snapshot {
					completed,
					..current
				});
			}
			let mut snapshot = self.member_guide_snapshot(guild, user).await?;
			match self
				.request_limited(Method::GET, &format!("/guilds/{guild}"), None, MAX_WIRE)
				.await
			{
				Ok(bytes) => {
					if let Ok(raw) = discord_protocol::community_admin::raw(&bytes)
						&& raw["id"].as_str().and_then(|s| s.parse::<Id>().ok()) == Some(guild)
					{
						let available = raw["features"]
							.as_array()
							.filter(|a| a.len() <= 256)
							.is_some_and(|a| a.iter().any(|v| v.as_str() == Some("BANNER")));
						let header = raw["home_header"]
							.as_str()
							.filter(|hash| model::valid_avatar_hash(hash))
							.map(str::to_owned);
						snapshot.guide.header = if available { header } else { None };
						snapshot.guide.header_available = available;
						snapshot.header_known = true;
					}
				}
				Err(failure) if failure.ends_session() => return Err(failure),
				Err(_) => {}
			}
			Ok(snapshot)
		}
		.await;
		Event {
			guild,
			user,
			request,
			result,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::time::Duration;
	fn guide(kind: u8) -> String {
		serde_json::json!({"guild_id":"1","enabled":true,"welcome_message":{"author_ids":[],"message":"Welcome"},"new_member_actions":[{"channel_id":"3","action_type":kind,"title":"Read","description":""}],"resource_channels":[]}).to_string()
	}
	#[tokio::test]
	async fn member_guide_complete_uses_published_singular_post_route_and_no_body() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let (api, captured, server) = crate::community_admin::tests::fixture(vec![
				("200 OK", guide(0)),
				("204 No Content", String::new()),
				(
					"200 OK",
					r#"{"guild_id":"1","user_id":"2","channel_actions":{"3":{"completed":true}}}"#
						.into(),
				),
			])
			.await;
			let event = api
				.member_guide(
					Id(1),
					Id(2),
					1,
					Action::Complete {
						channel: Id(3),
						kind: 0,
					},
				)
				.await;
			assert_eq!(event.result.unwrap().completed, vec![Id(3)]);
			server.await.unwrap();
			let requests = captured.lock().unwrap();
			assert_eq!(requests.len(), 3);
			assert!(
				requests[2]
					.0
					.starts_with("POST /guilds/1/new-member-action/3 ")
			);
			assert!(requests[2].1.is_null());
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn member_guide_action_type_change_never_marks_a_chat_step_from_a_visit() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let (api, captured, server) = crate::community_admin::tests::fixture(vec![
				("200 OK", guide(1)),
				("204 No Content", String::new()),
			])
			.await;
			assert_eq!(
				api.member_guide(
					Id(1),
					Id(2),
					1,
					Action::Complete {
						channel: Id(3),
						kind: 0
					}
				)
				.await
				.result
				.unwrap_err(),
				Failure::Forbidden
			);
			server.await.unwrap();
			assert_eq!(captured.lock().unwrap().len(), 2);
		})
		.await
		.unwrap();
	}
}
