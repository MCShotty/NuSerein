use crate::{DiscordApi, Failure};
use client_core::group_actions::Action;
use model::{ChannelPatch, Patch};
use reqwest::Method;

fn members_result(
	bytes: &[u8],
	channel: Option<model::Id>,
) -> Result<client_core::group_actions::Members, Failure> {
	let dto: discord_protocol::group_actions::MembersDto =
		discord_protocol::decode(bytes).map_err(|_| Failure::Protocol)?;
	if dto.id.0 == 0
		|| channel.is_some_and(|expected| expected != dto.id)
		|| dto.kind != 3
		|| dto.guild_id.is_some()
		|| dto.owner_id.0 == 0
		|| dto.recipients.len() > 9
	{
		return Err(Failure::Protocol);
	}
	Ok(client_core::group_actions::Members {
		channel: dto.id,
		owner: dto.owner_id,
		recipients: dto
			.recipients
			.into_iter()
			.map(discord_protocol::UserDto::into_model)
			.collect(),
	})
}
fn write_failure(failure: Failure) -> Failure {
	if matches!(failure, Failure::Capacity | Failure::Protocol) {
		Failure::Ambiguous
	} else {
		failure
	}
}

impl DiscordApi {
	pub(super) async fn create_group(
		&self,
		users: Vec<model::Id>,
	) -> Result<(Box<model::Channel>, client_core::group_actions::Members), Failure> {
		let action = Action::Create(users);
		if !action.valid() {
			return Err(Failure::Protocol);
		}
		let Action::Create(users) = action else {
			unreachable!()
		};
		let bytes = self
			.request_limited(
				Method::POST,
				"/users/@me/channels",
				Some(
					serde_json::json!({"recipients":users.iter().map(ToString::to_string).collect::<Vec<_>>()}),
				),
				32 * 1024,
			)
			.await
			.map_err(write_failure)?;
		let members = members_result(&bytes, None).map_err(write_failure)?;
		if members.recipients.len() != users.len()
			|| members
				.recipients
				.iter()
				.any(|user| !users.contains(&user.id))
		{
			return Err(Failure::Ambiguous);
		}
		let dto: discord_protocol::ChannelDto =
			discord_protocol::decode(&bytes).map_err(|_| Failure::Ambiguous)?;
		Ok((Box::new(dto.into_model()), members))
	}
	pub(super) async fn transfer_group_owner(
		&self,
		channel: model::Id,
		user: model::Id,
	) -> Result<client_core::group_actions::Members, Failure> {
		if channel.0 == 0 || user.0 == 0 {
			return Err(Failure::Protocol);
		}
		// Unofficial normal-user field, observed in discord.py-self GroupChannel.edit.
		let bytes = self
			.request_limited(
				Method::PATCH,
				&format!("/channels/{channel}"),
				Some(serde_json::json!({"owner":user.to_string()})),
				32 * 1024,
			)
			.await
			.map_err(write_failure)?;
		let members = members_result(&bytes, Some(channel)).map_err(write_failure)?;
		if members.owner != user {
			return Err(Failure::Ambiguous);
		}
		Ok(members)
	}
	pub(super) async fn group_members(
		&self,
		channel: model::Id,
	) -> Result<client_core::group_actions::Members, Failure> {
		if channel.0 == 0 {
			return Err(Failure::Protocol);
		}
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/channels/{channel}"),
				None,
				32 * 1024,
			)
			.await?;
		members_result(&bytes, Some(channel))
	}
	// Documented channel routes; ordinary-user session interoperability remains unverified.
	pub(super) async fn group_action(
		&self,
		action: Action,
	) -> Result<Option<ChannelPatch>, Failure> {
		if !action.valid() {
			return Err(Failure::Protocol);
		}
		let channel = action.channel();
		if let Action::AddMember { user, .. } | Action::RemoveMember { user, .. } = action {
			let method = if matches!(action, Action::AddMember { .. }) {
				Method::PUT
			} else {
				Method::DELETE
			};
			let bytes = self
				.request_limited(
					method,
					&format!("/channels/{channel}/recipients/{user}"),
					None,
					1024,
				)
				.await
				.map_err(|failure| {
					if matches!(failure, Failure::Capacity | Failure::Protocol) {
						Failure::Ambiguous
					} else {
						failure
					}
				})?;
			return if bytes.is_empty() {
				Ok(None)
			} else {
				Err(Failure::Ambiguous)
			};
		}
		let leaving = matches!(action, Action::Leave(_));
		let renaming = matches!(&action, Action::Edit { name: Some(_), .. });
		let payload = match action {
			Action::Leave(_) => None,
			Action::Create(_)
			| Action::TransferOwner { .. }
			| Action::LoadMembers(_)
			| Action::AddMember { .. }
			| Action::RemoveMember { .. } => {
				return Err(Failure::Protocol);
			}
			Action::Edit { name, icon, .. } => {
				let mut body = serde_json::json!({});
				if let Some(name) = name {
					body["name"] = name.into();
				}
				match icon {
					Patch::Absent => {}
					Patch::Null => body["icon"] = serde_json::Value::Null,
					Patch::Value(data) => {
						if !discord_protocol::group_actions::valid_icon_data_uri(&data) {
							return Err(Failure::Protocol);
						}
						body["icon"] = data.into();
					}
				}
				Some(body)
			}
		};
		let bytes = self
			.request_limited(
				if leaving {
					Method::DELETE
				} else {
					Method::PATCH
				},
				&format!("/channels/{channel}"),
				payload,
				64 * 1024,
			)
			.await
			.map_err(|f| {
				if f == Failure::Capacity {
					Failure::Ambiguous
				} else {
					f
				}
			})?;
		if leaving && bytes.is_empty() {
			return Ok(None);
		}
		let dto: discord_protocol::ChannelDto =
			discord_protocol::decode(&bytes).map_err(|_| Failure::Ambiguous)?;
		if dto.id != channel || dto.guild_id.is_some() || dto.kind != 3 {
			return Err(Failure::Ambiguous);
		}
		if leaving {
			return Ok(None);
		}
		let name = match dto.name {
			Some(name)
				if !name.is_empty() && name.chars().count() <= 100 && name.capacity() <= 400 =>
			{
				Patch::Value(name)
			}
			None if !renaming => Patch::Absent,
			_ => return Err(Failure::Ambiguous),
		};
		let icon = match dto.icon {
			Some(hash) if model::valid_avatar_hash(&hash) && hash.capacity() <= 128 => {
				Patch::Value(hash)
			}
			Some(_) => return Err(Failure::Ambiguous),
			None => Patch::Null,
		};
		Ok(Some(ChannelPatch {
			id: channel,
			name,
			icon,
			last_message: Patch::Absent,
			parent_id: Patch::Absent,
			position: Patch::Absent,
			kind: Patch::Absent,
			message_count: Patch::Absent,
			tags: Patch::Absent,
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::{Command, Event, auth::SessionSecret};
	use model::Id;
	use std::sync::Arc;
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	#[tokio::test]
	async fn group_creation_and_transfer_use_observed_fields_and_require_matching_results() {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut api = DiscordApi::new(Arc::new(
			SessionSecret::from_owner_input("SYNTHETIC_GROUP_CREATE_TOKEN".into()).unwrap(),
		))
		.unwrap();
		api.base = format!("http://{}", listener.local_addr().unwrap());
		let group = serde_json::json!({"id":"10","type":3,"owner_id":"1","recipients":[{"id":"2","username":"Two"},{"id":"3","username":"Three"}]});
		for (action, path, payload, owner, valid) in [
			(
				Action::Create(vec![Id(2), Id(3)]),
				"POST /users/@me/channels HTTP/1.1",
				serde_json::json!({"recipients":["2","3"]}),
				"1",
				true,
			),
			(
				Action::Create(vec![Id(2), Id(4)]),
				"POST /users/@me/channels HTTP/1.1",
				serde_json::json!({"recipients":["2","4"]}),
				"1",
				false,
			),
			(
				Action::TransferOwner {
					channel: Id(10),
					user: Id(2),
				},
				"PATCH /channels/10 HTTP/1.1",
				serde_json::json!({"owner":"2"}),
				"2",
				true,
			),
			(
				Action::TransferOwner {
					channel: Id(10),
					user: Id(2),
				},
				"PATCH /channels/10 HTTP/1.1",
				serde_json::json!({"owner":"2"}),
				"3",
				false,
			),
		] {
			let server = async {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut request = Vec::new();
				loop {
					let mut chunk = [0; 1024];
					let count = socket.read(&mut chunk).await.unwrap();
					assert!(count > 0);
					request.extend_from_slice(&chunk[..count]);
					assert!(request.len() <= 4096);
					if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
						let headers = std::str::from_utf8(&request[..end]).unwrap();
						let length = headers
							.lines()
							.find_map(|header| {
								header
									.to_ascii_lowercase()
									.strip_prefix("content-length: ")
									.map(str::to_owned)
							})
							.unwrap()
							.parse::<usize>()
							.unwrap();
						if request.len() < end + 4 + length {
							continue;
						}
						assert!(headers.starts_with(path));
						assert_eq!(
							serde_json::from_slice::<serde_json::Value>(&request[end + 4..])
								.unwrap(),
							payload
						);
						break;
					}
				}
				let mut response = group.clone();
				response["owner_id"] = owner.into();
				let body = response.to_string();
				socket
					.write_all(
						format!(
							"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await
					.unwrap();
			};
			let (event, ()) = tokio::join!(
				api.execute(Command::GroupAction { action, request: 9 }),
				server
			);
			let success = match event {
				Event::GroupAction(client_core::group_actions::Event::Created {
					request: 9,
					result,
				}) => result.is_ok(),
				Event::GroupAction(client_core::group_actions::Event::MembersLoaded {
					request: 9,
					channel: Id(10),
					result,
				}) => result.is_ok(),
				_ => panic!("unexpected group result"),
			};
			assert_eq!(success, valid);
		}
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(10), listener.accept())
				.await
				.is_err()
		);
	}
	#[tokio::test]
	async fn group_member_snapshot_is_on_demand_scoped_and_bounded() {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut api = DiscordApi::new(Arc::new(
			SessionSecret::from_owner_input("SYNTHETIC_MEMBERS_TOKEN".into()).unwrap(),
		))
		.unwrap();
		api.base = format!("http://{}", listener.local_addr().unwrap());
		let valid = serde_json::json!({"id":"10","type":3,"owner_id":"1","recipients":[{"id":"2","username":"Synthetic friend"}]});
		let mut foreign = valid.clone();
		foreign["id"] = "11".into();
		let mut too_many = valid.clone();
		too_many["recipients"] = serde_json::json!(
			(2..=11)
				.map(|id| serde_json::json!({"id":id.to_string(),"username":"Synthetic friend"}))
				.collect::<Vec<_>>()
		);
		let mut missing_owner = valid.clone();
		missing_owner.as_object_mut().unwrap().remove("owner_id");
		for (response, expected) in [
			(valid, true),
			(foreign, false),
			(too_many, false),
			(missing_owner, false),
		] {
			let server = async {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut request = Vec::new();
				while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
					let mut chunk = [0; 1024];
					let count = socket.read(&mut chunk).await.unwrap();
					assert!(count > 0);
					request.extend_from_slice(&chunk[..count]);
					assert!(request.len() <= 4096);
				}
				assert!(request.starts_with(b"GET /channels/10 HTTP/1.1\r\n"));
				let body = response.to_string();
				socket
					.write_all(
						format!(
							"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await
					.unwrap();
			};
			let (event, ()) = tokio::join!(
				api.execute(Command::GroupAction {
					action: Action::LoadMembers(Id(10)),
					request: 7,
				}),
				server
			);
			let Event::GroupAction(client_core::group_actions::Event::MembersLoaded {
				channel,
				request,
				result,
			}) = event
			else {
				panic!("wrong group read result");
			};
			assert_eq!((channel, request), (Id(10), 7));
			assert_eq!(result.is_ok(), expected);
			if let Ok(members) = result {
				assert_eq!(members.owner, Id(1));
				assert_eq!(members.recipients[0].id, Id(2));
				assert!(members.valid(Id(1)));
			}
		}
	}
	#[tokio::test]
	async fn group_routes_validate_outcomes_and_never_retry_uncertain_writes() {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let mut api = DiscordApi::new(Arc::new(
			SessionSecret::from_owner_input("SYNTHETIC_GROUP_TOKEN".into()).unwrap(),
		))
		.unwrap();
		api.base = format!("http://{}", listener.local_addr().unwrap());
		for (action, method, payload, status, body, success) in [
			(
				Action::AddMember {
					channel: Id(10),
					user: Id(2),
				},
				"PUT",
				None,
				204,
				"",
				true,
			),
			(
				Action::RemoveMember {
					channel: Id(10),
					user: Id(2),
				},
				"DELETE",
				None,
				204,
				"",
				true,
			),
			(
				Action::AddMember {
					channel: Id(10),
					user: Id(2),
				},
				"PUT",
				None,
				200,
				"{}",
				false,
			),
			(
				Action::Edit {
					channel: Id(10),
					name: None,
					icon: Patch::Null,
				},
				"PATCH",
				Some(serde_json::json!({"icon":null})),
				200,
				r#"{"id":"10","type":3,"name":null,"icon":null}"#,
				true,
			),
			(
				Action::Edit {
					channel: Id(10),
					name: Some("Renamed".into()),
					icon: Patch::Absent,
				},
				"PATCH",
				Some(serde_json::json!({"name":"Renamed"})),
				200,
				r#"{"id":"10","type":3,"name":null,"icon":null}"#,
				false,
			),
			(
				Action::Edit {
					channel: Id(10),
					name: Some("Renamed".into()),
					icon: Patch::Absent,
				},
				"PATCH",
				Some(serde_json::json!({"name":"Renamed"})),
				200,
				r#"{"id":"10","type":3,"name":"Renamed","icon":null}"#,
				true,
			),
			(
				Action::Edit {
					channel: Id(10),
					name: Some("Renamed".into()),
					icon: Patch::Null,
				},
				"PATCH",
				Some(serde_json::json!({"name":"Renamed","icon":null})),
				200,
				r#"{"id":"11","type":3,"name":"Wrong"}"#,
				false,
			),
			(
				Action::Leave(Id(10)),
				"DELETE",
				None,
				200,
				r#"{"id":"10","type":3}"#,
				true,
			),
			(Action::Leave(Id(10)), "DELETE", None, 500, r#"{}"#, false),
		] {
			let expected_path = if matches!(
				action,
				Action::AddMember { .. } | Action::RemoveMember { .. }
			) {
				"/channels/10/recipients/2"
			} else {
				"/channels/10"
			};
			let server = async {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut bytes = Vec::new();
				loop {
					let mut chunk = [0; 1024];
					let n = socket.read(&mut chunk).await.unwrap();
					assert!(n > 0);
					bytes.extend_from_slice(&chunk[..n]);
					assert!(bytes.len() <= 4096);
					if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
						let headers = std::str::from_utf8(&bytes[..end]).unwrap();
						let len = headers
							.lines()
							.find_map(|h| {
								h.to_ascii_lowercase()
									.strip_prefix("content-length: ")
									.map(str::to_owned)
							})
							.map_or(0, |n| n.parse::<usize>().unwrap());
						if bytes.len() < end + 4 + len {
							continue;
						}
						assert!(
							headers.starts_with(&format!("{method} {expected_path} HTTP/1.1\r\n"))
						);
						assert_eq!(
							if len == 0 {
								None
							} else {
								Some(
									serde_json::from_slice::<serde_json::Value>(&bytes[end + 4..])
										.unwrap(),
								)
							},
							payload
						);
						break;
					}
				}
				socket
					.write_all(
						format!(
							"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await
					.unwrap();
			};
			let (event, ()) = tokio::join!(
				api.execute(Command::GroupAction { action, request: 1 }),
				server
			);
			let Event::GroupAction(client_core::group_actions::Event::Written { result, .. }) =
				event
			else {
				panic!("wrong event");
			};
			assert_eq!(result.is_ok(), success);
			if success && body.contains("\"name\":null") {
				assert!(matches!(
					result,
					Ok(Some(ChannelPatch {
						name: Patch::Absent,
						..
					}))
				));
			}
		}
		assert!(matches!(
			api.group_action(Action::Edit {
				channel: Id(10),
				name: Some("Valid".into()),
				icon: Patch::Value("data:image/png;base64,bad".into())
			})
			.await,
			Err(Failure::Protocol)
		));
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(10), listener.accept())
				.await
				.is_err()
		);
	}
}
