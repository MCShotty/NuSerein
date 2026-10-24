//! Explicit channel, server and bulk read acknowledgements.
use super::{DiscordApi, Failure, Method};
use discord_protocol::decode;

impl DiscordApi {
	pub(super) async fn mark_read(
		&self,
		channel: model::Id,
		message: model::Id,
		manual: bool,
		mention_count: Option<u32>,
	) -> Result<(), Failure> {
		#[derive(serde::Deserialize)]
		struct Reply {
			#[serde(default)]
			token: Option<String>,
		}
		// Legacy acknowledgement tokens are session-only, redacted by ownership, and never cached.
		// Manual mark-unread omits the token, matching the unofficial normal-user ack body.
		let mut token = self.ack_token.lock().await;
		let body = if manual {
			let mut body = serde_json::json!({"manual": true});
			if let Some(count) = mention_count {
				body["mention_count"] = count.into();
			}
			body
		} else {
			serde_json::json!({"token":token.as_deref(),"manual":false})
		};
		let bytes = zeroize::Zeroizing::new(
			self.request_limited(
				Method::POST,
				&format!("/channels/{channel}/messages/{message}/ack"),
				Some(body),
				4096,
			)
			.await?,
		);
		let next = zeroize::Zeroizing::new(if bytes.is_empty() {
			None
		} else {
			decode::<Reply>(&bytes)
				.map_err(|_| Failure::Ambiguous)?
				.token
		});
		if next
			.as_ref()
			.is_some_and(|t| t.len() > 2048 || t.chars().any(char::is_control))
		{
			return Err(Failure::Ambiguous);
		}
		*token = next;
		Ok(())
	}
	pub(super) async fn mark_guild_read(&self, guild: model::Id) -> Result<(), Failure> {
		self.request_limited(Method::POST, &format!("/guilds/{guild}/ack"), None, 4096)
			.await
			.map(|_| ())
	}
	/// Unofficial normal-user bulk acknowledgement in 100-channel requests (unverified limit).
	/// Returns how many leading channels were confirmed, even when a later request fails.
	pub(super) async fn mark_all_read(
		&self,
		channels: &[(model::Id, model::Id)],
	) -> (usize, Result<(), Failure>) {
		if channels.len() > client_core::MAX_BULK_ACK {
			return (0, Err(Failure::Protocol));
		}
		let mut acked = 0;
		for chunk in channels.chunks(100) {
			let read_states = chunk
				.iter()
				.map(|(channel, message)| {
					serde_json::json!({
						"channel_id": channel.to_string(),
						"message_id": message.to_string(),
						"read_state_type": 0,
					})
				})
				.collect::<Vec<_>>();
			if let Err(failure) = self
				.request_limited(
					Method::POST,
					"/read-states/ack-bulk",
					Some(serde_json::json!({ "read_states": read_states })),
					4096,
				)
				.await
			{
				return (acked, Err(failure));
			}
			acked += chunk.len();
		}
		(acked, Ok(()))
	}
}
