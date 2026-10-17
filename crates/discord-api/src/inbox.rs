use crate::{DiscordApi, Failure};
use client_core::inbox::{MAX_WIRE, PAGE_SIZE};
use discord_protocol::{MessageDto, RecentMentions, decode};
use model::{Id, Message};
use reqwest::Method;

impl DiscordApi {
	pub(super) async fn inbox_action(
		&self,
		action: &client_core::inbox::Action,
	) -> Result<client_core::inbox::Outcome, Failure> {
		use client_core::inbox::{Action, Outcome};
		if !action.valid() {
			return Err(Failure::Protocol);
		}
		let (method, path, body) = match action {
			Action::Saved => (Method::GET, "/users/@me/saved-messages".into(), None),
			Action::Dismiss { message, .. } => (
				Method::DELETE,
				format!("/users/@me/mentions/{message}"),
				None,
			),
			Action::Save {
				channel,
				message,
				due_at,
			} => {
				let due = due_at
					.map(discord_protocol::pins::format_cursor)
					.transpose()
					.map_err(|_| Failure::Protocol)?;
				(
					Method::PUT,
					format!("/users/@me/saved-messages/{channel}/{message}"),
					Some(serde_json::json!({"due_at":due})),
				)
			}
			Action::Unsave { channel, message } => (
				Method::DELETE,
				format!("/users/@me/saved-messages/{channel}/{message}"),
				None,
			),
		};
		let bytes = self
			.request_limited(method, &path, body, MAX_WIRE)
			.await
			.map_err(|failure| {
				if action.write()
					&& matches!(
						failure,
						Failure::Network | Failure::Protocol | Failure::Capacity
					) {
					Failure::Ambiguous
				} else {
					failure
				}
			})?;
		match action {
			Action::Saved => discord_protocol::inbox::saved(&bytes)
				.map(Outcome::Saved)
				.map_err(|_| {
					Failure::ProtocolAt(
						"Saved messages are unsupported or exceed the display limit",
					)
				}),
			Action::Save {
				channel,
				message,
				due_at,
			} => {
				let saved =
					discord_protocol::inbox::saved_one(&bytes).map_err(|_| Failure::Ambiguous)?;
				if saved.channel != *channel || saved.message != *message || saved.due_at != *due_at
				{
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::SavedOne(Box::new(saved)))
			}
			_ if bytes.is_empty() => Ok(Outcome::Done),
			_ => Err(Failure::Ambiguous),
		}
	}
	pub(super) async fn mentions(&self, before: Option<Id>) -> Result<Vec<Message>, Failure> {
		if before.is_some_and(|id| id.0 == 0) {
			return Err(Failure::Protocol);
		}
		let mut path = format!("/users/@me/mentions?limit={PAGE_SIZE}&roles=true&everyone=true");
		if let Some(before) = before {
			path.push_str(&format!("&before={before}"));
		}
		let bytes = self
			.request_limited(Method::GET, &path, None, MAX_WIRE)
			.await?;
		let RecentMentions(messages) =
			decode::<RecentMentions>(&bytes).map_err(|_| Failure::Protocol)?;
		if messages.len() > PAGE_SIZE {
			return Err(Failure::Capacity);
		}
		let mut messages: Vec<_> = messages.into_iter().map(MessageDto::into_model).collect();
		messages.sort_unstable_by_key(|message| std::cmp::Reverse(message.id));
		messages.shrink_to_fit();
		if messages.iter().map(Message::bytes).sum::<usize>() > client_core::inbox::MAX_BYTES {
			return Err(Failure::Capacity);
		}
		Ok(messages)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::SessionSecret;
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};

	#[tokio::test]
	async fn saved_and_dismiss_actions_use_scoped_routes_and_verify_confirmation() {
		use client_core::inbox::{Action, Outcome};
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_INBOX_TOKEN".into()).unwrap())).unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				let saved = r#"{"message":null,"save_data":{"channel_id":"20","message_id":"30","due_at":null}}"#;
				for (method,path,status,body) in [
					("GET","/users/@me/saved-messages","200 OK", "{\"results\":[]}"),
					("PUT","/users/@me/saved-messages/20/30","200 OK", saved),
					("DELETE","/users/@me/saved-messages/20/30","204 No Content", ""),
					("DELETE","/users/@me/mentions/30","204 No Content", ""),
					("PUT","/users/@me/saved-messages/20/31","200 OK", saved),
				] {
					let (mut stream,_) = listener.accept().await.unwrap();
					let mut request=Vec::new();
					loop {
						let mut chunk=[0;4096];
						let count=stream.read(&mut chunk).await.unwrap();
						assert!(count>0); request.extend_from_slice(&chunk[..count]); assert!(request.len()<16384);
						if let Some(end)=request.windows(4).position(|part|part==b"\r\n\r\n") {
							let headers=std::str::from_utf8(&request[..end]).unwrap();
							let length=headers.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|value|value.parse::<usize>().ok())).unwrap_or(0);
							if request.len()>=end+4+length { break; }
						}
					}
					assert!(std::str::from_utf8(&request).unwrap().starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
					if method=="PUT" { assert!(std::str::from_utf8(&request).unwrap().ends_with("{\"due_at\":null}")); }
					stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
				}
			});
			assert!(matches!(api.inbox_action(&Action::Saved).await.unwrap(),Outcome::Saved(items) if items.is_empty()));
			assert!(matches!(api.inbox_action(&Action::Save {channel:Id(20),message:Id(30),due_at:None}).await.unwrap(),Outcome::SavedOne(_)));
			assert!(matches!(api.inbox_action(&Action::Unsave {channel:Id(20),message:Id(30)}).await.unwrap(),Outcome::Done));
			assert!(matches!(api.inbox_action(&Action::Dismiss {channel:Id(20),message:Id(30)}).await.unwrap(),Outcome::Done));
			assert!(matches!(api.inbox_action(&Action::Save {channel:Id(20),message:Id(31),due_at:None}).await,Err(Failure::Ambiguous)));
			server.await.unwrap();
		}).await.unwrap();
	}

	#[tokio::test]
	async fn mentions_http_includes_role_everyone_and_before_parameters() {
		let message = serde_json::json!({"id":"10", "channel_id":"20", "author":{"id":"2", "username":"Synthetic"}, "content":"Mention"});
		assert!(
			decode::<RecentMentions>(
				&serde_json::to_vec(&vec![message.clone(); PAGE_SIZE]).unwrap()
			)
			.is_ok()
		);
		assert!(
			decode::<RecentMentions>(&serde_json::to_vec(&vec![message; PAGE_SIZE + 1]).unwrap())
				.is_err()
		);
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_MENTIONS_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				for suffix in ["", "&before=80"] {
					let (mut stream, _) = listener.accept().await.unwrap();
					let mut bytes = vec![];
					loop {
						let mut chunk = [0; 4096];
						let n = stream.read(&mut chunk).await.unwrap();
						assert!(n > 0);
						bytes.extend_from_slice(&chunk[..n]);
						assert!(bytes.len() < 16_384);
						if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
							break;
						}
					}
					assert!(std::str::from_utf8(&bytes).unwrap().starts_with(&format!(
						"GET /users/@me/mentions?limit=25&roles=true&everyone=true{suffix} HTTP/1.1\r\n"
					)));
					stream
						.write_all(
							b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
						)
						.await
						.unwrap();
				}
			});
			assert!(api.mentions(None).await.unwrap().is_empty());
			assert!(api.mentions(Some(Id(80))).await.unwrap().is_empty());
			assert!(matches!(
				api.mentions(Some(Id(0))).await,
				Err(Failure::Protocol)
			));
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
}
