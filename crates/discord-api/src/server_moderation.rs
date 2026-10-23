//! Documented moderation routes. Writes are never retried after uncertain acceptance.
use crate::{DiscordApi, Failure};
use client_core::server_admin::{REVIEW_AUDIT, REVIEW_MESSAGES, REVIEW_ROLES, ReviewProgress};
use model::{Id, automod as a, moderation as m};
use reqwest::Method;
use serde_json::{Value, json};
pub(super) type ReviewNotify =
	std::sync::Arc<dyn Fn(ReviewProgress) -> Result<(), Failure> + Send + Sync>;
#[derive(Clone, Copy)]
enum ReviewSection {
	Roles,
	Messages,
	Audit,
}
enum ReviewPart {
	Roles(Result<Vec<model::server_admin::Role>, Failure>),
	Messages(Result<client_core::search::Outcome, Failure>),
	Audit(Result<model::server_audit_log::Page, Failure>),
}

fn write_failure(failure: Failure) -> Failure {
	if matches!(
		failure,
		Failure::Capacity | Failure::Network | Failure::Protocol
	) {
		Failure::Ambiguous
	} else {
		failure
	}
}
fn reconcile_failure(failure: Failure) -> Failure {
	if failure.ends_session() {
		failure
	} else {
		Failure::Ambiguous
	}
}
fn rule_body(settings: &a::Settings) -> Value {
	let metadata = &settings.metadata;
	let trigger = match settings.trigger {
		1 | 6 => {
			json!({"keyword_filter":metadata.keywords,"regex_patterns":metadata.regex,"allow_list":metadata.allow})
		}
		4 => json!({"presets":metadata.presets,"allow_list":metadata.allow}),
		5 => {
			json!({"mention_total_limit":metadata.mentions,"mention_raid_protection_enabled":metadata.raid_protection})
		}
		_ => json!({}),
	};
	let actions: Vec<_> = settings
		.effects
		.iter()
		.map(|effect| match effect.kind {
			1 => json!({"type":1,"metadata":{"custom_message":effect.message}}),
			2 => json!({"type":2,"metadata":{"channel_id":effect.channel}}),
			3 => json!({"type":3,"metadata":{"duration_seconds":effect.duration}}),
			_ => json!({"type":effect.kind}),
		})
		.collect();
	json!({"name":settings.name,"event_type":settings.event,"trigger_type":settings.trigger,"trigger_metadata":trigger,"actions":actions,"enabled":settings.enabled,"exempt_roles":settings.exempt_roles,"exempt_channels":settings.exempt_channels})
}
fn edit_body(before: &a::Settings, after: &a::Settings, current: &Value) -> Result<Value, Failure> {
	let before = rule_body(before);
	let after = rule_body(after);
	let mut patch = serde_json::Map::new();
	for (key, value) in after.as_object().ok_or(Failure::Protocol)? {
		if matches!(key.as_str(), "trigger_type" | "event_type") || before.get(key) == Some(value) {
			continue;
		}
		if matches!(key.as_str(), "exempt_roles" | "exempt_channels") {
			let previous = before[key].as_array().ok_or(Failure::Protocol)?;
			let next = value.as_array().ok_or(Failure::Protocol)?;
			let mut current = current
				.get(key)
				.and_then(Value::as_array)
				.cloned()
				.ok_or(Failure::Protocol)?;
			current.retain(|id| !previous.contains(id) || next.contains(id));
			for id in next.iter().filter(|id| !previous.contains(id)) {
				if !current.contains(id) {
					current.push(id.clone());
				}
			}
			if current.len() > if key == "exempt_roles" { 20 } else { 50 } {
				return Err(Failure::ProtocolAt(
					"Exemptions changed; reload before editing",
				));
			}
			patch.insert(key.clone(), current.into());
		} else if key == "actions" {
			let mut effects = current
				.get(key)
				.and_then(Value::as_array)
				.cloned()
				.ok_or(Failure::Protocol)?;
			for previous in before[key].as_array().ok_or(Failure::Protocol)? {
				if !value
					.as_array()
					.ok_or(Failure::Protocol)?
					.iter()
					.any(|row| row["type"] == previous["type"])
				{
					effects.retain(|row| row["type"] != previous["type"]);
				}
			}
			for next in value.as_array().ok_or(Failure::Protocol)? {
				let previous = before[key]
					.as_array()
					.ok_or(Failure::Protocol)?
					.iter()
					.find(|row| row["type"] == next["type"]);
				if previous == Some(next) {
					continue;
				}
				if let Some(existing) = effects.iter_mut().find(|row| row["type"] == next["type"]) {
					if let (Some(previous), Some(metadata), Some(next_metadata)) = (
						previous,
						existing.get_mut("metadata").and_then(Value::as_object_mut),
						next.get("metadata").and_then(Value::as_object),
					) {
						for (field, new) in next_metadata {
							if previous["metadata"].get(field) != Some(new) {
								metadata.insert(field.clone(), new.clone());
							}
						}
					} else {
						*existing = next.clone();
					}
				} else {
					effects.push(next.clone());
				}
			}
			patch.insert(key.clone(), effects.into());
		} else if key == "trigger_metadata" {
			let mut metadata = current
				.get(key)
				.and_then(Value::as_object)
				.cloned()
				.ok_or(Failure::Protocol)?;
			for (field, value) in value.as_object().ok_or(Failure::Protocol)? {
				if before[key].get(field) != Some(value) {
					metadata.insert(field.clone(), value.clone());
				}
			}
			patch.insert(key.clone(), metadata.into());
		} else {
			patch.insert(key.clone(), value.clone());
		}
	}
	Ok(patch.into())
}
impl DiscordApi {
	async fn automod_rules(&self, guild: Id) -> Result<a::Snapshot, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/guilds/{guild}/auto-moderation/rules"),
				None,
				1024 * 1024,
			)
			.await?;
		discord_protocol::automod::rules(&bytes, guild).map_err(|_| Failure::Protocol)
	}
	pub(super) async fn automod(
		&self,
		guild: Id,
		action: &a::Action,
	) -> Result<a::Snapshot, Failure> {
		if guild.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		let base = format!("/guilds/{guild}/auto-moderation/rules");
		match action {
			a::Action::Load => return self.automod_rules(guild).await,
			a::Action::Create { settings, reason } => {
				let bytes = self
					.request_reason_limited(
						Method::POST,
						&base,
						Some(rule_body(settings)),
						512 * 1024,
						reason,
					)
					.await
					.map_err(write_failure)?;
				let created = discord_protocol::automod::rule(&bytes, guild)
					.map_err(|_| Failure::Ambiguous)?;
				if created.settings != **settings {
					return Err(Failure::Ambiguous);
				}
			}
			a::Action::Edit {
				id,
				before,
				after,
				reason,
			} => {
				let path = format!("{base}/{id}");
				let latest = self
					.request_limited(Method::GET, &path, None, 512 * 1024)
					.await?;
				let latest_rule = discord_protocol::automod::rule(&latest, guild)
					.map_err(|_| Failure::Protocol)?;
				if latest_rule.id != *id
					|| latest_rule.settings.trigger != before.trigger
					|| latest_rule.settings.event != before.event
				{
					return Err(Failure::ProtocolAt("Rule changed; reload before editing"));
				}
				let current: Value =
					serde_json::from_slice(&latest).map_err(|_| Failure::Protocol)?;
				let body = edit_body(before, after, &current)?;
				if body.as_object().is_none_or(|body| body.is_empty()) {
					return self.automod_rules(guild).await;
				}
				let bytes = self
					.request_reason_limited(Method::PATCH, &path, Some(body), 512 * 1024, reason)
					.await
					.map_err(write_failure)?;
				let edited = discord_protocol::automod::rule(&bytes, guild)
					.map_err(|_| Failure::Ambiguous)?;
				if edited.id != *id || !edited.settings.confirms(before, after) {
					return Err(Failure::Ambiguous);
				}
			}
			a::Action::Delete { id, reason } => {
				let bytes = self
					.request_reason_limited(
						Method::DELETE,
						&format!("{base}/{id}"),
						None,
						4096,
						reason,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
			}
		}
		self.automod_rules(guild).await.map_err(reconcile_failure)
	}
	pub fn set_member_review_notifier(
		&self,
		notify: impl Fn(ReviewProgress) -> Result<(), Failure> + Send + Sync + 'static,
	) -> Result<(), Failure> {
		*self.review_progress.lock().map_err(|_| Failure::Network)? =
			Some(std::sync::Arc::new(notify));
		Ok(())
	}
	fn publish_member_review(
		&self,
		guild: Id,
		request: Option<u64>,
		review: &m::Review,
		pending: u8,
	) -> Result<(), Failure> {
		let Some(request) = request else {
			return Ok(());
		};
		if !review.valid() {
			return Err(Failure::Capacity);
		}
		let notify = self
			.review_progress
			.lock()
			.map_err(|_| Failure::Network)?
			.clone();
		if let Some(notify) = notify {
			notify(ReviewProgress {
				guild,
				request,
				review: Box::new(review.clone()),
				pending,
			})?;
		}
		Ok(())
	}
	#[cfg(test)]
	pub(super) async fn moderation(
		&self,
		guild: Id,
		action: &m::Action,
	) -> Result<m::Outcome, Failure> {
		self.moderation_progress(guild, action, None).await
	}
	pub(super) async fn moderation_progress(
		&self,
		guild: Id,
		action: &m::Action,
		request: Option<u64>,
	) -> Result<m::Outcome, Failure> {
		if guild.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		match action {
			m::Action::Bans { after } => {
				let path = format!(
					"/guilds/{guild}/bans?limit={}{}",
					m::MAX_BANS,
					after.map_or(String::new(), |id| format!("&after={id}"))
				);
				let bytes = self
					.request_limited(Method::GET, &path, None, 512 * 1024)
					.await?;
				discord_protocol::moderation::bans(&bytes, guild, *after)
					.map(m::Outcome::Bans)
					.map_err(|_| Failure::Protocol)
			}
			m::Action::BanSearch { query } => {
				let encoded: String = query
					.trim()
					.bytes()
					.map(|byte| format!("%{byte:02X}"))
					.collect();
				let bytes = self
					.request_limited(
						Method::GET,
						&format!("/guilds/{guild}/bans/search?query={encoded}&limit=10"),
						None,
						128 * 1024,
					)
					.await?;
				let mut page = discord_protocol::moderation::bans(&bytes, guild, None)
					.map_err(|_| Failure::Protocol)?;
				if page.items.len() > 10 {
					return Err(Failure::Protocol);
				}
				page.next = None;
				Ok(m::Outcome::Bans(page))
			}
			m::Action::BulkBan {
				users,
				delete_seconds,
				reason,
			} => {
				let response = self
					.request_reason_limited(
						Method::POST,
						&format!("/guilds/{guild}/bulk-ban/v2"),
						Some(json!({"user_ids": users,"delete_message_seconds":delete_seconds})),
						4096,
						reason,
					)
					.await
					.map_err(write_failure)?;
				if !response.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(m::Outcome::Bulk(m::BulkReport::pending(users)))
			}
			m::Action::BulkRefresh { users } => {
				use futures_util::{StreamExt, stream};
				let rows = stream::iter(users.iter().copied())
					.map(|user| async move {
						let status = match self
							.request_optional_limited(&format!("/guilds/{guild}/bans/{user}"), 8192)
							.await
						{
							Ok(Some(bytes)) => {
								let value = serde_json::from_slice::<Value>(&bytes).ok();
								if value
									.as_ref()
									.and_then(|value| value["user"]["id"].as_str())
									.and_then(|id| id.parse::<Id>().ok())
									== Some(user)
								{
									m::BanStatus::Banned
								} else {
									// A bad target response cannot invalidate another target's readback.
									m::BanStatus::Unknown
								}
							}
							Ok(None) => m::BanStatus::NotBanned,
							Err(error) if error.ends_session() => return Err(error),
							Err(_) => m::BanStatus::Unknown,
						};
						Ok((user, status))
					})
					.buffer_unordered(2)
					.collect::<Vec<Result<_, Failure>>>()
					.await;
				let users = rows.into_iter().collect::<Result<Vec<_>, _>>()?;
				Ok(m::Outcome::Bulk(m::BulkReport {
					users,
					accepted: true,
				}))
			}
			m::Action::Review {
				user,
				channel,
				audit,
			} => {
				let member = self.admin_member(guild, *user).await?;
				let mut review = m::Review {
					guild,
					channel: *channel,
					member,
					roles: Vec::new(),
					messages: Vec::new(),
					audit: Vec::new(),
					audit_window: None,
					messages_error: None,
					audit_error: None,
				};
				let mut pending =
					REVIEW_ROLES | REVIEW_MESSAGES | if *audit { REVIEW_AUDIT } else { 0 };
				self.publish_member_review(guild, request, &review, pending)?;
				use futures_util::{StreamExt, stream};
				let mut sections = vec![ReviewSection::Roles, ReviewSection::Messages];
				if *audit {
					sections.push(ReviewSection::Audit);
				}
				let mut reads = stream::iter(sections)
					.map(|section| async move {
						match section {
							ReviewSection::Roles => ReviewPart::Roles(
								self.admin_metadata(guild).await.and_then(|value| {
									value
										.checked_roles()
										.map(|(roles, _)| roles)
										.map_err(|_| Failure::Protocol)
								}),
							),
							ReviewSection::Messages => ReviewPart::Messages(
								self.search(
									*channel,
									Some(guild),
									&format!("from:{user}"),
									None,
									0,
									false,
								)
								.await,
							),
							ReviewSection::Audit => ReviewPart::Audit(
								self.server_audit_log(
									guild,
									&model::server_audit_log::Query::default(),
								)
								.await,
							),
						}
					})
					.buffer_unordered(2);
				while let Some(part) = reads.next().await {
					match part {
						ReviewPart::Roles(result) => {
							pending &= !REVIEW_ROLES;
							match result {
								Ok(roles) => review.roles = roles,
								Err(error) if error.ends_session() => return Err(error),
								Err(_) => {}
							}
						}
						ReviewPart::Messages(result) => {
							pending &= !REVIEW_MESSAGES;
							match result {
								Ok(client_core::search::Outcome::Page(page)) => {
									review.messages = page
										.hits
										.into_iter()
										.filter(|hit| hit.author.id == *user)
										.take(25)
										.map(|hit| m::ReviewMessage {
											id: hit.id,
											channel: hit.channel,
											excerpt: hit.excerpt.chars().take(512).collect(),
										})
										.collect()
								}
								Ok(_) => {
									review.messages_error =
										Some("Message search is still indexing. Retry later.")
								}
								Err(error) if error.ends_session() => return Err(error),
								Err(error) => review.messages_error = Some(error.label()),
							}
						}
						ReviewPart::Audit(result) => {
							pending &= !REVIEW_AUDIT;
							match result {
								Ok(page) => {
									review.audit_window = page
										.entries
										.iter()
										.map(|entry| entry.id)
										.min()
										.zip(page.entries.iter().map(|entry| entry.id).max());
									let target = user.to_string();
									review.audit = page
										.entries
										.into_iter()
										.filter(|entry| {
											entry.target_id.as_deref() == Some(target.as_str())
										})
										.take(20)
										.map(|entry| m::ReviewAudit {
											id: entry.id,
											actor: entry.user_id,
											action: entry.action_type,
											reason: entry.reason,
										})
										.collect();
								}
								Err(error) if error.ends_session() => return Err(error),
								Err(error) => review.audit_error = Some(error.label()),
							}
						}
					}
					if pending != 0 {
						self.publish_member_review(guild, request, &review, pending)?;
					}
				}
				if !review.valid() {
					return Err(Failure::Capacity);
				}
				Ok(m::Outcome::Review(Box::new(review)))
			}
			m::Action::Member(user) => self
				.admin_member(guild, *user)
				.await
				.map(|member| m::Outcome::Member(Box::new(member))),
			m::Action::Unban { user, reason } => {
				let bytes = self
					.request_reason_limited(
						Method::DELETE,
						&format!("/guilds/{guild}/bans/{user}"),
						None,
						4096,
						reason,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(m::Outcome::Changed {
					user: *user,
					member: None,
				})
			}
			m::Action::Change {
				user,
				change,
				reason,
			} => {
				let (method, path, body) = match change {
					m::Change::Kick => (
						Method::DELETE,
						format!("/guilds/{guild}/members/{user}"),
						None,
					),
					m::Change::Ban { delete_seconds } => (
						Method::PUT,
						format!("/guilds/{guild}/bans/{user}"),
						Some(json!({"delete_message_seconds":delete_seconds})),
					),
					m::Change::Timeout { until } => {
						let until = until
							.map(discord_protocol::pins::format_cursor)
							.transpose()
							.map_err(|_| Failure::Protocol)?;
						(
							Method::PATCH,
							format!("/guilds/{guild}/members/{user}"),
							Some(json!({"communication_disabled_until":until})),
						)
					}
				};
				let bytes = self
					.request_reason_limited(method, &path, body, 64 * 1024, reason)
					.await
					.map_err(write_failure)?;
				if let m::Change::Timeout { until } = change {
					let member = discord_protocol::server_admin::member(&bytes, *user)
						.map_err(|_| Failure::Ambiguous)?;
					if member.timeout_until != until.map(|at| at / 1_000_000) {
						return Err(Failure::Ambiguous);
					}
					Ok(m::Outcome::Changed {
						user: *user,
						member: Some(Box::new(member)),
					})
				} else if bytes.is_empty() {
					Ok(m::Outcome::Changed {
						user: *user,
						member: None,
					})
				} else {
					Err(Failure::Ambiguous)
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[tokio::test]
	async fn moderation_transport_preserves_intent_and_does_not_retry_unknown_writes() {
		use client_core::auth::SessionSecret;
		use std::{sync::Arc, time::Duration};
		use tokio::{
			io::{AsyncReadExt, AsyncWriteExt},
			net::TcpListener,
		};
		tokio::time::timeout(Duration::from_secs(15), async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_MODERATION_TOKEN".into()).unwrap())).unwrap();
			api.base=format!("http://{}",listener.local_addr().unwrap());
			let rule=json!({"id":"10","guild_id":"2","creator_id":"1","name":"Synthetic","event_type":1,"trigger_type":1,"trigger_metadata":{"keyword_filter":["first"],"allow_list":[],"future_field":"preserve"},"actions":[{"type":1,"metadata":{}}],"enabled":false,"exempt_roles":[],"exempt_channels":[]});
			let definition=rule.clone();
			let server=tokio::spawn(async move {
				for step in 0..9 {
					let (mut socket,_)=listener.accept().await.unwrap();
					let mut bytes=Vec::new();
					let (end,length)=loop {
						let mut chunk=[0;1024];let n=socket.read(&mut chunk).await.unwrap();assert!(n>0);bytes.extend_from_slice(&chunk[..n]);assert!(bytes.len()<=64*1024);
						if let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
							let header=std::str::from_utf8(&bytes[..end]).unwrap();
							let length=header.lines().find_map(|line|{let (key,value)=line.split_once(':')?;key.eq_ignore_ascii_case("content-length").then(||value.trim().parse::<usize>().unwrap())}).unwrap_or(0);
							if bytes.len()>=end+4+length {break (end+4,length);}
						}
					};
					let headers=std::str::from_utf8(&bytes[..end]).unwrap();
					let body=if length==0 {Value::Null} else {serde_json::from_slice(&bytes[end..end+length]).unwrap()};
					let expected=["GET /guilds/2/bans?limit=100 ","PUT /guilds/2/bans/7 ","DELETE /guilds/2/bans/7 ","PATCH /guilds/2/members/7 ","GET /guilds/2/auto-moderation/rules ","GET /guilds/2/auto-moderation/rules/10 ","PATCH /guilds/2/auto-moderation/rules/10 ","GET /guilds/2/auto-moderation/rules ","DELETE /guilds/2/members/7 "];
					assert!(headers.starts_with(expected[step]),"unexpected route in step {step}");
					if [1,2,3,6,8].contains(&step) {let encoded=headers.lines().find_map(|line|{let (key,value)=line.split_once(':')?;key.eq_ignore_ascii_case("x-audit-log-reason").then(||value.trim())}).expect("audit reason header");let field=reqwest::Url::parse(&format!("http://127.0.0.1/?reason={encoded}")).unwrap();let decoded=field.query_pairs().next().unwrap().1.into_owned();assert_eq!(decoded,"Reviewed / reason");}
					if step==1 {assert_eq!(body,json!({"delete_message_seconds":0}));}
					if step==3 {assert_eq!(body,json!({"communication_disabled_until":null}));}
					if step==6 {assert_eq!(body,json!({"name":"Explicit rename"}));}
					if step==8 {drop(socket);break;}
					let mut edited=definition.clone();edited["name"]=json!("Explicit rename");
					let response=match step {
						0=>"[]".to_string(),1|2=>String::new(),3=>json!({"user":{"id":"7","username":"Synthetic"},"roles":[],"communication_disabled_until":null}).to_string(),
						4=>json!([definition.clone()]).to_string(),5=>definition.to_string(),6=>edited.to_string(),7=>json!([edited]).to_string(),_=>unreachable!()
					};
					let status=if response.is_empty(){"204 No Content"}else{"200 OK"};
					socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
				}
				assert!(tokio::time::timeout(Duration::from_millis(150),listener.accept()).await.is_err(),"an uncertain write must not be replayed");
			});
			assert!(matches!(api.moderation(Id(2),&m::Action::Bans {after:None}).await,Ok(m::Outcome::Bans(_))));
			let reason="Reviewed / reason".to_string();
			assert!(api.moderation(Id(2),&m::Action::Change {user:Id(7),change:m::Change::Ban {delete_seconds:0},reason:reason.clone()}).await.is_ok());
			assert!(api.moderation(Id(2),&m::Action::Unban {user:Id(7),reason:reason.clone()}).await.is_ok());
			assert!(api.moderation(Id(2),&m::Action::Change {user:Id(7),change:m::Change::Timeout {until:None},reason:reason.clone()}).await.is_ok());
			assert_eq!(api.automod(Id(2),&a::Action::Load).await.unwrap().rules.len(),1);
			let before=discord_protocol::automod::rule(&serde_json::to_vec(&rule).unwrap(),Id(2)).unwrap().settings;
			let mut after=before.clone();after.name="Explicit rename".into();
			assert!(api.automod(Id(2),&a::Action::Edit {id:Id(10),before:Box::new(before),after:Box::new(after),reason:reason.clone()}).await.is_ok());
			assert!(matches!(api.moderation(Id(2),&m::Action::Change {user:Id(7),change:m::Change::Kick,reason}).await,Err(Failure::Ambiguous)));
			server.await.unwrap();
		}).await.unwrap();
	}

	#[test]
	fn sparse_rule_edit_preserves_unknown_metadata_and_unrelated_changes() {
		let before = a::Settings {
			name: "Synthetic".into(),
			metadata: a::Metadata {
				keywords: vec!["first".into()],
				..Default::default()
			},
			..Default::default()
		};
		let mut after = before.clone();
		after.metadata.keywords = vec!["second".into()];
		let current = json!({"name":"Renamed elsewhere","trigger_metadata":{"keyword_filter":["first"],"allow_list":["concurrent allowance"],"future_field":{"retain":true}}});
		let body = edit_body(&before, &after, &current).unwrap();
		assert_eq!(
			body,
			json!({"trigger_metadata":{"keyword_filter":["second"],"allow_list":["concurrent allowance"],"future_field":{"retain":true}}})
		);
		after.name = "Explicit rename".into();
		assert_eq!(
			edit_body(&before, &after, &current).unwrap()["name"],
			"Explicit rename"
		);
		assert!(body.get("event_type").is_none() && body.get("trigger_type").is_none());
	}
}
#[cfg(test)]
mod member_review_tests {
	use super::*;
	use client_core::auth::SessionSecret;
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	async fn request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
		let mut bytes = Vec::new();
		loop {
			let mut chunk = [0; 4096];
			let n = socket.read(&mut chunk).await.unwrap();
			assert!(n > 0);
			bytes.extend_from_slice(&chunk[..n]);
			assert!(bytes.len() < 32768);
			if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
				let headers = std::str::from_utf8(&bytes[..end]).unwrap();
				let length = headers
					.lines()
					.find_map(|line| {
						let (key, value) = line.split_once(':')?;
						key.eq_ignore_ascii_case("content-length")
							.then(|| value.trim().parse::<usize>().unwrap())
					})
					.unwrap_or(0);
				if bytes.len() >= end + 4 + length {
					return (
						headers.to_owned(),
						if length == 0 {
							Value::Null
						} else {
							serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap()
						},
					);
				}
			}
		}
	}
	async fn respond(socket: &mut tokio::net::TcpStream, status: &str, body: &str) {
		socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
	}
	#[tokio::test]
	async fn bulk_v2_ack_is_pending_and_explicit_readback_does_not_resend_a_ban() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_BULK_REVIEW_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				let (mut socket, _) = listener.accept().await.unwrap();
				let (headers, body) = request(&mut socket).await;
				assert!(headers.starts_with("POST /guilds/2/bulk-ban/v2 "));
				assert_eq!(
					body,
					json!({"user_ids":["7","8"],"delete_message_seconds":0})
				);
				let encoded = headers
					.lines()
					.find_map(|line| {
						let (key, value) = line.split_once(':')?;
						key.eq_ignore_ascii_case("x-audit-log-reason")
							.then(|| value.trim())
					})
					.expect("bulk ban retains its audit reason header");
				let reason =
					reqwest::Url::parse(&format!("http://127.0.0.1/?reason={encoded}")).unwrap();
				assert_eq!(reason.query_pairs().next().unwrap().1, "Reviewed");
				respond(&mut socket, "204 No Content", "").await;
				for _ in 0..2 {
					let (mut socket, _) = listener.accept().await.unwrap();
					let (headers, body) = request(&mut socket).await;
					assert_eq!(body, Value::Null);
					if headers.starts_with("GET /guilds/2/bans/7 ") {
						respond(
							&mut socket,
							"200 OK",
							r#"{"user":{"id":"7","username":"Synthetic"},"reason":"Reviewed"}"#,
						)
						.await;
					} else {
						assert!(headers.starts_with("GET /guilds/2/bans/8 "));
						respond(&mut socket, "404 Not Found", r#"{"code":10026}"#).await;
					}
				}
				assert!(
					tokio::time::timeout(Duration::from_millis(80), listener.accept())
						.await
						.is_err()
				);
			});
			let m::Outcome::Bulk(report) = api
				.moderation(
					Id(2),
					&m::Action::BulkBan {
						users: vec![Id(7), Id(8)],
						delete_seconds: 0,
						reason: "Reviewed".into(),
					},
				)
				.await
				.unwrap()
			else {
				panic!()
			};
			assert!(
				report
					.users
					.iter()
					.all(|(_, status)| *status == m::BanStatus::Pending)
			);
			let m::Outcome::Bulk(report) = api
				.moderation(
					Id(2),
					&m::Action::BulkRefresh {
						users: vec![Id(7), Id(8)],
					},
				)
				.await
				.unwrap()
			else {
				panic!()
			};
			assert!(report.users.contains(&(Id(7), m::BanStatus::Banned)));
			assert!(report.users.contains(&(Id(8), m::BanStatus::NotBanned)));
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn bulk_readback_preserves_verified_targets_when_other_responses_are_invalid() {
		tokio::time::timeout(Duration::from_secs(15), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_BULK_RECONCILE_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				for _ in 0..3 {
					let (mut socket, _) = listener.accept().await.unwrap();
					let (headers, body) = request(&mut socket).await;
					assert_eq!(body, Value::Null);
					let response = if headers.starts_with("GET /guilds/2/bans/7 ") {
						r#"{"user":{"id":"7"}}"#
					} else if headers.starts_with("GET /guilds/2/bans/8 ") {
						"invalid JSON"
					} else {
						assert!(headers.starts_with("GET /guilds/2/bans/9 "));
						r#"{"user":{"id":"99"}}"#
					};
					respond(&mut socket, "200 OK", response).await;
				}
				assert!(
					tokio::time::timeout(Duration::from_millis(80), listener.accept())
						.await
						.is_err()
				);
			});
			let m::Outcome::Bulk(report) = api
				.moderation(
					Id(2),
					&m::Action::BulkRefresh {
						users: vec![Id(7), Id(8), Id(9)],
					},
				)
				.await
				.unwrap()
			else {
				panic!()
			};
			assert!(report.valid());
			assert!(report.users.contains(&(Id(7), m::BanStatus::Banned)));
			assert!(report.users.contains(&(Id(8), m::BanStatus::Unknown)));
			assert!(report.users.contains(&(Id(9), m::BanStatus::Unknown)));
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn member_review_publishes_identity_and_fast_sections_before_delayed_messages_with_two_read_limit()
	 {
		tokio::time::timeout(Duration::from_secs(15),async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_MEMBER_REVIEW_TOKEN".into()).unwrap())).unwrap();
			api.base=format!("http://{}",listener.local_addr().unwrap());
			let (progress_send,mut progress_receive)=tokio::sync::mpsc::channel(8);
			api.set_member_review_notifier(move |progress|progress_send.try_send(progress).map_err(|_|Failure::Capacity)).unwrap();
			let api=Arc::new(api);
			let (held_send,held_receive)=tokio::sync::oneshot::channel();
			let (release_send,release_receive)=tokio::sync::oneshot::channel();
			let server=tokio::spawn(async move {
				let (mut member,_)=listener.accept().await.unwrap();
				let (headers,body)=request(&mut member).await;assert_eq!(body,Value::Null);assert!(headers.starts_with("GET /guilds/2/members/7 "));
				respond(&mut member,"200 OK",r#"{"user":{"id":"7","username":"account_name","global_name":"Display"},"nick":"Server nickname","roles":[],"joined_at":"2025-01-01T00:00:00Z"}"#).await;
				let mut metadata=None;let mut messages=None;
				for _ in 0..2 {
					let (mut socket,_)=listener.accept().await.unwrap();let(headers,body)=request(&mut socket).await;assert_eq!(body,Value::Null);
					if headers.starts_with("GET /guilds/2 "){metadata=Some(socket);}else{assert!(headers.starts_with("GET /guilds/2/messages/search?"));assert!(headers.contains("author_id=%37"));messages=Some(socket);}
				}
				held_send.send(()).unwrap();
				assert!(tokio::time::timeout(Duration::from_millis(100),listener.accept()).await.is_err(),"third administrative HTTP read exceeded the shared ceiling");
				respond(metadata.as_mut().unwrap(),"200 OK",r#"{"id":"2","owner_id":"99","roles":[{"id":"2","name":"@everyone","permissions":"1024","position":0,"managed":false}],"features":[]}"#).await;
				for _ in 0..2 {
					let(mut socket,_)=listener.accept().await.unwrap();let(headers,body)=request(&mut socket).await;assert_eq!(body,Value::Null);
					if headers.starts_with("GET /guilds/2/audit-logs?limit=50 ") {respond(&mut socket,"200 OK",r#"{"audit_log_entries":[{"id":"99","user_id":"1","target_id":"7","action_type":24,"reason":"Reviewed"},{"id":"98","user_id":"1","target_id":"8","action_type":24}],"users":[]}"#).await;}
					else {assert!(headers.starts_with("GET /guilds/2/bans?limit=100 "));respond(&mut socket,"200 OK","[]").await;}
				}
				release_receive.await.unwrap();
				respond(messages.as_mut().unwrap(),"200 OK",r#"{"messages":[[{"id":"9","channel_id":"3","author":{"id":"7","username":"account_name"},"content":"Recent ||spoiler||"}]],"total_results":1}"#).await;
				assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err());
			});
			let operation={let api=api.clone();tokio::spawn(async move {api.execute(client_core::Command::ServerAdmin {guild:Id(2),request:44,action:Box::new(model::server_admin::Action::Moderation(m::Action::Review {user:Id(7),channel:Id(3),audit:true}))}).await})};
			let identity=progress_receive.recv().await.unwrap();
			assert_eq!(identity.request,44);assert_eq!(identity.pending,REVIEW_ROLES|REVIEW_MESSAGES|REVIEW_AUDIT);assert_eq!(identity.review.member.nick.as_deref(),Some("Server nickname"));assert!(identity.review.messages.is_empty()&&identity.review.audit.is_empty());
			held_receive.await.unwrap();
			let other={let api=api.clone();tokio::spawn(async move {api.execute(client_core::Command::ServerAdmin {guild:Id(2),request:45,action:Box::new(model::server_admin::Action::Moderation(m::Action::Bans {after:None}))}).await})};
			loop {let progress=progress_receive.recv().await.unwrap();if progress.pending==REVIEW_MESSAGES {assert_eq!(progress.review.audit_window,Some((Id(98),Id(99))));assert_eq!(progress.review.audit.len(),1);assert!(progress.review.messages.is_empty());break;}}
			release_send.send(()).unwrap();
			let client_core::Event::ServerAdmin(client_core::server_admin::Event {result:Ok(model::server_admin::Result::Moderation(m::Outcome::Review(review))),..})=operation.await.unwrap()else{panic!()};
			assert!(review.valid());assert_eq!(review.messages[0].id,Id(9));assert_eq!(review.audit[0].reason.as_deref(),Some("Reviewed"));
			assert!(matches!(other.await.unwrap(),client_core::Event::ServerAdmin(client_core::server_admin::Event {result:Ok(_),..})));
			server.await.unwrap();
		}).await.unwrap();
	}
}
