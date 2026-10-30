//! Ordinary-member read adapters for explicit background synchronization.
use crate::{DiscordApi, Failure, SPECULATIVE};
use client_core::force_sync_read::{self as sync, GuildRead, HistoryPage, Read, ReadResult};
use discord_protocol::{ChannelDto, MemberDto, MessageDto, decode};
use futures_util::{StreamExt, stream};
use model::{Id, Member, Patch};
use reqwest::Method;
use serde::{Deserialize, Deserializer, de::SeqAccess};
use serde_json::value::RawValue;
use std::collections::BTreeSet;

impl DiscordApi {
	/// Uses only spare REST capacity and the normal route/global rate admission.
	/// Busy/rate-limit/response failures belong to this read, never to the account session.
	pub async fn force_sync_read(&self, read: Read) -> Result<ReadResult, Failure> {
		if !read.valid() {
			return Err(Failure::Protocol);
		}
		SPECULATIVE
			.scope((), async {
				let result = self.force_sync_read_inner(read).await?;
				if result.bytes() > sync::MAX_READ_BYTES {
					return Err(Failure::ResponseLimit);
				}
				Ok(result)
			})
			.await
			.map_err(|failure| {
				if matches!(failure, Failure::Capacity | Failure::CapacityAt(_)) {
					Failure::Busy
				} else {
					failure
				}
			})
	}
	async fn force_sync_get(&self, path: &str, limit: usize) -> Result<Vec<u8>, Failure> {
		self.request_limited(Method::GET, path, None, limit)
			.await
			.map_err(|failure| {
				if matches!(failure, Failure::Capacity | Failure::CapacityAt(_)) {
					Failure::Busy
				} else {
					failure
				}
			})
	}
	async fn force_sync_member(
		&self,
		guild: Id,
		user: Id,
	) -> Result<(Member, Vec<u8>, bool), Failure> {
		let bytes = self
			.force_sync_get(&format!("/guilds/{guild}/members/{user}"), 64 * 1024)
			.await?;
		let dto: MemberDto = decode(&bytes).map_err(|_| Failure::Protocol)?;
		if dto.user.id != user || !matches!(&dto.roles, Patch::Value(_)) {
			return Err(Failure::Protocol);
		}
		let nick_known = !matches!(&dto.nick, Patch::Absent);
		let member = dto.into_guild_member(guild).ok_or(Failure::Protocol)?;
		if !member.valid() || member.bytes() > 64 * 1024 {
			return Err(Failure::ResponseLimit);
		}
		Ok((member, bytes, nick_known))
	}
	async fn force_sync_read_inner(&self, read: Read) -> Result<ReadResult, Failure> {
		match read {
			Read::Guild { guild, user } => {
				let bytes = self
					.force_sync_get(
						&format!("/guilds/{guild}?with_counts=true"),
						2 * 1024 * 1024,
					)
					.await?;
				let dto: discord_protocol::GuildDto =
					decode(&bytes).map_err(|_| Failure::Protocol)?;
				if dto.id != guild {
					return Err(Failure::Protocol);
				}
				let roles = discord_protocol::server_roles::catalog(&bytes, guild)
					.map_err(|_| Failure::Protocol)?;
				let mut permissions = discord_protocol::permissions::guild(&bytes, user)
					.map_err(|_| Failure::Protocol)?;
				#[derive(Deserialize)]
				struct Metadata {
					#[serde(default)]
					banner: Patch<String>,
					member_count: Option<u64>,
					approximate_member_count: Option<u64>,
				}
				let metadata: Metadata = decode(&bytes).map_err(|_| Failure::Protocol)?;
				if matches!(&metadata.banner, Patch::Value(hash) if !model::valid_avatar_hash(hash))
				{
					return Err(Failure::Protocol);
				}
				let mut self_member_nick_known = false;
				let self_member = match self.force_sync_member(guild, user).await {
					Ok((member, own_bytes, nick_known)) => {
						self_member_nick_known = nick_known;
						// Reuse the existing strict timeout/own-member permission projection.
						let mut value: serde_json::Value =
							decode(&bytes).map_err(|_| Failure::Protocol)?;
						value["members"] =
							serde_json::json!([decode::<serde_json::Value>(&own_bytes)
								.map_err(|_| Failure::Protocol)?]);
						let combined = serde_json::to_vec(&value).map_err(|_| Failure::Protocol)?;
						permissions = discord_protocol::permissions::guild(&combined, user)
							.map_err(|_| Failure::Protocol)?;
						Ok(member)
					}
					Err(failure) if failure.ends_session() => return Err(failure),
					Err(failure) => Err(failure),
				};
				Ok(ReadResult::Guild(Box::new(GuildRead {
					display: dto.display_patch(),
					permissions,
					roles,
					self_member,
					self_member_nick_known,
					banner: metadata.banner,
					member_count: metadata.member_count.or(metadata.approximate_member_count),
				})))
			}
			Read::Channels { guild, user } => {
				let bytes = self
					.force_sync_get(&format!("/guilds/{guild}/channels"), sync::MAX_READ_BYTES)
					.await?;
				let rows: Vec<Box<RawValue>> = decode(&bytes).map_err(|_| Failure::Protocol)?;
				if rows.len() > model::account::MAX_ENTRIES {
					return Err(Failure::ResponseLimit);
				}
				let mut seen = BTreeSet::new();
				let mut channels = Vec::new();
				let mut permissions = Vec::new();
				for raw in rows {
					let mut dto: ChannelDto =
						decode(raw.get().as_bytes()).map_err(|_| Failure::Protocol)?;
					if dto.id.0 == 0
						|| dto.guild_id.is_some_and(|id| id != guild)
						|| !seen.insert(dto.id)
						|| matches!(dto.kind, 1 | 3 | 10..=12)
					{
						return Err(Failure::Protocol);
					}
					if dto.is_obfuscated() {
						continue;
					}
					dto.guild_id = Some(guild);
					let overwrites =
						discord_protocol::permissions::channel(raw.get().as_bytes(), user)
							.map_err(|_| Failure::Protocol)?
							.and_then(|row| match row.overwrites {
								Patch::Value(rows) => Some(rows),
								_ => None,
							});
					permissions.push(model::permissions::Channel {
						id: dto.id,
						guild,
						overwrites,
					});
					channels.push(dto.into_model());
				}
				Ok(ReadResult::Channels {
					guild,
					channels,
					permissions,
				})
			}
			Read::Emojis { guild } => {
				let bytes = self
					.force_sync_get(&format!("/guilds/{guild}/emojis"), 2 * 1024 * 1024)
					.await?;
				let items =
					discord_protocol::guild_emojis(&bytes).map_err(|_| Failure::Protocol)?;
				Ok(ReadResult::Emojis { guild, items })
			}
			Read::Stickers { guild } => {
				let bytes = self
					.force_sync_get(&format!("/guilds/{guild}/stickers"), 2 * 1024 * 1024)
					.await?;
				let rows = decode::<discord_protocol::stickers::Catalog>(&bytes)
					.map_err(|_| Failure::Protocol)?
					.0;
				let items = discord_protocol::stickers::guild_catalog(rows, guild)
					.map_err(|_| Failure::Protocol)?;
				Ok(ReadResult::Stickers { guild, items })
			}
			Read::Sounds { guild } => {
				let bytes = self
					.force_sync_get(
						&format!("/guilds/{guild}/soundboard-sounds"),
						client_core::soundboard::MAX_WIRE_BYTES,
					)
					.await?;
				let items = discord_protocol::soundboard::guild_sounds(&bytes, guild)
					.map_err(|_| Failure::Protocol)?;
				Ok(ReadResult::Sounds { guild, items })
			}
			Read::History {
				channel,
				before,
				cutoff_ms,
				end_ms,
			} => {
				let end = sync::last_before(end_ms).ok_or(Failure::Protocol)?;
				let before = before.map_or(end, |before| before.min(end));
				let bytes = self
					.force_sync_get(
						&format!("/channels/{channel}/messages?limit=50&before={before}"),
						sync::MAX_READ_BYTES,
					)
					.await?;
				Ok(ReadResult::History(history_page(
					&bytes, channel, before, cutoff_ms, end_ms,
				)?))
			}
			Read::Archives {
				guild,
				parent,
				kind,
				before,
			} => self
				.archives(parent, guild, kind, before)
				.await
				.map(ReadResult::Archives),
			Read::ActiveThreads {
				guild,
				parent,
				before,
			} => {
				// The guild-wide active endpoint is bot-only. This per-channel search is
				// the normal-account route and also covers private threads when visible.
				let mut path = format!(
					"/channels/{parent}/threads/search?archived=false&sort_by=creation_time&sort_order=desc&limit={}",
					sync::THREAD_PAGE_SIZE
				);
				if let Some(before) = before {
					path.push_str(&format!("&max_id={before}"));
				}
				let bytes = self
					.force_sync_get(&path, discord_protocol::forum::MAX_WIRE)
					.await?;
				let status: discord_protocol::search::Reply =
					decode(&bytes).map_err(|_| Failure::Protocol)?;
				if status
					.code
					.is_some_and(|code| (110_000..120_000).contains(&code))
				{
					let retry_after = crate::safe_delay(status.retry_after)?;
					self.cooldown.lock().await.observe(
						&crate::rate_limits::Route::new(&Method::GET, &path),
						None,
						Some(retry_after),
						false,
					)?;
					return Ok(ReadResult::ActiveThreadsIndexing {
						guild,
						parent,
						retry_after,
					});
				}
				if status.code.is_some() {
					return Err(Failure::Protocol);
				}
				active_thread_page(&bytes, guild, parent, before)
			}
			Read::Members { guild, users } => {
				let mut items = Vec::new();
				let mut missing = Vec::new();
				let mut nick_unknown = Vec::new();
				let mut pending = stream::iter(users)
					.map(|user| async move {
						let path = format!("/guilds/{guild}/members/{user}");
						// 404 is a confirmed departure, while 403 and transient failures remain failures.
						let bytes = self.request_optional_limited(&path, 64 * 1024).await?;
						let Some(bytes) = bytes else {
							return Ok((user, None));
						};
						let dto: MemberDto = decode(&bytes).map_err(|_| Failure::Protocol)?;
						if dto.user.id != user || !matches!(&dto.roles, Patch::Value(_)) {
							return Err(Failure::Protocol);
						}
						let nick_known = !matches!(&dto.nick, Patch::Absent);
						let member = dto.into_guild_member(guild).ok_or(Failure::Protocol)?;
						if !member.valid() {
							return Err(Failure::Protocol);
						}
						Ok((user, Some((member, nick_known))))
					})
					.buffered(2);
				while let Some(result) = pending.next().await {
					match result? {
						(user, None) => missing.push(user),
						(user, Some((member, nick_known))) => {
							if !nick_known {
								nick_unknown.push(user);
							}
							items.push(member);
						}
					}
					if items.iter().map(Member::bytes).sum::<usize>() > sync::MAX_READ_BYTES {
						return Err(Failure::ResponseLimit);
					}
				}
				Ok(ReadResult::Members {
					guild,
					items,
					missing,
					nick_unknown,
				})
			}
		}
	}
}

#[derive(Deserialize)]
struct ActiveThreadRow {
	#[serde(flatten)]
	channel: ChannelDto,
	#[serde(default)]
	thread_metadata: Option<ActiveThreadMetadata>,
}

#[derive(Deserialize)]
struct ActiveThreadMetadata {
	#[serde(default)]
	archived: bool,
}

#[derive(Deserialize)]
struct ActiveThreadReply {
	#[serde(deserialize_with = "active_thread_rows")]
	threads: Vec<ActiveThreadRow>,
	has_more: bool,
}

fn active_thread_rows<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ActiveThreadRow>, D::Error> {
	struct Bounded;
	impl<'de> serde::de::Visitor<'de> for Bounded {
		type Value = Vec<ActiveThreadRow>;
		fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			formatter.write_str("at most 25 active threads")
		}
		fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
			let mut rows = Vec::new();
			while rows.len() < sync::THREAD_PAGE_SIZE {
				match seq.next_element()? {
					Some(row) => rows.push(row),
					None => return Ok(rows),
				}
			}
			if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
				return Err(serde::de::Error::custom("active thread page exceeds limit"));
			}
			Ok(rows)
		}
	}
	d.deserialize_seq(Bounded)
}

fn active_thread_page(
	bytes: &[u8],
	guild: Id,
	parent: Id,
	before: Option<Id>,
) -> Result<ReadResult, Failure> {
	let rows: ActiveThreadReply = decode(bytes).map_err(|_| Failure::Protocol)?;
	let mut cursor = before;
	let mut last = None;
	let mut retained = 0usize;
	let mut threads = Vec::with_capacity(rows.threads.len());
	for row in rows.threads {
		let mut channel = row.channel;
		if channel.id.0 == 0
			|| channel.parent_id != Some(parent)
			|| channel.guild_id.is_some_and(|id| id != guild)
			|| !matches!(channel.kind, 10..=12)
			|| cursor.is_some_and(|id| channel.id >= id)
			|| row
				.thread_metadata
				.is_some_and(|metadata| metadata.archived)
		{
			return Err(Failure::Protocol);
		}
		cursor = Some(channel.id);
		last = cursor;
		let hidden = channel.is_obfuscated();
		// Validate hidden rows too, and advance past them to avoid a stuck cursor.
		channel.flags &= !(1 << 17);
		let channel = discord_protocol::threads::into_thread(channel, guild)
			.map_err(|_| Failure::Protocol)?;
		retained = retained.saturating_add(channel.bytes());
		if retained > model::forum::MAX_BYTES {
			return Err(Failure::ResponseLimit);
		}
		if !hidden {
			threads.push(channel);
		}
	}
	let next = if rows.has_more {
		Some(last.ok_or(Failure::Protocol)?)
	} else {
		None
	};
	let page = ReadResult::ActiveThreads {
		guild,
		parent,
		threads,
		next,
	};
	if page.bytes() > model::forum::MAX_BYTES {
		return Err(Failure::ResponseLimit);
	}
	Ok(page)
}

fn history_page(
	bytes: &[u8],
	channel: Id,
	before: Id,
	cutoff_ms: i64,
	end_ms: i64,
) -> Result<HistoryPage, Failure> {
	let rows: Vec<MessageDto> = decode(bytes).map_err(|_| Failure::Protocol)?;
	if rows.len() > sync::PAGE_MESSAGES {
		return Err(Failure::ResponseLimit);
	}
	let channel_beginning = rows.len() < sync::PAGE_MESSAGES;
	let mut seen = BTreeSet::new();
	let mut messages = Vec::new();
	let mut oldest = None;
	let mut crossed = false;
	for row in rows {
		if row.channel_id != channel || row.id.0 == 0 || row.id >= before || !seen.insert(row.id) {
			return Err(Failure::Protocol);
		}
		oldest = Some(oldest.map_or(row.id, |old: Id| old.min(row.id)));
		let at = sync::timestamp_ms(row.id);
		crossed |= at < cutoff_ms;
		if at < cutoff_ms || at > end_ms {
			continue;
		}
		let message = row.into_model();
		if !sync::valid_history_message(&message) || message.flags & 64 != 0 {
			return Err(Failure::Protocol);
		}
		messages.push(message);
	}
	messages.sort_by_key(|message| std::cmp::Reverse(message.id));
	if messages.iter().map(model::Message::bytes).sum::<usize>() > sync::MAX_READ_BYTES {
		return Err(Failure::ResponseLimit);
	}
	Ok(HistoryPage {
		channel,
		messages,
		oldest,
		boundary_reached: channel_beginning || crossed,
		channel_beginning,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::auth::SessionSecret;
	use serde_json::{Value, json};
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};

	async fn fixture(
		replies: Vec<(String, &'static str, Value)>,
	) -> (DiscordApi, tokio::task::JoinHandle<()>) {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut api = DiscordApi::new(Arc::new(
			SessionSecret::from_owner_input("SYNTHETIC_FORCE_SYNC_TOKEN".into()).unwrap(),
		))
		.unwrap();
		api.base = format!("http://{}", listener.local_addr().unwrap());
		let task = tokio::spawn(async move {
			for (path, status, body) in replies {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut request = Vec::new();
				loop {
					let mut bytes = [0; 1024];
					let size = socket.read(&mut bytes).await.unwrap();
					assert!(size > 0);
					request.extend_from_slice(&bytes[..size]);
					assert!(request.len() <= 8192);
					if request.windows(4).any(|part| part == b"\r\n\r\n") {
						break;
					}
				}
				assert!(
					std::str::from_utf8(&request)
						.unwrap()
						.starts_with(&format!("GET {path} HTTP/1.1\r\n"))
				);
				let body = body.to_string();
				socket
					.write_all(
						format!(
							"HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await
					.unwrap();
			}
		});
		(api, task)
	}
	fn member(user: &str) -> Value {
		json!({"user":{"id":user,"username":"account-name"},"nick":"Server nickname","roles":["3"],"avatar":"0123456789abcdef0123456789abcdef","communication_disabled_until":null})
	}
	fn guild() -> Value {
		json!({"id":"2","owner_id":"1","name":"Refreshed","icon":null,"banner":null,"features":[],"member_count":2,"roles":[
			{"id":"2","name":"@everyone","permissions":"68608","color":0,"position":0,"hoist":false,"mentionable":false,"managed":false},
			{"id":"3","name":"New role","permissions":"0","color":1122867,"colors":{"primary_color":1122867,"secondary_color":4478310,"tertiary_color":7833753},"unicode_emoji":"🌟","position":1,"hoist":true,"mentionable":true,"managed":false}
		]})
	}
	#[tokio::test]
	async fn force_sync_metadata_reads_use_ordinary_routes_and_keep_role_colors_and_nicknames() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let (api, server) = fixture(vec![
				("/guilds/2?with_counts=true".into(), "200 OK", guild()),
				("/guilds/2/members/1".into(), "200 OK", member("1")),
				("/guilds/2/channels".into(), "200 OK", json!([{"id":"4","guild_id":"2","name":"chat","type":0,"permission_overwrites":[{"id":"2","type":0,"allow":"1024","deny":"0"}]}])),
				("/guilds/2/emojis".into(), "200 OK", json!([])),
				("/guilds/2/stickers".into(), "200 OK", json!([])),
				("/guilds/2/soundboard-sounds".into(), "200 OK", json!({"items":[]})),
				("/channels/4/threads/search?archived=false&sort_by=creation_time&sort_order=desc&limit=25".into(), "200 OK", json!({"threads":[{"id":"8","guild_id":"2","parent_id":"4","name":"Old post still active","type":11}],"has_more":false})),
			]).await;
			let ReadResult::Guild(value) = api.force_sync_read(Read::Guild { guild: Id(2), user: Id(1) }).await.unwrap() else { panic!("guild result") };
			assert_eq!(value.self_member.as_ref().unwrap().nick.as_deref(), Some("Server nickname"));
			assert_eq!(value.self_member.as_ref().unwrap().user.avatar_key(), "member-avatar-2-1-0123456789abcdef0123456789abcdef");
			assert_eq!(value.roles.items[0].colors.secondary, Some(4478310));
			assert_eq!(value.roles.items[0].colors.tertiary, Some(7833753));
			assert!(value.roles.items[0].mentionable);
			assert_eq!(value.permissions.guilds[0].member.as_ref().unwrap().roles, vec![Id(3)]);
			assert!(matches!(value.display.icon, Patch::Null));
			let ReadResult::Channels { channels, permissions, .. } = api.force_sync_read(Read::Channels { guild: Id(2), user: Id(1) }).await.unwrap() else { panic!("channels result") };
			assert_eq!(channels[0].name, "chat");
			assert_eq!(permissions[0].overwrites.as_ref().unwrap()[0].allow, 1024);
			assert!(matches!(api.force_sync_read(Read::Emojis { guild: Id(2) }).await.unwrap(), ReadResult::Emojis { items, .. } if items.is_empty()));
			assert!(matches!(api.force_sync_read(Read::Stickers { guild: Id(2) }).await.unwrap(), ReadResult::Stickers { items, .. } if items.is_empty()));
			assert!(matches!(api.force_sync_read(Read::Sounds { guild: Id(2) }).await.unwrap(), ReadResult::Sounds { items, .. } if items.is_empty()));
			assert!(matches!(api.force_sync_read(Read::ActiveThreads { guild: Id(2), parent: Id(4), before: None }).await.unwrap(), ReadResult::ActiveThreads { threads, next: None, .. } if threads[0].id == Id(8)));
			server.await.unwrap(); assert!(!api.stopped());
		}).await.unwrap();
	}
	const THREAD_ROUTE: &str =
		"/channels/4/threads/search?archived=false&sort_by=creation_time&sort_order=desc&limit=25";
	fn thread(id: u64, kind: u8) -> Value {
		json!({
			"id": id.to_string(), "guild_id": "2", "parent_id": "4",
			"type": kind, "name": "Synthetic thread", "thread_metadata": {"archived": false}
		})
	}
	fn thread_read(before: Option<Id>) -> Read {
		Read::ActiveThreads {
			guild: Id(2),
			parent: Id(4),
			before,
		}
	}
	#[tokio::test]
	async fn member_refresh_preserves_missing_nickname_presence() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let (api, server) = fixture(vec![(
				"/guilds/2/members/3".into(),
				"200 OK",
				json!({"user":{"id":"3","username":"synthetic"},"roles":["9"]}),
			)])
			.await;
			let ReadResult::Members {
				items,
				missing,
				nick_unknown,
				..
			} = api.force_sync_read(Read::Members {
				guild: Id(2),
				users: vec![Id(3)],
			})
			.await
			.unwrap()
			else {
				panic!("member result")
			};
			assert!(missing.is_empty());
			assert_eq!(nick_unknown, vec![Id(3)]);
			assert_eq!(items[0].roles, vec![Id(9)]);
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn active_threads_page_through_private_and_obfuscated_rows() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let mut hidden = thread(20, 11);
			hidden["flags"] = json!(1 << 17);
			let (api, server) = fixture(vec![
				(
					THREAD_ROUTE.into(),
					"200 OK",
					json!({"threads": [thread(30, 11), thread(25, 12), hidden], "has_more": true}),
				),
				(
					format!("{THREAD_ROUTE}&max_id=20"),
					"200 OK",
					json!({"threads": [thread(19, 10)], "has_more": false}),
				),
			])
			.await;
			let ReadResult::ActiveThreads {
				guild,
				parent,
				threads,
				next,
			} = api.force_sync_read(thread_read(None)).await.unwrap()
			else {
				panic!("active thread page")
			};
			assert_eq!((guild, parent, next), (Id(2), Id(4), Some(Id(20))));
			assert_eq!(
				threads.iter().map(|row| row.id).collect::<Vec<_>>(),
				vec![Id(30), Id(25)]
			);
			assert_eq!(threads[1].kind, 12, "private threads stay visible");
			let ReadResult::ActiveThreads { threads, next, .. } =
				api.force_sync_read(thread_read(next)).await.unwrap()
			else {
				panic!("second active thread page")
			};
			assert_eq!(threads[0].id, Id(19));
			assert_eq!(next, None);
			server.await.unwrap();
			assert!(!api.stopped());
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn active_threads_indexing_defers_and_enforces_the_route_cooldown() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let (api, server) = fixture(vec![(
				THREAD_ROUTE.into(),
				"202 Accepted",
				json!({"code":110000,"retry_after":1.5}),
			)])
			.await;
			let ReadResult::ActiveThreadsIndexing {
				guild,
				parent,
				retry_after,
			} = api.force_sync_read(thread_read(None)).await.unwrap()
			else {
				panic!("indexing outcome")
			};
			assert_eq!((guild, parent), (Id(2), Id(4)));
			assert_eq!(retry_after, Duration::from_millis(1500));
			let route = crate::rate_limits::Route::new(&Method::GET, THREAD_ROUTE);
			assert!(
				api.cooldown.lock().await.deadline(&route).unwrap() > tokio::time::Instant::now()
			);
			assert!(matches!(
				api.force_sync_read(thread_read(None)).await,
				Err(Failure::Busy)
			));
			assert!(!api.stopped());
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn active_thread_http_errors_and_oversized_pages_are_scoped() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let too_many: Vec<_> = (10..36).rev().map(|id| thread(id, 11)).collect();
			let mut wrong_guild = thread(30, 11);
			wrong_guild["guild_id"] = json!("3");
			let mut wrong_parent = thread(30, 11);
			wrong_parent["parent_id"] = json!("5");
			let (api, server) = fixture(vec![
				(THREAD_ROUTE.into(), "403 Forbidden", json!({"code":50001})),
				(
					THREAD_ROUTE.into(),
					"200 OK",
					json!({"threads":[wrong_guild],"has_more":false}),
				),
				(
					THREAD_ROUTE.into(),
					"200 OK",
					json!({"threads":[wrong_parent],"has_more":false}),
				),
				(
					THREAD_ROUTE.into(),
					"200 OK",
					json!({"threads":{},"has_more":false}),
				),
				(
					THREAD_ROUTE.into(),
					"200 OK",
					json!({"threads":too_many,"has_more":false}),
				),
				(
					THREAD_ROUTE.into(),
					"202 Accepted",
					json!({"code":110000,"retry_after":-1}),
				),
			])
			.await;
			assert!(matches!(
				api.force_sync_read(thread_read(None)).await,
				Err(Failure::Forbidden)
			));
			for _ in 0..5 {
				assert!(matches!(
					api.force_sync_read(thread_read(None)).await,
					Err(Failure::Protocol)
				));
			}
			assert!(!api.stopped());
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[test]
	fn active_thread_pages_reject_wrong_scope_order_archived_rows_and_stalled_cursors() {
		let parse = |value: Value, before: Option<Id>| {
			active_thread_page(&serde_json::to_vec(&value).unwrap(), Id(2), Id(4), before)
		};
		let mut wrong_guild = thread(30, 11);
		wrong_guild["guild_id"] = json!("3");
		let mut wrong_parent = thread(30, 11);
		wrong_parent["parent_id"] = json!("5");
		let mut archived = thread(30, 11);
		archived["thread_metadata"]["archived"] = json!(true);
		let mut hidden_wrong_parent = wrong_parent.clone();
		hidden_wrong_parent["flags"] = json!(1 << 17);
		for rows in [
			vec![wrong_guild],
			vec![wrong_parent],
			vec![hidden_wrong_parent],
			vec![thread(0, 11)],
			vec![thread(4, 11)],
			vec![thread(30, 0)],
			vec![archived],
			vec![thread(30, 11), thread(30, 12)],
			vec![thread(29, 11), thread(30, 12)],
		] {
			assert!(matches!(
				parse(json!({"threads":rows,"has_more":false}), None),
				Err(Failure::Protocol)
			));
		}
		assert!(matches!(
			parse(
				json!({"threads":[thread(30, 11)],"has_more":false}),
				Some(Id(30))
			),
			Err(Failure::Protocol)
		));
		assert!(matches!(
			parse(json!({"threads":[],"has_more":true}), None),
			Err(Failure::Protocol)
		));
		assert!(matches!(
			parse(json!({"threads":[]}), None),
			Err(Failure::Protocol)
		));
		assert!(matches!(
			parse(json!({"threads":"invalid","has_more":false}), None),
			Err(Failure::Protocol)
		));
		let mut hidden = thread(20, 12);
		hidden["flags"] = json!(1 << 17);
		let ReadResult::ActiveThreads { threads, next, .. } =
			parse(json!({"threads":[hidden],"has_more":true}), None).unwrap()
		else {
			panic!("hidden-only page")
		};
		assert!(threads.is_empty());
		assert_eq!(next, Some(Id(20)));
		let full: Vec<_> = (10..35).rev().map(|id| thread(id, 11)).collect();
		let ReadResult::ActiveThreads { threads, next, .. } =
			parse(json!({"threads":full,"has_more":true}), None).unwrap()
		else {
			panic!("full thread page")
		};
		assert_eq!(threads.len(), sync::THREAD_PAGE_SIZE);
		assert_eq!(next, Some(Id(10)));
	}
	fn message(id: Id, channel: Id) -> Value {
		json!({"id":id.to_string(),"channel_id":channel.to_string(),"content":"Cached body","author":{"id":"9","username":"author"}})
	}
	#[tokio::test]
	async fn force_sync_history_pages_more_than_five_hundred_without_opening_or_acknowledging() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let end_ms = sync::DISCORD_EPOCH_MS + sync::HISTORY_MILLIS + 10_000;
			let cutoff_ms = end_ms - sync::HISTORY_MILLIS;
			let end = sync::last_before(end_ms).unwrap();
			let mut before = end;
			let mut replies = Vec::new();
			for _ in 0..12 {
				let rows: Vec<_> = (1..=50)
					.map(|delta| message(Id(before.0 - delta), Id(4)))
					.collect();
				replies.push((
					format!("/channels/4/messages?limit=50&before={before}"),
					"200 OK",
					json!(rows),
				));
				before = Id(before.0 - 50);
			}
			let (api, server) = fixture(replies).await;
			let mut before = None;
			let mut count = 0;
			for _ in 0..12 {
				let ReadResult::History(page) = api
					.force_sync_read(Read::History {
						channel: Id(4),
						before,
						cutoff_ms,
						end_ms,
					})
					.await
					.unwrap()
				else {
					panic!("history result")
				};
				assert!(!page.boundary_reached);
				assert!(!page.channel_beginning);
				assert!(page.messages.windows(2).all(|rows| rows[0].id > rows[1].id));
				count += page.messages.len();
				before = page.oldest;
			}
			assert_eq!(count, 600);
			server.await.unwrap();
			assert!(!api.stopped());
		})
		.await
		.unwrap();
	}
	#[test]
	fn force_sync_history_validates_identity_dates_cursor_and_page_limits() {
		let cutoff = sync::DISCORD_EPOCH_MS + 1000;
		let end = cutoff + 1000;
		let id = |offset: u64| Id(offset << 22);
		let rows = json!([message(id(1000), Id(4)), message(id(999), Id(4))]);
		let page = history_page(
			&serde_json::to_vec(&rows).unwrap(),
			Id(4),
			id(2001),
			cutoff,
			end,
		)
		.unwrap();
		assert_eq!(page.messages.len(), 1);
		assert!(page.boundary_reached);
		assert!(page.channel_beginning);
		assert_eq!(page.oldest, Some(id(999)));
		assert!(
			history_page(
				&serde_json::to_vec(&rows).unwrap(),
				Id(5),
				id(2001),
				cutoff,
				end
			)
			.is_err()
		);
		assert!(
			history_page(
				&serde_json::to_vec(&rows).unwrap(),
				Id(4),
				id(1000),
				cutoff,
				end
			)
			.is_err()
		);
		let duplicate = json!([message(id(1000), Id(4)), message(id(1000), Id(4))]);
		assert!(
			history_page(
				&serde_json::to_vec(&duplicate).unwrap(),
				Id(4),
				id(2001),
				cutoff,
				end
			)
			.is_err()
		);
		let oversized: Vec<_> = (1..=51)
			.map(|offset| message(id(1000 + offset), Id(4)))
			.collect();
		assert!(matches!(
			history_page(
				&serde_json::to_vec(&oversized).unwrap(),
				Id(4),
				id(2001),
				cutoff,
				end
			),
			Err(Failure::ResponseLimit)
		));
	}
	#[tokio::test]
	async fn force_sync_busy_forbidden_and_wrong_identity_remain_scoped() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let (api, server) = fixture(vec![
				(
					"/guilds/2/channels".into(),
					"403 Forbidden",
					json!({"code":50001}),
				),
				(
					"/guilds/2/channels".into(),
					"200 OK",
					json!([{"id":"4","guild_id":"3","type":0,"name":"Wrong server"}]),
				),
			])
			.await;
			let permit = api.requests.acquire_many(4).await.unwrap();
			assert!(matches!(
				api.force_sync_read(Read::Channels {
					guild: Id(2),
					user: Id(1)
				})
				.await,
				Err(Failure::Busy)
			));
			drop(permit);
			assert!(matches!(
				api.force_sync_read(Read::Channels {
					guild: Id(2),
					user: Id(1)
				})
				.await,
				Err(Failure::Forbidden)
			));
			assert!(matches!(
				api.force_sync_read(Read::Channels {
					guild: Id(2),
					user: Id(1)
				})
				.await,
				Err(Failure::Protocol)
			));
			assert!(!api.stopped());
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn force_sync_production_read_apply_and_sqlite_do_not_replace_the_open_timeline() {
		tokio::time::timeout(Duration::from_secs(5), async {
			let end_ms = sync::DISCORD_EPOCH_MS + sync::HISTORY_MILLIS + 1000;
			let cutoff_ms = end_ms - sync::HISTORY_MILLIS;
			let upper = sync::last_before(end_ms).unwrap();
			let target = Id(upper.0 - 1);
			let (api, server) = fixture(vec![
				("/guilds/2?with_counts=true".into(), "200 OK", guild()),
				("/guilds/2/members/1".into(), "200 OK", member("1")),
				(
					format!("/channels/4/messages?limit=50&before={upper}"),
					"200 OK",
					json!([message(target, Id(4))]),
				),
			])
			.await;
			let mut state = client_core::State::default();
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::Ready {
					user: model::SavedAccount {
						id: Id(1),
						name: "Old account".into(),
						display: None,
						avatar: None,
						discriminator: 0,
						has_token: false,
					}
					.user(),
					guilds: vec![model::Guild {
						id: Id(2),
						name: "Old server".into(),
						icon: None,
						emojis: None,
						stickers: None,
						default_message_notifications: None,
					}],
					channels: vec![model::Channel {
						id: Id(4),
						guild: Some(Id(2)),
						parent_id: None,
						position: 0,
						name: "chat".into(),
						kind: 0,
						recipients: vec![],
						member_list_id: None,
						message_count: None,
						tags: None,
						icon: None,
						last_message: None,
					}],
					permissions: model::permissions::Snapshot {
						guilds: vec![model::permissions::Guild {
							id: Id(2),
							owner: Some(Id(1)),
							roles: None,
							member: None,
						}],
						channels: vec![],
					},
				},
			});
			state.select(Id(4));
			state.drafts.insert(Id(4), "Keep draft".into());
			let request = state.request;
			assert!(
				state.apply_force_sync_result(
					api.force_sync_read(Read::Guild {
						guild: Id(2),
						user: Id(1)
					})
					.await
					.unwrap()
				)
			);
			assert_eq!(state.guilds[0].name, "Refreshed");
			assert_eq!(
				state.display_member(Id(4), Id(1)).unwrap().nick.as_deref(),
				Some("Server nickname")
			);
			assert_eq!(
				state
					.display_member(Id(4), Id(1))
					.unwrap()
					.user
					.avatar_key(),
				"member-avatar-2-1-0123456789abcdef0123456789abcdef"
			);
			let ReadResult::History(page) = api
				.force_sync_read(Read::History {
					channel: Id(4),
					before: None,
					cutoff_ms,
					end_ms,
				})
				.await
				.unwrap()
			else {
				panic!("history")
			};
			let mut store =
				local_store::LocalStore::open(std::path::Path::new(":memory:")).unwrap();
			store
				.save_sync_checkpoint(
					Id(1),
					&local_store::SyncCheckpoint {
						job: 1,
						scope: Some(Id(2)),
						started_ms: end_ms,
						cutoff_ms,
						payload: Vec::new(),
					},
				)
				.unwrap();
			let coverage = local_store::HistoryCoverage {
				job: 1,
				request: 1,
				cutoff: Id(((cutoff_ms - sync::DISCORD_EPOCH_MS) as u64) << 22),
				upper: Id(upper.0 - 1),
				before: None,
				next_before: if page.boundary_reached {
					None
				} else {
					page.oldest
				},
				complete: page.boundary_reached,
				revision: store.history_revision(Id(1), Id(4)).unwrap(),
			};
			let outcome = store
				.save_sync_page(Id(1), Id(4), &page.messages, &coverage)
				.unwrap();
			assert_eq!(outcome.saved, 1);
			assert!(!outcome.stale);
			assert_eq!(store.load_channel(Id(1), Id(4)).unwrap()[0].id, target);
			assert!(
				store
					.history_coverage(Id(1), Id(4))
					.unwrap()
					.unwrap()
					.complete
			);
			assert_eq!(state.request, request);
			assert_eq!(state.selected, Some(Id(4)));
			assert_eq!(state.timeline.row_count(), 0);
			assert_eq!(state.drafts[&Id(4)], "Keep draft");
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
}
