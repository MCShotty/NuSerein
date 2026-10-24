//! Single-attempt message writes and user-solved invite verification.
use super::{DiscordApi, Event, Failure, Method, Reply, allowed_mentions, message_options};
use discord_protocol::{MAX_WIRE, MessageDto, decode};

impl DiscordApi {
	/// One text send that reports a service captcha instead of a terminal rejection.
	pub(super) async fn send_text(
		&self,
		channel: model::Id,
		content: &str,
		nonce: String,
		reply: Option<Reply>,
		sticker: Option<model::Id>,
		retry: Option<&client_core::captcha::Retry>,
	) -> Event {
		let mut challenge = None;
		let result = self
			.send_message_with_captcha(
				channel,
				content,
				&nonce,
				(reply, None, sticker),
				retry,
				Some(&mut challenge),
			)
			.await;
		match challenge {
			Some(challenge) => Event::SendChallenge {
				nonce,
				reply,
				challenge: Box::new(challenge),
			},
			None => Event::SendResult { nonce, result },
		}
	}
	pub(super) async fn send_message(
		&self,
		channel: model::Id,
		content: &str,
		nonce: &str,
		reply: Option<Reply>,
		attachment: Option<Vec<serde_json::Value>>,
		sticker: Option<model::Id>,
	) -> Result<model::Message, Failure> {
		self.send_message_with_captcha(
			channel,
			content,
			nonce,
			(reply, attachment, sticker),
			None,
			None,
		)
		.await
	}
	pub(super) async fn send_message_with_captcha(
		&self,
		channel: model::Id,
		content: &str,
		nonce: &str,
		(reply, attachment, sticker): (
			Option<Reply>,
			Option<Vec<serde_json::Value>>,
			Option<model::Id>,
		),
		retry: Option<&client_core::captcha::Retry>,
		challenge: Option<&mut Option<client_core::captcha::Challenge>>,
	) -> Result<model::Message, Failure> {
		if !message_options::valid(
			content,
			client_core::MAX_PREMIUM_CONTENT,
			attachment.as_ref().is_some_and(|items| !items.is_empty()) || sticker.is_some(),
		) {
			return Err(Failure::Capacity);
		}
		let (content, silent) = message_options::content(content);
		if sticker.is_some_and(|id| id.0 == 0) {
			return Err(Failure::Protocol);
		}
		let mut body = serde_json::json!({"content":content,"nonce":nonce,"allowed_mentions":allowed_mentions(content, reply)});
		if silent {
			body["flags"] = serde_json::json!(message_options::SUPPRESS_NOTIFICATIONS);
		}
		if let Some(sticker) = sticker {
			body["sticker_ids"] = serde_json::json!([sticker]);
		}
		if let Some(reply) = reply {
			body["message_reference"] =
				serde_json::json!({"message_id":reply.target(),"channel_id":channel});
		}
		if let Some(attachment) = attachment {
			body["attachments"] = serde_json::json!(attachment);
		}
		// No enforce_nonce claim until normal-user semantics are live verified. Never auto-retry writes.
		self.request_with_captcha(
			Method::POST,
			&format!("/channels/{channel}/messages"),
			Some(body),
			MAX_WIRE,
			retry,
			challenge,
		)
		.await
		.and_then(|bytes| {
			let message = decode::<MessageDto>(&bytes).map_err(|_| Failure::Ambiguous)?;
			if message.channel_id != channel {
				return Err(Failure::Ambiguous);
			}
			Ok(message.into_model())
		})
	}
}
