//! Documented moderation routes. Writes are never retried after uncertain acceptance.
use crate::{DiscordApi, Failure};
use model::{Id, automod as a, moderation as m};
use reqwest::Method;
use serde_json::{Value, json};

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
	pub(super) async fn moderation(
		&self,
		guild: Id,
		action: &m::Action,
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
