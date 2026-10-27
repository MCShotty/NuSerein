//! Unofficial normal-user interaction submission; HTTP acceptance is not bot completion.
use crate::{DiscordApi, Failure};
use client_core::interactions::{Data, Request};
use serde_json::{Value, json};

fn component(c: &model::Component, depth: usize) -> Result<Value, Failure> {
	if depth > 8 {
		return Err(Failure::Capacity);
	}
	let mut out = json!({"type":c.kind});
	if c.id != 0 {
		out["id"] = json!(c.id);
	}
	match c.kind {
		1 => {
			out["components"] = c
				.components
				.iter()
				.filter(|c| c.kind != 10)
				.map(|c| component(c, depth + 1))
				.collect::<Result<Vec<_>, _>>()?
				.into()
		}
		18 => {
			out["component"] =
				component(c.component.as_deref().ok_or(Failure::Protocol)?, depth + 1)?
		}
		3..=8 | 19 | 21..=23 => {
			out["custom_id"] = json!(c.custom_id.as_ref().ok_or(Failure::Protocol)?);
			match c.kind {
				4 => out["value"] = json!(c.value.as_deref().unwrap_or_default()),
				21 => out["value"] = json!(c.value),
				23 => out["value"] = json!(c.checked.unwrap_or(false)),
				_ => out["values"] = json!(c.values),
			}
		}
		_ => return Err(Failure::Protocol),
	}
	Ok(out)
}

pub(crate) fn valid_uploads(request: &Request, count: usize) -> bool {
	fn walk(
		c: &model::Component,
		count: usize,
		seen: &mut std::collections::BTreeSet<usize>,
	) -> bool {
		if c.kind == 19
			&& !c.values.iter().all(|v| {
				v.parse::<usize>()
					.ok()
					.is_some_and(|i| i < count && i.to_string() == *v && seen.insert(i))
			}) {
			return false;
		}
		c.components
			.iter()
			.chain(c.component.as_deref())
			.all(|c| walk(c, count, seen))
	}
	let mut seen = std::collections::BTreeSet::new();
	match &request.data {
		Data::ApplicationCommand { invocation } => invocation.attachment_count() == Some(count),
		Data::Modal { components, .. } => {
			components.iter().all(|c| walk(c, count, &mut seen)) && seen.len() == count
		}
		_ => count == 0,
	}
}

fn catalog_failure(failure: Failure) -> Failure {
	if matches!(failure, Failure::Capacity | Failure::ResponseLimit) {
		Failure::ProtocolAt("Application commands exceed the catalog limit")
	} else {
		failure
	}
}

impl DiscordApi {
	pub(crate) async fn application_commands(
		&self,
		channel: model::Id,
		guild: Option<model::Id>,
		context: u8,
	) -> Result<Vec<model::application_commands::Command>, Failure> {
		if channel.0 == 0 || guild.is_some_and(|id| id.0 == 0) {
			return Err(Failure::Protocol);
		}
		if context > 2 || (guild.is_some() != (context == 0)) {
			return Err(Failure::Protocol);
		}
		let mut commands = Vec::new();
		if context != 2 {
			let path = guild.map_or_else(
				|| format!("/channels/{channel}/application-command-index"),
				|guild| format!("/guilds/{guild}/application-command-index"),
			);
			let bytes = self
				.request_limited(
					reqwest::Method::GET,
					&path,
					None,
					discord_protocol::MAX_WIRE,
				)
				.await
				.map_err(catalog_failure)?;
			commands =
				discord_protocol::application_commands::decode_context(&bytes, guild, context)
					.map_err(|_| {
						Failure::ProtocolAt(
							"Application commands are unsupported or exceed the catalog limit",
						)
					})?;
		}
		let user_index = self
			.request_optional_limited(
				"/users/@me/application-command-index",
				discord_protocol::MAX_WIRE,
			)
			.await;
		let bytes = match user_index {
			Ok(bytes) => bytes,
			Err(Failure::Forbidden) if !commands.is_empty() => None,
			Err(failure) => return Err(catalog_failure(failure)),
		};
		if let Some(bytes) = bytes {
			let installed =
				discord_protocol::application_commands::decode_context(&bytes, guild, context)
					.map_err(|_| {
						Failure::ProtocolAt("Account command list is invalid or exceeds its limits")
					})?;
			for command in installed.into_iter().filter(|command| {
				command.guild_id.is_none()
					&& command
						.integration_types
						.as_ref()
						.is_some_and(|kinds| kinds.contains(&1))
			}) {
				if let Some(existing) = commands.iter().find(|existing| existing.id == command.id) {
					if existing.version != command.version {
						return Err(Failure::ProtocolAt(
							"Command version changed; refresh the command list",
						));
					}
					continue;
				}
				commands.push(command);
				if !model::application_commands::valid_catalog(&commands) {
					return Err(Failure::ProtocolAt(
						"Application commands exceed the catalog limit",
					));
				}
			}
		}
		commands.shrink_to_fit();
		Ok(commands)
	}

	pub fn interaction_session(
		&self,
		session: Option<zeroize::Zeroizing<String>>,
	) -> Result<(), Failure> {
		*self
			.interaction_session
			.lock()
			.map_err(|_| Failure::Protocol)? = session;
		Ok(())
	}
	pub(crate) async fn interaction(
		&self,
		request: &Request,
		attachments: Option<Vec<Value>>,
	) -> Result<(), Failure> {
		if !request.valid() || !valid_uploads(request, attachments.as_ref().map_or(0, Vec::len)) {
			return Err(Failure::ProtocolAt("Invalid interaction; nothing was sent"));
		}
		let session = self
			.interaction_session
			.lock()
			.map_err(|_| Failure::Protocol)?
			.clone()
			.ok_or(Failure::ProtocolAt(
				"Interaction unavailable while reconnecting",
			))?;
		let (kind, mut data) = match &request.data {
			Data::Autocomplete { query } => {
				let mut options =
					serde_json::to_value(query.options().map_err(|_| Failure::Protocol)?)
						.map_err(|_| Failure::Protocol)?;
				let mut selected = &mut options;
				for _ in &query.path {
					selected = &mut selected[0]["options"];
				}
				let focused = selected
					.as_array_mut()
					.ok_or(Failure::Protocol)?
					.iter_mut()
					.find(|option| option["name"] == query.focused)
					.ok_or(Failure::Protocol)?;
				focused["focused"] = json!(true);
				let command = &query.command;
				let mut data = json!({"type":1,"id":command.id,"version":command.version,"name":command.name,"application_command":command,"options":options,"attachments":[]});
				if let Some(guild) = command.guild_id {
					data["guild_id"] = json!(guild);
				}
				(4, data)
			}

			Data::ApplicationCommand { invocation } => {
				let command = &invocation.command;
				let mut data = json!({"type":command.kind,"id":command.id,"version":command.version,
					"name":command.name,"application_command":command,"options":invocation.options,
					"attachments":[]});
				if let Some(target) = invocation.target {
					data["target_id"] = json!(target);
				}
				if let Some(guild) = command.guild_id {
					data["guild_id"] = json!(guild);
				}
				(2, data)
			}
			Data::Component {
				custom_id,
				component_type,
				values,
			} => {
				let mut data = json!({"custom_id":custom_id,"component_type":component_type});
				if *component_type != 2 {
					data["values"] = json!(values);
				}
				(3, data)
			}
			Data::Modal {
				id,
				custom_id,
				components,
			} => {
				let components = components
					.iter()
					.filter(|c| c.kind != 10)
					.map(|c| component(c, 0))
					.collect::<Result<Vec<_>, _>>()?;
				(
					5,
					json!({"id":id,"custom_id":custom_id,"components":components}),
				)
			}
		};
		if let Some(attachments) = attachments {
			if !matches!(kind, 2 | 5) {
				return Err(Failure::Protocol);
			}
			data["attachments"] = attachments.into();
		}
		let mut body = json!({"type":kind,"application_id":request.application_id,"channel_id":request.channel_id,"session_id":session.as_str(),"nonce":request.nonce,"data":data});
		if let Some(guild) = request.guild_id {
			body["guild_id"] = json!(guild);
		}
		if kind == 3 {
			body["message_id"] = json!(request.message_id.ok_or(Failure::Protocol)?);
			body["message_flags"] = json!(request.message_flags);
		}
		if serde_json::to_vec(&body)
			.map_err(|_| Failure::Protocol)?
			.len() > 256 * 1024
		{
			return Err(Failure::ProtocolAt(
				"Interaction is too large; nothing was sent",
			));
		}
		self.request_limited(
			reqwest::Method::POST,
			"/interactions",
			Some(body),
			64 * 1024,
		)
		.await
		.map(|_| ())
		.map_err(|failure| {
			if kind != 4 && failure == Failure::Network {
				Failure::Ambiguous
			} else {
				failure
			}
		})
	}
}

pub(crate) fn valid_file_types(request: &Request, sources: &[crate::upload::Source]) -> bool {
	fn valid(component: &model::Component, sources: &[crate::upload::Source]) -> bool {
		(component.kind != 19
			|| component.values.iter().all(|value| {
				value
					.parse::<usize>()
					.ok()
					.and_then(|i| sources.get(i))
					.is_some_and(|source| component.accepts_file(source.filename()))
			})) && component
			.components
			.iter()
			.chain(component.component.as_deref())
			.all(|c| valid(c, sources))
	}
	match &request.data {
		Data::ApplicationCommand { invocation } => {
			invocation.attachment_count() == Some(sources.len())
		}
		Data::Modal { components, .. } => components.iter().all(|c| valid(c, sources)),
		_ => sources.is_empty(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[tokio::test]
	async fn slash_catalog_and_submission_use_typed_bounded_account_transport() {
		use client_core::{Command, Event, auth::SessionSecret};
		use model::{
			Id,
			application_commands::{Argument, Invocation, Value as ArgumentValue},
		};
		use std::{sync::Arc, time::Duration};
		use tokio::{
			io::{AsyncReadExt, AsyncWriteExt},
			net::TcpListener,
		};
		tokio::time::timeout(Duration::from_secs(10), async {
			let definition = json!({"type":1,"id":"10","version":"11","application_id":"12",
				"name":"inspect","description":"Inspect a synthetic member","contexts":[0],
				"default_member_permissions":"0",
				"permissions":{"user":true,"roles":{"20":false,"55":true},"channels":{"19":false,"21":true}},
				"options":[{"type":2,"name":"utility","description":"Tools","options":[
					{"type":1,"name":"member","description":"Member","options":[
						{"type":3,"name":"label","description":"Label","required":true,"max_length":20},
						{"type":5,"name":"private","description":"Private"},
						{"type":6,"name":"user","description":"User"}]}]}]});
			let index =
				json!({"application_commands":[definition,{"type":0},{"type":1,"nsfw":true},
				{"type":1,"contexts":[1]}, {"type":1,"guild_id":"99"}],
				"applications":[{"id":"12","name":"Synthetic App","icon":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
					"permissions":{"user":false,"roles":{"20":true},"channels":{"19":true}}}]})
				.to_string();
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_SLASH_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			api.interaction_session(Some(zeroize::Zeroizing::new("synthetic-session".into())))
				.unwrap();
			let server =
				tokio::spawn(async move {
					for step in 0..4 {
						let (mut socket, _) = listener.accept().await.unwrap();
						let mut bytes = Vec::new();
						let (header_end, length) = loop {
							let mut chunk = [0; 1024];
							let read = socket.read(&mut chunk).await.unwrap();
							assert!(read > 0);
							bytes.extend_from_slice(&chunk[..read]);
							assert!(bytes.len() <= 16 * 1024);
							if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
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
									break (end + 4, length);
								}
							}
						};
						let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
						assert!(headers.contains("SYNTHETIC_SLASH_TOKEN"));
						if step == 3 {
							assert!(headers.starts_with(
								"GET /guilds/20/application-command-index HTTP/1.1\r\n"
							));
							socket
								.write_all(
									format!(
										"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
										discord_protocol::MAX_WIRE + 1
									)
									.as_bytes(),
								)
								.await
								.unwrap();
							continue;
						}
						let (status, response) = if step == 0 {
							assert!(headers.starts_with(
								"GET /guilds/20/application-command-index HTTP/1.1\r\n"
							));
							("200 OK", index.as_str())
						} else if step == 1 {
							assert!(headers.starts_with(
								"GET /users/@me/application-command-index HTTP/1.1\r\n"
							));
							("200 OK", "{\"application_commands\":[]}")
						} else {
							assert!(headers.starts_with("POST /interactions HTTP/1.1\r\n"));
							let body: Value =
								serde_json::from_slice(&bytes[header_end..header_end + length])
									.unwrap();
							assert_eq!(body["type"], 2);
							assert_eq!(body["application_id"], "12");
							assert_eq!(body["guild_id"], "20");
							assert_eq!(body["channel_id"], "21");
							assert_eq!(body["session_id"], "synthetic-session");
							assert_eq!(body["nonce"], "123");
							assert_eq!(body["data"]["type"], 1);
							assert_eq!(body["data"]["id"], "10");
							assert_eq!(body["data"]["version"], "11");
							assert_eq!(body["data"]["name"], "inspect");
							assert!(body["data"].get("guild_id").is_none());
							assert!(body.get("message_id").is_none());
							assert!(
								body["data"]["application_command"]
									.get("application_name")
									.is_none()
							);
							assert!(
								body["data"]["application_command"]
									.get("application_icon")
									.is_none()
							);
							for metadata in [
								"default_member_permissions",
								"permissions",
								"application_permissions",
							] {
								assert!(
									body["data"]["application_command"].get(metadata).is_none()
								);
							}
							assert_eq!(body["data"]["attachments"], json!([]));
							assert_eq!(
								body["data"]["options"],
								json!([{"type":2,"name":"utility","options":[
							{"type":1,"name":"member","options":[{"type":3,"name":"label","value":"Hello"},
							{"type":5,"name":"private","value":false},{"type":6,"name":"user","value":"55"}]}]}])
							);
							("204 No Content", "")
						};
						socket
							.write_all(
								format!(
									"HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
									response.len()
								)
								.as_bytes(),
							)
							.await
							.unwrap();
					}
				});
			let Event::ApplicationCommands {
				channel: Id(21),
				request: 7,
				result: Ok(mut commands),
			} = api.execute(Command::ApplicationCommands {
				context: 0,
				channel: Id(21),
				guild: Some(Id(20)),
				request: 7,
			})
			.await
			else {
				panic!("catalog failed")
			};
			assert_eq!(commands.len(), 1);
			let command = commands.remove(0);
			assert_eq!(command.application_name, "Synthetic App");
			assert_eq!(
				command.application_icon.as_deref(),
				Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
			);
			assert_eq!(command.default_member_permissions, Some(0));
			assert_eq!(command.permissions.user, Some(true));
			assert_eq!(command.permissions.roles.get(&Id(20)), Some(&false));
			assert_eq!(command.permissions.roles.get(&Id(55)), Some(&true));
			assert_eq!(command.permissions.channels.get(&Id(19)), Some(&false));
			assert_eq!(command.permissions.channels.get(&Id(21)), Some(&true));
			assert_eq!(command.application_permissions.user, Some(false));
			assert_eq!(
				command.application_permissions.roles.get(&Id(20)),
				Some(&true)
			);
			assert_eq!(
				command.application_permissions.channels.get(&Id(19)),
				Some(&true)
			);
			let mut no_permissions = command.clone();
			no_permissions.permissions = Default::default();
			no_permissions.application_permissions = Default::default();
			assert!(command.bytes() > no_permissions.bytes());
			let value = |kind, name: &str, value| Argument {
				kind,
				name: name.into(),
				value: Some(value),
				options: vec![],
			};
			let invocation = Invocation {
				target: None,
				command,
				options: vec![Argument {
					kind: 2,
					name: "utility".into(),
					value: None,
					options: vec![Argument {
						kind: 1,
						name: "member".into(),
						value: None,
						options: vec![
							value(3, "label", ArgumentValue::String("Hello".into())),
							value(5, "private", ArgumentValue::Boolean(false)),
							value(6, "user", ArgumentValue::String("55".into())),
						],
					}],
				}],
			};
			let request = Request {
				request: 8,
				nonce: "123".into(),
				application_id: Id(12),
				channel_id: Id(21),
				guild_id: Some(Id(20)),
				message_id: None,
				message_flags: 0,
				data: Data::ApplicationCommand {
					invocation: Box::new(invocation),
				},
			};
			assert!(api.interaction(&request, None).await.is_ok());
			assert!(matches!(
				api.application_commands(Id(21), Some(Id(20)), 0).await,
				Err(Failure::ProtocolAt(
					"Application commands exceed the catalog limit"
				))
			));
			assert!(!api.stopped());
			server.await.unwrap();
			let invalid_icon = serde_json::to_vec(&json!({"application_commands":[{
				"id":"10","version":"11","application_id":"12","type":1,"name":"ping","description":"Ping"}],
				"applications":[{"id":"12","name":"Synthetic App","icon":"../other"}]}))
			.unwrap();
			assert!(
				discord_protocol::application_commands::decode(&invalid_icon, Some(Id(20)))
					.unwrap()[0]
					.application_icon
					.is_none()
			);
			let mut permission_index: Value = serde_json::from_slice(&invalid_icon).unwrap();
			let decode_permissions = |index: &Value| {
				discord_protocol::application_commands::decode(
					&serde_json::to_vec(index).unwrap(),
					Some(Id(20)),
				)
			};
			let bare = decode_permissions(&permission_index).unwrap().remove(0);
			assert_eq!(bare.default_member_permissions, None);
			assert_eq!(bare.permissions, Default::default());
			assert_eq!(bare.application_permissions, Default::default());
			for bits in [
				Value::Null,
				json!(0),
				json!("0"),
				json!(u128::MAX.to_string()),
			] {
				permission_index["application_commands"][0]["default_member_permissions"] = bits;
				assert!(decode_permissions(&permission_index).is_ok());
			}
			for bits in [
				json!(-1),
				json!(""),
				json!("-1"),
				json!(format!("{}0", u128::MAX)),
			] {
				permission_index["application_commands"][0]["default_member_permissions"] = bits;
				assert!(decode_permissions(&permission_index).is_err());
			}
			permission_index["application_commands"][0]["default_member_permissions"] = Value::Null;
			let roles: serde_json::Map<_, _> =
				(1..=100).map(|id| (id.to_string(), json!(true))).collect();
			for application_layer in [false, true] {
				let (rows, other) = if application_layer {
					("applications", "application_commands")
				} else {
					("application_commands", "applications")
				};
				permission_index[other][0]["permissions"] = Value::Null;
				permission_index[rows][0]["permissions"] = json!({"roles":roles});
				assert!(decode_permissions(&permission_index).is_ok());
				permission_index[rows][0]["permissions"]["roles"]["101"] = json!(true);
				assert!(decode_permissions(&permission_index).is_err());
				permission_index[rows][0]["permissions"] = json!({"roles":roles});
				permission_index[rows][0]["permissions"]["user"] = json!(false);
				assert!(decode_permissions(&permission_index).is_err());
				permission_index[rows][0]["permissions"] = json!({"roles":{"0":true}});
				assert!(decode_permissions(&permission_index).is_err());
				permission_index[rows][0]["permissions"] = json!({"roles":{"20":true}});
				let duplicate = serde_json::to_string(&permission_index)
					.unwrap()
					.replace("\"20\":true", "\"20\":true,\"20\":false");
				assert!(
					discord_protocol::application_commands::decode(
						duplicate.as_bytes(),
						Some(Id(20))
					)
					.is_err()
				);
			}
			permission_index["applications"][0]["permissions"] = json!({"roles":roles});
			permission_index["application_commands"][0]["permissions"] = Value::Null;
			let repeated = (100..900)
				.map(|id| {
					let mut command = permission_index["application_commands"][0].clone();
					command["id"] = json!(id.to_string());
					command
				})
				.collect::<Vec<_>>();
			permission_index["application_commands"] = json!(repeated);
			assert!(decode_permissions(&permission_index).is_err());
			assert!(
				discord_protocol::application_commands::decode(
					&vec![b' '; discord_protocol::MAX_WIRE + 1],
					Some(Id(20))
				)
				.is_err()
			);
			assert!(
				discord_protocol::application_commands::decode(
					&serde_json::to_vec(&json!({
				"application_commands":vec![json!({"type":2});2001]}))
					.unwrap(),
					Some(Id(20))
				)
				.is_err()
			);
		})
		.await
		.unwrap();
	}
	#[test]
	fn modal_submission_projects_input_values_without_schema_metadata() {
		let input = model::Component {
			kind: 23,
			custom_id: Some("agree".into()),
			checked: Some(true),
			label: Some("Terms".into()),
			..Default::default()
		};
		let label = model::Component {
			kind: 18,
			component: Some(Box::new(input)),
			..Default::default()
		};
		assert_eq!(
			component(&label, 0).unwrap(),
			json!({"type":18,"component":{"type":23,"custom_id":"agree","value":true}})
		);
		assert!(
			component(
				&model::Component {
					kind: 255,
					..Default::default()
				},
				0
			)
			.is_err()
		);
	}
}

#[cfg(test)]
mod parity_tests {
	use super::*;
	use model::{Id, application_commands as a};
	#[tokio::test]
	async fn account_context_and_autocomplete_use_distinct_typed_requests() {
		use client_core::auth::SessionSecret;
		use std::{sync::Arc, time::Duration};
		use tokio::{
			io::{AsyncReadExt, AsyncWriteExt},
			net::TcpListener,
		};
		tokio::time::timeout(Duration::from_secs(10),async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_COMMAND_PARITY_TOKEN".into()).unwrap())).unwrap();
			api.base=format!("http://{}",listener.local_addr().unwrap());api.interaction_session(Some(zeroize::Zeroizing::new("synthetic-session".into()))).unwrap();
			let server=tokio::spawn(async move {
				for step in 0..5 {
					let (mut socket,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();
					let (end,length)=loop {let mut chunk=[0;1024];let n=socket.read(&mut chunk).await.unwrap();assert!(n>0);bytes.extend_from_slice(&chunk[..n]);assert!(bytes.len()<=32*1024);
						if let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {let header=std::str::from_utf8(&bytes[..end]).unwrap();let length=header.lines().find_map(|line|{let(key,value)=line.split_once(':')?;key.eq_ignore_ascii_case("content-length").then(||value.trim().parse::<usize>().unwrap())}).unwrap_or(0);if bytes.len()>=end+4+length {break(end+4,length);}}
					};
					let headers=std::str::from_utf8(&bytes[..end]).unwrap();
					if step==4 {assert!(headers.starts_with("POST /interactions "));drop(socket);assert!(tokio::time::timeout(Duration::from_millis(100),listener.accept()).await.is_err(),"an uncertain command must never be repeated");continue;}
					let response=if step==0 {assert!(headers.starts_with("GET /users/@me/application-command-index "));json!({"application_commands":[{"type":2,"id":"20","version":"21","application_id":"12","name":"Inspect member","contexts":[2],"integration_types":[1]}]}).to_string()} else {
						assert!(headers.starts_with("POST /interactions "));let body:Value=serde_json::from_slice(&bytes[end..end+length]).unwrap();assert!(body.get("guild_id").is_none());assert_eq!(body["channel_id"],"2");
						if step==1 || step==2 {assert_eq!(body["type"],2);assert_eq!(body["data"]["type"],if step==1 {2} else {3});assert_eq!(body["data"]["target_id"],if step==1 {"7"} else {"8"});assert!(body.get("message_id").is_none());}
						else {assert_eq!(body["type"],4);assert_eq!(body["data"]["type"],1);assert_eq!(body["data"]["options"][0]["value"],"2x");assert_eq!(body["data"]["options"][0]["focused"],true);assert!(body["data"].get("target_id").is_none());}String::new()
					};
					let status=if response.is_empty(){"204 No Content"}else{"200 OK"};socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
				}
			});
			let mut commands=api.application_commands(Id(2),None,2).await.unwrap();let mut command=commands.remove(0);assert!(command.valid());
			let request=Request {request:1,nonce:"123".into(),application_id:Id(12),channel_id:Id(2),guild_id:None,message_id:None,message_flags:0,data:Data::ApplicationCommand {invocation:Box::new(a::Invocation {command:command.clone(),options:Vec::new(),target:Some(Id(7))})}};
			assert!(api.interaction(&request,None).await.is_ok());
			command.kind=3;command.name="Inspect message".into();
			let message_request=Request {data:Data::ApplicationCommand {invocation:Box::new(a::Invocation {command:command.clone(),options:Vec::new(),target:Some(Id(8))})},nonce:"125".into(),..request};
			assert!(api.interaction(&message_request,None).await.is_ok());
			command.kind=1;command.name="count".into();command.description="Count".into();command.options=vec![a::CommandOption {kind:4,name:"count".into(),description:"Count".into(),autocomplete:true,..Default::default()}];
			let request=Request {request:2,nonce:"124".into(),application_id:Id(12),channel_id:Id(2),guild_id:None,message_id:None,message_flags:0,data:Data::Autocomplete {query:Box::new(a::Completion {command,path:Vec::new(),values:vec![("count".into(),"2x".into())],focused:"count".into()})}};
			assert!(api.interaction(&request,None).await.is_ok());assert_eq!(api.interaction(&message_request,None).await,Err(Failure::Ambiguous));server.await.unwrap();
		}).await.unwrap();
	}
}
