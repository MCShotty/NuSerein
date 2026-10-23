//! Explicit server-installed app command permissions, reconciled before and after each write.
use crate::{DiscordApi, Failure};
use model::{Id, command_permissions as m};
use reqwest::Method;
use serde_json::json;
impl DiscordApi {
	async fn command_permission_list(
		&self,
		guild: Id,
		app: Id,
	) -> Result<Vec<m::Permissions>, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/applications/{app}/guilds/{guild}/commands/permissions"),
				None,
				m::MAX_BYTES,
			)
			.await?;
		discord_protocol::command_permissions::list(&bytes, guild, app).map_err(|_| {
			Failure::ProtocolAt("Command permissions were invalid or exceeded their limits")
		})
	}
	async fn command_permission_one(
		&self,
		guild: Id,
		app: Id,
		id: Id,
	) -> Result<m::Permissions, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/applications/{app}/guilds/{guild}/commands/{id}/permissions"),
				None,
				32 * 1024,
			)
			.await?;
		discord_protocol::command_permissions::one(&bytes, guild, app, id).map_err(|_| {
			Failure::ProtocolAt("Command permissions did not match this application and server")
		})
	}
	pub(super) async fn command_permissions(
		&self,
		guild: Id,
		action: &m::Action,
	) -> Result<m::Snapshot, Failure> {
		if guild.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		let app = action.application();
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/guilds/{guild}/application-command-index"),
				None,
				discord_protocol::MAX_WIRE,
			)
			.await?;
		let index = discord_protocol::application_commands::decode_context(&bytes, Some(guild), 0)
			.map_err(|_| Failure::ProtocolAt("Application command list is unavailable"))?;
		let commands = index
			.into_iter()
			.filter(|command| command.application_id == app)
			.map(|command| m::Command {
				id: command.id,
				name: command.name,
				kind: command.kind,
			})
			.collect::<Vec<_>>();
		if commands.len() > m::MAX_COMMANDS {
			return Err(Failure::Capacity);
		}
		if let m::Action::Save { before, after, .. } = action {
			if before.id != app && !commands.iter().any(|command| command.id == before.id) {
				return Err(Failure::Forbidden);
			}
			let fresh = self.command_permission_one(guild, app, before.id).await?;
			let merged = m::Permissions::merge(before, after, &fresh).ok_or(
				Failure::ProtocolAt("Command permissions changed; reload and review your changes"),
			)?;
			let bytes = self
				.request_limited(
					Method::PUT,
					&format!(
						"/applications/{app}/guilds/{guild}/commands/{}/permissions",
						before.id
					),
					Some(json!({"permissions":merged.overrides})),
					32 * 1024,
				)
				.await
				.map_err(|failure| {
					if failure == Failure::Capacity {
						Failure::Ambiguous
					} else {
						failure
					}
				})?;
			let saved = discord_protocol::command_permissions::one(&bytes, guild, app, before.id)
				.map_err(|_| Failure::Ambiguous)?;
			if !saved.equivalent(&merged) {
				return Err(Failure::Ambiguous);
			}
			let checked = self
				.command_permission_one(guild, app, before.id)
				.await
				.map_err(|failure| {
					if failure.ends_session() {
						failure
					} else {
						Failure::Ambiguous
					}
				})?;
			if !checked.equivalent(&merged) {
				return Err(Failure::Ambiguous);
			}
		}
		let permissions = self
			.command_permission_list(guild, app)
			.await
			.map_err(|failure| {
				if action.write() && !failure.ends_session() {
					Failure::Ambiguous
				} else {
					failure
				}
			})?;
		let snapshot = m::Snapshot {
			guild,
			application: app,
			commands,
			permissions,
		};
		if !snapshot.valid() {
			return Err(if action.write() {
				Failure::Ambiguous
			} else {
				Failure::Capacity
			});
		}
		if let m::Action::Save { before, after, .. } = action
			&& !snapshot
				.get(after.id)
				.is_some_and(|actual| m::Permissions::confirms(before, after, &actual))
		{
			return Err(Failure::Ambiguous);
		}
		Ok(snapshot)
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
	async fn permission_save_merges_concurrent_unknown_rules_confirms_and_never_repeats_a_conflict()
	{
		tokio::time::timeout(Duration::from_secs(10),async{
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_COMMAND_PERMISSION_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());
			let index=json!({"application_commands":[{"id":"4","application_id":"2","version":"5","type":1,"name":"ping","description":"Ping"}],"applications":[{"id":"2","name":"Example"}]});
			let old=json!({"id":"4","application_id":"2","guild_id":"3","permissions":[{"id":"8","type":1,"permission":true},{"id":"9","type":42,"permission":false}]});
			let new=json!({"id":"4","application_id":"2","guild_id":"3","permissions":[{"id":"8","type":1,"permission":false},{"id":"9","type":42,"permission":false}]});
			let server=tokio::spawn(async move{
				let path="/applications/2/guilds/3/commands/4/permissions";
				for(method,route,response,expected)in[
					("GET","/guilds/3/application-command-index",index.clone(),None),
					("GET",path,old,None),
					("PUT",path,new.clone(),Some(json!({"permissions":[{"id":"8","type":1,"permission":false},{"id":"9","type":42,"permission":false}]}))),
					("GET",path,new.clone(),None),
					("GET","/applications/2/guilds/3/commands/permissions",json!([new.clone()]),None),
					("GET","/guilds/3/application-command-index",index,None),
					("GET",path,new,None),
				]{
					let(mut stream,_)=listener.accept().await.unwrap();let mut input=vec![];
					let end=loop{let mut part=[0;4096];let n=stream.read(&mut part).await.unwrap();assert!(n>0);input.extend_from_slice(&part[..n]);assert!(input.len()<65536);if let Some(end)=input.windows(4).position(|p|p==b"\r\n\r\n"){break end+4;}};
					let headers=std::str::from_utf8(&input[..end]).unwrap();assert!(headers.starts_with(&format!("{method} {route} HTTP/1.1\r\n")));
					let len=headers.lines().find_map(|line|{let(k,v)=line.split_once(':')?;k.eq_ignore_ascii_case("content-length").then(||v.trim().parse::<usize>().unwrap())}).unwrap_or(0);
					while input.len()<end+len{let mut part=[0;4096];let n=stream.read(&mut part).await.unwrap();assert!(n>0);input.extend_from_slice(&part[..n]);}
					if let Some(expected)=expected{assert_eq!(serde_json::from_slice::<serde_json::Value>(&input[end..end+len]).unwrap(),expected);}else{assert_eq!(len,0);}
					let body=response.to_string();stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
				}
				assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err());
			});
			let before=m::Permissions{id:Id(4),overrides:vec![m::Override{id:Id(8),kind:1,permission:true},m::Override{id:Id(9),kind:42,permission:true}]};
			let mut after=before.clone();after.overrides[0].permission=false;
			let action=m::Action::Save{application:Id(2),before,after};
			let result=api.command_permissions(Id(3),&action).await.unwrap();assert!(!result.get(Id(4)).unwrap().overrides[1].permission);
			assert!(matches!(api.command_permissions(Id(3),&action).await,Err(Failure::ProtocolAt(_))));server.await.unwrap();
		}).await.unwrap();
	}
}
