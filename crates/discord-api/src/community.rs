//! Official event/Stage resources; normal-account subscription routes are unofficial.
//! One attempt per write. Only bounded reads may follow explicit service confirmation.
use crate::{DiscordApi, Failure};
use client_core::community::Event;
use discord_protocol::community as wire;
use model::{Id, community::*};
use reqwest::Method;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn write_failure(f: Failure) -> Failure {
	if matches!(f, Failure::Capacity | Failure::Protocol | Failure::Network) {
		Failure::Ambiguous
	} else {
		f
	}
}
impl DiscordApi {
	pub(super) async fn community(
		&self,
		guild: Id,
		user: Id,
		request: u64,
		action: &Action,
	) -> Event {
		Event {
			guild,
			request,
			result: self.community_action(guild, user, action).await,
		}
	}
	async fn community_action(
		&self,
		guild: Id,
		user: Id,
		action: &Action,
	) -> Result<Outcome, Failure> {
		if guild.0 == 0 || user.0 == 0 || !action.valid() {
			return Err(Failure::Protocol);
		}
		if matches!(action,Action::Edit {before,..}|Action::EditOccurrence{before,..}|Action::RestoreOccurrence{before,..} if before.guild!=guild)
		{
			return Err(Failure::Protocol);
		}
		let base = format!("/guilds/{guild}/scheduled-events");
		match action {
			Action::ResetOccurrenceRsvp { event, occurrence } => {
				let bytes = self
					.request_limited(
						Method::DELETE,
						&format!("{base}/{event}/{occurrence}/users/@me"),
						None,
						4096,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::OccurrenceRsvpReset {
					event: *event,
					occurrence: *occurrence,
				})
			}
			Action::EditOccurrence {
				before,
				original,
				canceled,
				start,
				end,
			} => {
				let id = occurrence_id(*original).ok_or(Failure::Protocol)?;
				let exists = before.exceptions.iter().any(|e| e.id == id);
				let path = if exists {
					format!("{base}/{}/exceptions/{id}", before.id)
				} else {
					format!("{base}/{}/exceptions", before.id)
				};
				let mut body = json!({"is_canceled":canceled,"scheduled_start_time":start.map(wire::iso).transpose().map_err(|_|Failure::Protocol)?,"scheduled_end_time":end.map(wire::iso).transpose().map_err(|_|Failure::Protocol)?});
				if !exists {
					body["original_scheduled_start_time"] =
						json!(wire::iso(*original).map_err(|_| Failure::Protocol)?);
				}
				let bytes = self
					.request_limited(
						if exists { Method::PATCH } else { Method::POST },
						&path,
						Some(body),
						8192,
					)
					.await
					.map_err(write_failure)?;
				let exception =
					wire::event_exception(&bytes, before.id).map_err(|_| Failure::Ambiguous)?;
				if exception.id != id
					|| exception.canceled != *canceled
					|| exception.start != *start
					|| exception.end != *end
				{
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Exception {
					event: before.id,
					exception,
				})
			}
			Action::RestoreOccurrence { before, exception } => {
				let bytes = self
					.request_limited(
						Method::DELETE,
						&format!("{base}/{}/exceptions/{exception}", before.id),
						None,
						4096,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::ExceptionRestored {
					event: before.id,
					exception: *exception,
				})
			}
			Action::RsvpOccurrence {
				event,
				occurrence,
				interested,
			} => {
				let bytes = self
					.request_limited(
						Method::PUT,
						&format!("{base}/{event}/{occurrence}/users/@me"),
						Some(json!({"response":if *interested{1}else{0}})),
						4096,
					)
					.await
					.map_err(write_failure)?;
				if *interested {
					wire::subscription_response(&bytes, user, *event, Some(*occurrence))
						.map_err(|_| Failure::Ambiguous)?;
				} else {
					let values = serde_json::from_slice::<serde_json::Value>(&bytes)
						.map_err(|_| Failure::Ambiguous)?;
					if values["guild_scheduled_event_id"] != event.to_string()
						|| values["guild_scheduled_event_exception_id"] != occurrence.to_string()
						|| values["user_id"] != user.to_string()
						|| values["response"] != 0
					{
						return Err(Failure::Ambiguous);
					}
				}
				Ok(Outcome::Rsvp {
					event: *event,
					occurrence: Some(*occurrence),
					interested: *interested,
				})
			}
			Action::ListEvents => {
				let bytes = self
					.request_limited(
						Method::GET,
						&format!("{base}?with_user_count=true"),
						None,
						MAX_WIRE,
					)
					.await?;
				let mut page = wire::events(&bytes, guild).map_err(|_| Failure::Protocol)?;
				// Optional account-only enrichment. Failure is unknown subscription state, never false.
				match self
					.request_optional_limited(
						&format!("/users/@me/scheduled-events?guild_ids={guild}"),
						128 * 1024,
					)
					.await
				{
					Ok(Some(bytes)) => {
						page.subscriptions =
							Some(wire::subscriptions(&bytes, user).map_err(|_| Failure::Protocol)?);
						page.subscriptions_complete = true;
					}
					Err(f) if f.ends_session() => return Err(f),
					_ => {}
				}
				Ok(Outcome::Events(page))
			}
			Action::GetEvent(id) => {
				let bytes = self
					.request_limited(
						Method::GET,
						&format!("{base}/{id}?with_user_count=true"),
						None,
						64 * 1024,
					)
					.await?;
				let event = wire::event(&bytes, guild).map_err(|_| Failure::Protocol)?;
				if event.id != *id {
					return Err(Failure::Protocol);
				}
				Ok(Outcome::Event(event))
			}
			Action::Create(draft) | Action::Edit { after: draft, .. } => {
				let before = if let Action::Edit { before, .. } = action {
					Some(&before.draft)
				} else {
					None
				};
				let now = SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.map_err(|_| Failure::Protocol)?
					.as_secs() as i64;
				if before.is_none_or(|b| b.start != draft.start) && draft.start <= now {
					return Err(Failure::ProtocolAt("Choose a future event start time"));
				}
				let body = wire::event_body(draft, before).map_err(|_| Failure::Protocol)?;
				let (method, path) = if let Action::Edit { before, .. } = action {
					(Method::PATCH, format!("{base}/{}", before.id))
				} else {
					(Method::POST, base)
				};
				let bytes = self
					.request_limited(method, &path, Some(body), 64 * 1024)
					.await
					.map_err(write_failure)?;
				let event = wire::event(&bytes, guild).map_err(|_| Failure::Ambiguous)?;
				if event.draft != *draft
					|| matches!(action,Action::Edit{before,..} if event.id!=before.id)
				{
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Event(event))
			}
			Action::Status { event, status } => {
				let bytes = self
					.request_limited(
						Method::PATCH,
						&format!("{base}/{event}"),
						Some(json!({"status":status})),
						64 * 1024,
					)
					.await
					.map_err(write_failure)?;
				let changed = wire::event(&bytes, guild).map_err(|_| Failure::Ambiguous)?;
				if changed.id != *event || changed.status != *status {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Event(changed))
			}
			Action::Delete(id) => {
				let bytes = self
					.request_limited(Method::DELETE, &format!("{base}/{id}"), None, 4096)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Deleted(*id))
			}
			Action::Rsvp { event, interested } => {
				let bytes = self
					.request_limited(
						if *interested {
							Method::PUT
						} else {
							Method::DELETE
						},
						&format!("{base}/{event}/users/@me"),
						None,
						4096,
					)
					.await
					.map_err(write_failure)?;
				if *interested {
					wire::subscription(&bytes, user, *event).map_err(|_| Failure::Ambiguous)?;
				} else if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Rsvp {
					occurrence: None,
					event: *event,
					interested: *interested,
				})
			}
			Action::GetStage {
				channel,
				call_request,
			} => {
				let bytes = self
					.request_optional_limited(&format!("/stage-instances/{channel}"), 8192)
					.await?;
				let voice = if call_request.is_some() {
					let bytes = self
						.request_limited(
							Method::GET,
							&format!("/guilds/{guild}/voice-states/@me"),
							None,
							8192,
						)
						.await?;
					Some(
						wire::stage_voice(&bytes, guild, *channel, user)
							.map_err(|_| Failure::Protocol)?,
					)
				} else {
					None
				};
				Ok(Outcome::Stage {
					channel: *channel,
					voice,
					instance: bytes
						.map(|b| wire::stage(&b, guild, *channel).map_err(|_| Failure::Protocol))
						.transpose()?,
				})
			}
			Action::CreateStage { channel, topic, .. } | Action::EditStage { channel, topic } => {
				let event = if let Action::CreateStage { event, .. } = action {
					*event
				} else {
					None
				};
				let create = matches!(action, Action::CreateStage { .. });
				let body = if create {
					json!({"channel_id":channel.to_string(),"topic":topic,"privacy_level":2,"send_start_notification":false,"guild_scheduled_event_id":event.map(|id|id.to_string())})
				} else {
					json!({"topic":topic})
				};
				let path = if create {
					"/stage-instances".to_owned()
				} else {
					format!("/stage-instances/{channel}")
				};
				let bytes = self
					.request_limited(
						if create { Method::POST } else { Method::PATCH },
						&path,
						Some(body),
						8192,
					)
					.await
					.map_err(write_failure)?;
				let instance =
					wire::stage(&bytes, guild, *channel).map_err(|_| Failure::Ambiguous)?;
				if instance.topic != *topic {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Stage {
					channel: *channel,
					instance: Some(instance),
					voice: None,
				})
			}
			Action::EndStage(channel) => {
				let bytes = self
					.request_limited(
						Method::DELETE,
						&format!("/stage-instances/{channel}"),
						None,
						4096,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::Stage {
					channel: *channel,
					instance: None,
					voice: None,
				})
			}
			Action::StageVoice {
				channel, change, ..
			} => {
				let mut body = json!({"channel_id":channel.to_string()});
				let mut target = "@me".to_owned();
				match change {
					StageVoiceChange::RequestToSpeak(requested) => {
						body["request_to_speak_timestamp"] = if *requested {
							json!(
								wire::iso(
									SystemTime::now()
										.duration_since(UNIX_EPOCH)
										.map_err(|_| Failure::Protocol)?
										.as_secs() as i64
								)
								.map_err(|_| Failure::Protocol)?
							)
						} else {
							serde_json::Value::Null
						};
					}
					StageVoiceChange::AcceptInvitation | StageVoiceChange::BecomeSpeaker => {
						body["suppress"] = json!(false);
						body["request_to_speak_timestamp"] = serde_json::Value::Null;
					}
					StageVoiceChange::Audience => {
						body["suppress"] = json!(true);
						body["request_to_speak_timestamp"] = serde_json::Value::Null;
					}
					StageVoiceChange::Invite(user) => {
						target = user.to_string();
						body["suppress"] = json!(false);
					}
					StageVoiceChange::MoveToAudience(user) => {
						target = user.to_string();
						body["suppress"] = json!(true);
					}
				}
				let bytes = self
					.request_limited(
						Method::PATCH,
						&format!("/guilds/{guild}/voice-states/{target}"),
						Some(body),
						4096,
					)
					.await
					.map_err(write_failure)?;
				if !bytes.is_empty() {
					return Err(Failure::Ambiguous);
				}
				Ok(Outcome::StageVoice { channel: *channel })
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::SessionSecret;
	use serde_json::Value;
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	async fn exchange(
		listener: &TcpListener,
		method: &str,
		path: &str,
		body: Option<Value>,
		status: u16,
		response: &str,
	) {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut bytes = Vec::new();
		let end = loop {
			let mut chunk = [0; 4096];
			let n = stream.read(&mut chunk).await.unwrap();
			assert!(n > 0);
			bytes.extend_from_slice(&chunk[..n]);
			assert!(bytes.len() < 64 * 1024);
			if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
				break end + 4;
			}
		};
		let headers = std::str::from_utf8(&bytes[..end]).unwrap();
		assert!(headers.starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
		let length = headers
			.lines()
			.find_map(|line| {
				let (name, value) = line.split_once(':')?;
				name.eq_ignore_ascii_case("content-length")
					.then(|| value.trim().parse::<usize>().unwrap())
			})
			.unwrap_or(0);
		assert!(length < 32 * 1024);
		while bytes.len() < end + length {
			let mut chunk = [0; 4096];
			let n = stream.read(&mut chunk).await.unwrap();
			assert!(n > 0);
			bytes.extend_from_slice(&chunk[..n]);
		}
		assert_eq!(
			if length == 0 {
				None
			} else {
				Some(serde_json::from_slice::<Value>(&bytes[end..end + length]).unwrap())
			},
			body
		);
		stream
			.write_all(
				format!(
					"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
					response.len()
				)
				.as_bytes(),
			)
			.await
			.unwrap();
	}
	#[tokio::test]
	async fn community_http_event_lifecycle_stage_actions_and_uncertain_writes_are_single_attempts()
	{
		tokio::time::timeout(Duration::from_secs(20),async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_COMMUNITY_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());
			let event=json!({"id":"30","guild_id":"10","creator_id":"1","channel_id":null,"name":"Meetup","description":"Synthetic","scheduled_start_time":"2030-01-01T08:00:00Z","scheduled_end_time":"2030-01-01T09:00:00Z","privacy_level":2,"status":1,"entity_type":3,"entity_metadata":{"location":"Park"}});
			let draft=wire::event(event.to_string().as_bytes(),Id(10)).unwrap().draft;
			let create_body=wire::event_body(&draft,None).unwrap();let changed={let mut e=event.clone();e["name"]=json!("Changed");e};let stage=json!({"id":"40","guild_id":"10","channel_id":"21","topic":"New topic","privacy_level":2,"guild_scheduled_event_id":null});
			let event_response=event.to_string();let changed_response=changed.to_string();let stage_response=stage.to_string();
			let server=tokio::spawn(async move {
				exchange(&listener,"GET","/guilds/10/scheduled-events?with_user_count=true",None,200,&format!("[{event_response}]")).await;
				exchange(&listener,"GET","/users/@me/scheduled-events?guild_ids=10",None,200,"[]").await;
				exchange(&listener,"POST","/guilds/10/scheduled-events",Some(create_body),200,&event_response).await;
				exchange(&listener,"PATCH","/guilds/10/scheduled-events/30",Some(json!({"name":"Changed"})),200,&changed_response).await;
				exchange(&listener,"PUT","/guilds/10/scheduled-events/30/users/@me",None,200,r#"{"guild_scheduled_event_id":"30","user_id":"1","response":1}"#).await;
				exchange(&listener,"DELETE","/guilds/10/scheduled-events/30/users/@me",None,204,"").await;
				exchange(&listener,"GET","/stage-instances/21",None,200,&stage_response).await;
				exchange(&listener,"GET","/guilds/10/voice-states/@me",None,200,r#"{"guild_id":"10","channel_id":"21","user_id":"1","suppress":true,"request_to_speak_timestamp":null}"#).await;
				exchange(&listener,"PATCH","/stage-instances/21",Some(json!({"topic":"New topic"})),200,&stage_response).await;
				exchange(&listener,"PATCH","/guilds/10/voice-states/@me",Some(json!({"channel_id":"21","suppress":false,"request_to_speak_timestamp":null})),204,"").await;
				exchange(&listener,"PATCH","/guilds/10/voice-states/2",Some(json!({"channel_id":"21","suppress":false})),204,"").await;
				exchange(&listener,"PATCH","/guilds/10/voice-states/@me",Some(json!({"channel_id":"21","suppress":true,"request_to_speak_timestamp":null})),500,"{}").await;
				assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err(),"unknown Stage writes must never be repeated");
			});
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::ListEvents).await,Ok(Outcome::Events(_))));
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::Create(draft.clone())).await,Ok(Outcome::Event(_))));
			let before=wire::event(event.to_string().as_bytes(),Id(10)).unwrap();let mut after=draft;after.name="Changed".into();assert!(matches!(api.community_action(Id(10),Id(1),&Action::Edit{before,after}).await,Ok(Outcome::Event(_))));
			for interested in [true,false]{assert!(matches!(api.community_action(Id(10),Id(1),&Action::Rsvp{event:Id(30),interested}).await,Ok(Outcome::Rsvp{..})));}
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::GetStage{channel:Id(21),call_request:Some(7)}).await,Ok(Outcome::Stage{voice:Some(_),..})));
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::EditStage{channel:Id(21),topic:"New topic".into()}).await,Ok(Outcome::Stage{..})));
			for change in [StageVoiceChange::AcceptInvitation,StageVoiceChange::Invite(Id(2))]{assert!(matches!(api.community_action(Id(10),Id(1),&Action::StageVoice{channel:Id(21),call_request:7,change}).await,Ok(Outcome::StageVoice{..})));}
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::StageVoice{channel:Id(21),call_request:7,change:StageVoiceChange::Audience}).await,Err(Failure::Ambiguous)));
			server.await.unwrap();
		}).await.unwrap();
	}
	#[tokio::test]
	async fn community_http_occurrence_lifecycle_and_series_cancel_delete_are_scoped_single_attempts()
	 {
		tokio::time::timeout(Duration::from_secs(20),async {
			let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_RECURRING_TOKEN".into()).unwrap())).unwrap();api.base=format!("http://{}",listener.local_addr().unwrap());
			let original=wire::timestamp("2030-01-01T08:00:00Z").unwrap();let id=occurrence_id(original).unwrap();
			let event=json!({"id":"30","guild_id":"10","creator_id":"1","name":"Daily meetup","description":"Synthetic recurring event","channel_id":null,"scheduled_start_time":"2030-01-01T08:00:00Z","scheduled_end_time":"2030-01-01T09:00:00Z","privacy_level":2,"status":1,"entity_type":3,"entity_metadata":{"location":"Park"},"recurrence_rule":{"start":"2030-01-01T08:00:00Z","frequency":3,"interval":1}});
			let mut before=wire::event(event.to_string().as_bytes(),Id(10)).unwrap();let canceled=json!({"event_id":"30","event_exception_id":id.to_string(),"is_canceled":true,"scheduled_start_time":null,"scheduled_end_time":null});let mut shifted=canceled.clone();shifted["is_canceled"]=json!(false);shifted["scheduled_start_time"]=json!("2030-01-01T08:30:00Z");shifted["scheduled_end_time"]=json!("2030-01-01T09:30:00Z");let exception=wire::event_exception(canceled.to_string().as_bytes(),Id(30)).unwrap();let cancel_response=canceled.to_string();let shift_response=shifted.to_string();let mut stopped=event.clone();stopped["status"]=json!(4);let stopped_response=stopped.to_string();let event_response=event.to_string();
			let server=tokio::spawn(async move {
				exchange(&listener,"GET","/guilds/10/scheduled-events/30?with_user_count=true",None,200,&event_response).await;
				exchange(&listener,"POST","/guilds/10/scheduled-events/30/exceptions",Some(json!({"original_scheduled_start_time":"2030-01-01T08:00:00Z","is_canceled":true,"scheduled_start_time":null,"scheduled_end_time":null})),200,&cancel_response).await;
				exchange(&listener,"PATCH",&format!("/guilds/10/scheduled-events/30/exceptions/{id}"),Some(json!({"is_canceled":false,"scheduled_start_time":"2030-01-01T08:30:00Z","scheduled_end_time":"2030-01-01T09:30:00Z"})),200,&shift_response).await;
				for response in [1,0] {exchange(&listener,"PUT",&format!("/guilds/10/scheduled-events/30/{id}/users/@me"),Some(json!({"response":response})),200,&json!({"guild_scheduled_event_id":"30","guild_scheduled_event_exception_id":id.to_string(),"user_id":"1","response":response}).to_string()).await;}
				exchange(&listener,"DELETE",&format!("/guilds/10/scheduled-events/30/{id}/users/@me"),None,204,"").await;
				exchange(&listener,"DELETE",&format!("/guilds/10/scheduled-events/30/exceptions/{id}"),None,204,"").await;
				exchange(&listener,"PATCH","/guilds/10/scheduled-events/30",Some(json!({"status":4})),200,&stopped_response).await;
				exchange(&listener,"DELETE","/guilds/10/scheduled-events/30",None,204,"").await;
				exchange(&listener,"POST","/guilds/10/scheduled-events/30/exceptions",Some(json!({"original_scheduled_start_time":"2030-01-01T08:00:00Z","is_canceled":true,"scheduled_start_time":null,"scheduled_end_time":null})),200,r#"{"event_id":"31","event_exception_id":"1","is_canceled":true}"#).await;
				assert!(tokio::time::timeout(Duration::from_millis(80),listener.accept()).await.is_err(),"unknown occurrence write must not be retried");
			});
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::GetEvent(Id(30))).await,Ok(Outcome::Event(_))));
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::EditOccurrence{before:before.clone(),original,canceled:true,start:None,end:None}).await,Ok(Outcome::Exception{..})));
			before.exceptions.push(exception);assert!(matches!(api.community_action(Id(10),Id(1),&Action::EditOccurrence{before:before.clone(),original,canceled:false,start:Some(original+1800),end:Some(original+5400)}).await,Ok(Outcome::Exception{..})));
			for interested in [true,false]{assert!(matches!(api.community_action(Id(10),Id(1),&Action::RsvpOccurrence{event:Id(30),occurrence:id,interested}).await,Ok(Outcome::Rsvp{..})));}
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::ResetOccurrenceRsvp{event:Id(30),occurrence:id}).await,Ok(Outcome::OccurrenceRsvpReset{..})));
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::RestoreOccurrence{before:before.clone(),exception:id}).await,Ok(Outcome::ExceptionRestored{..})));
			assert!(matches!(api.community_action(Id(10),Id(1),&Action::Status{event:Id(30),status:4}).await,Ok(Outcome::Event(_))));assert!(matches!(api.community_action(Id(10),Id(1),&Action::Delete(Id(30))).await,Ok(Outcome::Deleted(Id(30)))));
			before.exceptions.clear();assert!(matches!(api.community_action(Id(10),Id(1),&Action::EditOccurrence{before,original,canceled:true,start:None,end:None}).await,Err(Failure::Ambiguous)));server.await.unwrap();
		}).await.unwrap();
	}
}
