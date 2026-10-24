//! Bounded history windows, reply originals and conversation search.
use super::{DiscordApi, Event, Failure, Id, Instant, Method, safe_delay};
use discord_protocol::{MessageDto, decode, pins, search};

impl DiscordApi {
	/// One bounded 50-message history page, before or after a cursor or at the latest message.
	pub(super) async fn history_around(&self, channel: Id, target: Id, request: u64) -> Event {
		let result = self
			.request(
				Method::GET,
				&format!("/channels/{channel}/messages?limit=50&around={target}"),
				None,
			)
			.await
			.and_then(|bytes| decode::<Vec<MessageDto>>(&bytes).map_err(|_| Failure::Protocol));
		match result {
			Ok(messages) if messages.len() <= 50 => Event::History {
				channel,
				request,
				older: false,
				messages: messages.into_iter().map(MessageDto::into_model).collect(),
			},
			Ok(_) => Event::HistoryFailed {
				channel,
				request,
				failure: Failure::Protocol,
			},
			Err(failure) if failure.ends_session() => Event::Failure(failure),
			Err(failure) => Event::HistoryFailed {
				channel,
				request,
				failure,
			},
		}
	}
	pub(super) async fn reply_preview(&self, channel: Id, message: Id, request: u64) -> Event {
		let result = self
			.request(
				Method::GET,
				&format!("/channels/{channel}/messages?limit=1&around={message}"),
				None,
			)
			.await
			.and_then(|bytes| {
				let mut messages =
					decode::<Vec<MessageDto>>(&bytes).map_err(|_| Failure::Protocol)?;
				if messages.is_empty() {
					return Ok(None);
				}
				if messages.len() != 1 {
					return Err(Failure::Protocol);
				}
				let dto = messages.pop().ok_or(Failure::Protocol)?;
				// Normal-user history can return a neighbouring row when the original is gone.
				if dto.id != message || dto.channel_id != channel {
					return Ok(None);
				}
				model::ReplyPreview::from_message(dto.into_model())
					.map(|preview| Some(Box::new(preview)))
					.ok_or(Failure::Protocol)
			});
		Event::ReplyPreview(client_core::reply_previews::ResultEvent {
			channel,
			message,
			request,
			result,
		})
	}
	pub(super) async fn history(
		&self,
		channel: Id,
		before: Option<Id>,
		after: Option<Id>,
		request: u64,
	) -> Event {
		if before.is_some() && after.is_some() {
			return Event::Failure(Failure::Protocol);
		}
		let mut path = format!("/channels/{channel}/messages?limit=50");
		if let Some(before) = before {
			path.push_str(&format!("&before={before}"));
		}
		if let Some(after) = after {
			path.push_str(&format!("&after={after}"));
		}
		match self
			.request(Method::GET, &path, None)
			.await
			.and_then(|bytes| decode::<Vec<MessageDto>>(&bytes).map_err(|_| Failure::Protocol))
		{
			Ok(messages) if messages.len() <= 50 => Event::History {
				channel,
				request,
				older: before.is_some(),
				messages: messages.into_iter().map(MessageDto::into_model).collect(),
			},
			Ok(_) => Event::Failure(Failure::Capacity),
			Err(Failure::Forbidden) => Event::Unavailable(channel),
			Err(f) => Event::Failure(f),
		}
	}
	pub(super) async fn pins(
		&self,
		channel: model::Id,
		before: Option<i128>,
	) -> Result<client_core::search::Outcome, Failure> {
		let mut path = format!("/channels/{channel}/messages/pins?limit=25");
		if let Some(cursor) = before {
			let timestamp = pins::format_cursor(cursor).map_err(|_| Failure::Protocol)?;
			let encoded: String = timestamp
				.bytes()
				.map(|byte| format!("%{byte:02X}"))
				.collect();
			path.push_str(&format!("&before={encoded}"));
		}
		let bytes = self
			.request_limited(Method::GET, &path, None, search::MAX_WIRE)
			.await?;
		decode::<discord_protocol::pins::Reply>(&bytes)
			.map_err(|_| Failure::Protocol)?
			.into_page(channel, before)
			.map(client_core::search::Outcome::Pins)
			.map_err(|_| Failure::Protocol)
	}
	pub(super) async fn search(
		&self,
		channel: model::Id,
		guild: Option<model::Id>,
		query: &str,
		before: Option<model::Id>,
		offset: u32,
		oldest: bool,
	) -> Result<client_core::search::Outcome, Failure> {
		if !model::valid_search_query(query) || offset > model::MAX_SEARCH_OFFSET {
			return Err(Failure::Protocol);
		}
		let (content, filters) = model::search_terms(query).map_err(|_| Failure::Protocol)?;
		// Encode values separately; user input cannot add arbitrary query parameters.
		let encoded: String = content.bytes().map(|b| format!("%{b:02X}")).collect();
		// Server searches span every readable channel unless `in:` narrows them.
		let mut path = match guild {
			Some(guild) => format!("/guilds/{guild}/messages/search?"),
			None => format!("/channels/{channel}/messages/search?"),
		};
		// Discord sorts the whole result set; ascending puts the earliest match on page one.
		path.push_str(&format!(
			"content={encoded}&limit=25&sort_by=timestamp&sort_order={}",
			if oldest { "asc" } else { "desc" }
		));
		// The cursor continues past the previous page's last hit: older for newest-first,
		// newer for oldest-first.
		let (mut maximum, mut minimum) = if oldest {
			(None, before.map(|id| id.0))
		} else {
			(before.map(|id| id.0), None)
		};
		for (key, value) in filters {
			if key == "channel_id" && guild.is_none() {
				return Err(Failure::Protocol);
			}
			if key == "max_id" {
				let id = value.parse::<u64>().map_err(|_| Failure::Protocol)?;
				maximum = Some(maximum.map_or(id, |current| current.min(id)));
			} else if key == "min_id" {
				let id = value.parse::<u64>().map_err(|_| Failure::Protocol)?;
				minimum = Some(minimum.map_or(id, |current| current.max(id)));
			} else {
				let encoded: String = value.bytes().map(|b| format!("%{b:02X}")).collect();
				path.push_str(&format!("&{key}={encoded}"));
			}
		}
		if let Some(before) = maximum {
			path.push_str(&format!("&max_id={before}"));
		}
		if let Some(after) = minimum {
			path.push_str(&format!("&min_id={after}"));
		}
		if offset != 0 {
			path.push_str(&format!("&offset={offset}"));
		}
		let bytes = self
			.request_limited(Method::GET, &path, None, search::MAX_WIRE)
			.await?;
		let reply = decode::<search::Reply>(&bytes).map_err(|_| Failure::Protocol)?;
		if reply
			.code
			.is_some_and(|code| (110000..119999).contains(&code))
		{
			let mut next = self.cooldown.lock().await;
			*next = (*next).max(Instant::now() + safe_delay(reply.retry_after)?);
			return Ok(client_core::search::Outcome::Indexing);
		}
		reply
			.into_page(
				guild.is_none().then_some(channel),
				if oldest { minimum } else { maximum }.map(model::Id),
				oldest,
			)
			.map(client_core::search::Outcome::Page)
			.map_err(|_| Failure::Protocol)
	}
}
