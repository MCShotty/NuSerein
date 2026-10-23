//! Explicit application review and access mutations; no background or uncertain write retries.
use crate::{DiscordApi, Failure};
use discord_protocol::server_access as wire;
use model::{
	Id,
	server_access::{Action, Outcome, Settings, Status},
};
use reqwest::Method;
use serde_json::json;

impl DiscordApi {
	async fn access_settings(&self, guild: Id) -> Result<Settings, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/guilds/{guild}"),
				None,
				wire::MAX_WIRE,
			)
			.await?;
		wire::settings(&bytes, guild).map_err(|_| {
			Failure::ProtocolAt("Server access settings are unavailable or incompatible")
		})
	}
	async fn access_application(
		&self,
		guild: Id,
		request: Id,
	) -> Result<model::server_access::Application, Failure> {
		let bytes = self
			.request_limited(
				Method::GET,
				&format!("/join-requests/{request}"),
				None,
				64 * 1024,
			)
			.await?;
		wire::application(&bytes, guild, request).map_err(|_| {
			Failure::ProtocolAt("The application was removed or its response is incompatible")
		})
	}
	pub(super) async fn server_access_action(
		&self,
		guild: Id,
		action: &Action,
	) -> Result<Outcome, Failure> {
		if guild.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		match action {
			Action::Applications { status, before } => {
				let path = format!(
					"/guilds/{guild}/requests?status={}&limit=100{}",
					status.wire(),
					before.map_or_else(String::new, |id| format!("&before={id}"))
				);
				let bytes = self
					.request_limited(Method::GET, &path, None, wire::MAX_WIRE)
					.await?;
				wire::applications(&bytes, guild, *status)
					.map(Outcome::Applications)
					.map_err(|_| {
						Failure::ProtocolAt(
							"Applications exceeded their limits or used an unsupported response",
						)
					})
			}
			Action::Application(request) => self
				.access_application(guild, *request)
				.await
				.map(Outcome::Application),
			Action::Decide {
				request,
				status,
				reason,
			} => {
				let current = self.access_application(guild, *request).await?;
				if current.status != Status::Pending {
					return Err(Failure::ProtocolAt(
						"This application has already been reviewed; reload it before continuing",
					));
				}
				let mut body = json!({"action":status.wire()});
				if *status == Status::Rejected {
					body["rejection_reason"] = json!(reason);
				}
				let bytes = self
					.request_limited(
						Method::PATCH,
						&format!("/guilds/{guild}/requests/{request}"),
						Some(body),
						64 * 1024,
					)
					.await
					.map_err(write_failure)?;
				let updated =
					wire::application(&bytes, guild, *request).map_err(|_| Failure::Ambiguous)?;
				if updated.status != *status
					|| (*status == Status::Rejected
						&& !reason.is_empty()
						&& updated.reason.as_deref() != Some(reason.as_str()))
				{
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Application(updated))
			}
			Action::Interview { request } => {
				let current = self.access_application(guild, *request).await?;
				if current.status != Status::Pending {
					return Err(Failure::ProtocolAt(
						"Only pending applications can open an interview",
					));
				}
				let bytes = self
					.request_limited(
						Method::POST,
						&format!("/join-requests/{request}/interview"),
						None,
						128 * 1024,
					)
					.await
					.map_err(write_failure)?;
				let dto: discord_protocol::ChannelDto =
					discord_protocol::decode(&bytes).map_err(|_| Failure::Ambiguous)?;
				let channel = Box::new(dto.into_model());
				let application = self
					.access_application(guild, *request)
					.await
					.map_err(reconcile_failure)?;
				let outcome = Outcome::Interview {
					application,
					channel,
				};
				if !outcome.valid() {
					return Err(Failure::Ambiguous);
				}
				Ok(outcome)
			}
			Action::Settings => self.access_settings(guild).await.map(Outcome::Settings),
			Action::Recipient(user) => {
				let member = self.admin_member(guild, *user).await?;
				if member.user.kind != model::AccountKind::Human || member.user.webhook {
					return Err(Failure::ProtocolAt(
						"Choose a current human member as the new owner",
					));
				}
				Ok(Outcome::Recipient(Box::new(member)))
			}
			Action::Incident { before, after } => {
				let current = self.access_settings(guild).await?;
				if current.incident != Some(*before) {
					return Err(Failure::ProtocolAt(
						"Security actions changed; reload before saving",
					));
				}
				let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
				if !after.writable(now) {
					return Err(Failure::ProtocolAt(
						"Security actions must expire within 24 hours",
					));
				}
				let body = wire::incident_body(*after).map_err(|_| Failure::Protocol)?;
				let _bytes = self
					.request_limited(
						Method::PUT,
						&format!("/guilds/{guild}/incident-actions"),
						Some(body),
						64 * 1024,
					)
					.await
					.map_err(write_failure)?;
				let updated = self
					.access_settings(guild)
					.await
					.map_err(reconcile_failure)?;
				if updated.incident != Some(*after) {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Settings(updated))
			}
			Action::Configure { before, after } => {
				let current = self.access_settings(guild).await?;
				let body = wire::configuration_body(&current, *before, *after).map_err(|_| {
					Failure::ProtocolAt(
						"Community configuration changed or its requirements are not met; reload before saving",
					)
				})?;
				let _bytes = self
					.request_limited(
						Method::PATCH,
						&format!("/guilds/{guild}"),
						Some(body),
						wire::MAX_WIRE,
					)
					.await
					.map_err(write_failure)?;
				let updated = self
					.access_settings(guild)
					.await
					.map_err(reconcile_failure)?;
				if updated.configuration != *after {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Settings(updated))
			}
			Action::MfaLevel { before, after } => {
				let current = self.access_settings(guild).await?;
				if current.mfa_level != *before {
					return Err(Failure::ProtocolAt(
						"Moderator verification changed; reload before saving",
					));
				}
				let _bytes = self
					.request_limited(
						Method::POST,
						&format!("/guilds/{guild}/mfa"),
						Some(json!({"level":after})),
						64 * 1024,
					)
					.await
					.map_err(write_failure)?;
				let updated = self
					.access_settings(guild)
					.await
					.map_err(reconcile_failure)?;
				if updated.mfa_level != *after {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Settings(updated))
			}
			Action::Transfer {
				before_owner,
				recipient,
			} => {
				let current = self.access_settings(guild).await?;
				if current.owner != *before_owner {
					return Err(Failure::ProtocolAt(
						"Server ownership changed; reload before continuing",
					));
				}
				let member = self.admin_member(guild, *recipient).await?;
				if member.user.kind != model::AccountKind::Human || member.user.webhook {
					return Err(Failure::ProtocolAt(
						"Choose a current human member as the new owner",
					));
				}
				let _bytes = self
					.request_limited(
						Method::PATCH,
						&format!("/guilds/{guild}"),
						Some(json!({"owner_id":recipient})),
						wire::MAX_WIRE,
					)
					.await
					.map_err(write_failure)?;
				let updated = self
					.access_settings(guild)
					.await
					.map_err(reconcile_failure)?;
				if updated.owner != *recipient {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Settings(updated))
			}
		}
	}
}
fn write_failure(failure: Failure) -> Failure {
	if failure == Failure::Capacity {
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
	async fn application_decision_reads_current_state_and_does_not_repeat_uncertain_write() {
		tokio::time::timeout(Duration::from_secs(5),async{
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_APPLICATION_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());
			let server=tokio::spawn(async move{
				let request=json!({"id":"10","join_request_id":"10","guild_id":"2","user_id":"3","application_status":"SUBMITTED","created_at":"2026-01-01T00:00:00Z"});
				for (method,path,status,response) in [("GET","/join-requests/10",200,request),("PATCH","/guilds/2/requests/10",500,json!({}))]{
					let(mut stream,_)=listener.accept().await.unwrap();let mut bytes=Vec::new();
					let end=loop{let mut buf=[0;4096];let n=stream.read(&mut buf).await.unwrap();assert!(n>0);bytes.extend_from_slice(&buf[..n]);assert!(bytes.len()<8192);if let Some(i)=bytes.windows(4).position(|v|v==b"\r\n\r\n"){break i+4;}};
					let headers=std::str::from_utf8(&bytes[..end]).unwrap();assert!(headers.starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
					let length=headers.lines().find_map(|line|{let(name,value)=line.split_once(':')?;name.eq_ignore_ascii_case("content-length").then(||value.trim().parse::<usize>().unwrap())}).unwrap_or(0);
					while bytes.len()<end+length{let mut buf=[0;4096];let n=stream.read(&mut buf).await.unwrap();assert!(n>0);bytes.extend_from_slice(&buf[..n]);}
					if method=="PATCH"{assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes[end..end+length]).unwrap(),json!({"action":"REJECTED","rejection_reason":"Not a fit"}));}
					let body=response.to_string();stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
				}
				assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err());
			});
			assert!(matches!(api.server_access_action(Id(2),&Action::Decide{request:Id(10),status:Status::Rejected,reason:"Not a fit".into()}).await,Err(Failure::Ambiguous)));server.await.unwrap();
		}).await.unwrap();
	}
}
