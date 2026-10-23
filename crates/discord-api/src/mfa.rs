//! Unofficial normal-account MFA. One suspended HTTP request, no durable tickets or proofs.
use crate::{DiscordApi, Failure, RequestContent, ResponsePolicy};
use client_core::mfa::{self as core, Action, Method, Status, Target};
use std::{
	fmt,
	sync::{
		Arc, Mutex,
		atomic::{AtomicU64, Ordering},
	},
	time::Instant,
};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

pub(super) const MAX_WIRE: usize = 64 * 1024;
const MAX_TICKET: usize = 8192;
const MAX_PROOF: usize = 16 * 1024;
type Notify = Arc<dyn Fn(core::Event) -> Result<(), Failure> + Send + Sync>;

tokio::task_local! { pub(super) static OPERATION: Target; }

pub(super) struct Challenge {
	ticket: Zeroizing<String>,
	methods: Vec<Method>,
}
pub(super) fn challenge(bytes: &[u8]) -> Option<Challenge> {
	#[derive(serde::Deserialize)]
	struct Wire {
		code: u32,
		mfa: Details,
	}
	#[derive(serde::Deserialize)]
	struct Details {
		ticket: String,
		methods: Vec<Choice>,
	}
	#[derive(serde::Deserialize)]
	struct Choice {
		#[serde(rename = "type")]
		kind: String,
		#[serde(default)]
		backup_codes_allowed: bool,
	}
	if bytes.len() > MAX_WIRE {
		return None;
	}
	let wire: Wire = serde_json::from_slice(bytes).ok()?;
	let ticket = Zeroizing::new(wire.mfa.ticket);
	if wire.code != 60003
		|| ticket.is_empty()
		|| ticket.capacity() > MAX_TICKET
		|| !ticket.bytes().all(|c| c.is_ascii_graphic())
		|| wire.mfa.methods.is_empty()
		|| wire.mfa.methods.len() > core::MAX_METHODS
	{
		return None;
	}
	let mut methods = Vec::with_capacity(core::MAX_METHODS);
	for choice in wire.mfa.methods {
		let method = match choice.kind.as_str() {
			"totp" => Method::Totp,
			"backup" => Method::Backup,
			"sms" => Method::Sms,
			"password" => Method::Password,
			"webauthn" => Method::Passkey,
			_ => Method::Unsupported,
		};
		if !methods.contains(&method) {
			methods.push(method);
		}
		if method == Method::Totp
			&& choice.backup_codes_allowed
			&& !methods.contains(&Method::BackupViaTotp)
		{
			methods.push(Method::BackupViaTotp);
		}
		if methods.len() > core::MAX_METHODS {
			return None;
		}
	}
	Some(Challenge { ticket, methods })
}

pub(super) struct Proof {
	value: Zeroizing<String>,
	at: Instant,
}
impl Proof {
	fn new(value: String) -> Option<Self> {
		let value = Zeroizing::new(value);
		(!value.is_empty()
			&& value.capacity() <= MAX_PROOF
			&& value.bytes().all(|c| c.is_ascii_graphic()))
		.then_some(Self {
			value,
			at: Instant::now(),
		})
	}
	pub(super) fn remaining(&self) -> std::time::Duration {
		core::LIFETIME.saturating_sub(self.at.elapsed())
	}
	pub(super) fn header(&self) -> Result<reqwest::header::HeaderValue, Failure> {
		if self.at.elapsed() >= core::LIFETIME {
			return Err(Failure::ProtocolAt(
				"Verification expired; start the action again",
			));
		}
		let mut value =
			reqwest::header::HeaderValue::from_str(&self.value).map_err(|_| Failure::Protocol)?;
		value.set_sensitive(true);
		Ok(value)
	}
}
impl fmt::Debug for Proof {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("MfaProof([REDACTED])")
	}
}
struct Pending {
	id: u64,
	ticket: Zeroizing<String>,
	methods: Vec<Method>,
	at: Instant,
	sender: Option<oneshot::Sender<Result<Proof, Failure>>>,
	proof: Option<Proof>,
	busy: bool,
	sms_at: Option<Instant>,
}
#[derive(Default)]
pub(super) struct Broker {
	sequence: AtomicU64,
	pending: Mutex<Option<Pending>>,
	notify: Mutex<Option<Notify>>,
}
impl Broker {
	pub(super) fn set_notify(&self, notify: Notify) -> Result<(), Failure> {
		*self.notify.lock().map_err(|_| Failure::Protocol)? = Some(notify);
		Ok(())
	}
	fn begin(
		&self,
		target: Target,
		challenge: Challenge,
	) -> Result<(u64, oneshot::Receiver<Result<Proof, Failure>>), Failure> {
		let notify = self
			.notify
			.lock()
			.map_err(|_| Failure::Protocol)?
			.clone()
			.ok_or(Failure::ProtocolAt(
				"Discord requires verification; complete this action in Discord",
			))?;
		let id = self
			.sequence
			.fetch_add(1, Ordering::Relaxed)
			.wrapping_add(1);
		if id == 0 {
			return Err(Failure::Protocol);
		}
		let (send, receive) = oneshot::channel();
		{
			let mut pending = self.pending.lock().map_err(|_| Failure::Protocol)?;
			if pending
				.as_ref()
				.is_some_and(|p| p.at.elapsed() < core::LIFETIME)
			{
				return Err(Failure::ProtocolAt(
					"Complete or cancel the current security check first",
				));
			}
			*pending = Some(Pending {
				id,
				ticket: challenge.ticket,
				methods: challenge.methods.clone(),
				at: Instant::now(),
				sender: Some(send),
				proof: None,
				busy: false,
				sms_at: None,
			});
		}
		if let Err(failure) = notify(core::Event::Required(core::Notice {
			id,
			target,
			methods: challenge.methods,
		})) {
			self.cancel(id, failure);
			return Err(failure);
		}
		Ok((id, receive))
	}
	fn cancel(&self, id: u64, failure: Failure) {
		if let Ok(mut pending) = self.pending.lock()
			&& pending.as_ref().is_some_and(|p| p.id == id)
			&& let Some(mut old) = pending.take()
			&& let Some(sender) = old.sender.take()
		{
			let _ = sender.send(Err(failure));
		}
	}
	fn resume_ready(&self, id: u64) -> bool {
		self.pending.lock().is_ok_and(|pending| {
			pending.as_ref().is_some_and(|pending| {
				pending.id == id
					&& pending.at.elapsed() < core::LIFETIME
					&& !pending.busy
					&& pending.proof.is_some()
			})
		})
	}
	pub(super) fn cancel_all(&self) {
		if let Ok(mut pending) = self.pending.lock()
			&& let Some(mut old) = pending.take()
			&& let Some(sender) = old.sender.take()
		{
			let _ = sender.send(Err(Failure::Expired));
		}
	}
	fn ticket(&self, id: u64, method: Option<Method>) -> Result<Zeroizing<String>, Failure> {
		let mut pending = self.pending.lock().map_err(|_| Failure::Protocol)?;
		let pending = pending
			.as_mut()
			.filter(|p| p.id == id && p.at.elapsed() < core::LIFETIME)
			.ok_or(Failure::ProtocolAt(
				"Verification expired; start the action again",
			))?;
		if pending.busy || pending.proof.is_some() {
			return Err(Failure::ProtocolAt(
				"A security check is already in progress",
			));
		}
		if let Some(method) = method {
			if !pending.methods.contains(&method) || !method.supported() {
				return Err(Failure::Protocol);
			}
		} else if !pending.methods.contains(&Method::Sms) {
			return Err(Failure::Protocol);
		}
		if method.is_none()
			&& pending
				.sms_at
				.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(30))
		{
			return Err(Failure::RateLimited);
		}
		if method == Some(Method::Sms) && pending.sms_at.is_none() {
			return Err(Failure::ProtocolAt("Send an SMS code before verifying"));
		}
		pending.busy = true;
		if method.is_none() {
			pending.sms_at = Some(Instant::now());
		}
		Ok(Zeroizing::new(pending.ticket.to_string()))
	}
	fn finish(&self, id: u64, result: Result<Option<Proof>, Failure>) -> Result<Status, Failure> {
		let mut pending = self.pending.lock().map_err(|_| Failure::Protocol)?;
		let pending = pending
			.as_mut()
			.filter(|p| p.id == id && p.at.elapsed() < core::LIFETIME)
			.ok_or(Failure::ProtocolAt(
				"Verification expired; start the action again",
			))?;
		pending.busy = false;
		match result {
			Ok(Some(proof)) => {
				pending.proof = Some(proof);
				Ok(Status::Verified)
			}
			Ok(None) => Ok(Status::SmsSent),
			Err(failure) => Err(failure),
		}
	}
	fn resume(&self, id: u64) -> Result<Status, Failure> {
		let mut slot = self.pending.lock().map_err(|_| Failure::Protocol)?;
		let pending = slot
			.as_mut()
			.filter(|p| p.id == id && p.at.elapsed() < core::LIFETIME)
			.ok_or(Failure::ProtocolAt(
				"Verification expired; start the action again",
			))?;
		if pending.busy || pending.proof.is_none() {
			return Err(Failure::Protocol);
		}
		let mut pending = slot.take().ok_or(Failure::Protocol)?;
		let proof = pending.proof.take().ok_or(Failure::Protocol)?;
		// Publish completion before waking the original continuation. It may immediately
		// encounter another protected route in a multi-route settings operation.
		if let Some(notify) = self.notify.lock().map_err(|_| Failure::Protocol)?.clone() {
			notify(core::Event::Updated {
				id,
				result: Ok(Status::Resumed),
			})?;
		}
		pending
			.sender
			.take()
			.ok_or(Failure::Protocol)?
			.send(Ok(proof))
			.map_err(|_| Failure::ProtocolAt("The original operation is no longer available"))?;
		Ok(Status::Resumed)
	}
}
struct Lease<'a> {
	broker: &'a Broker,
	id: u64,
}
impl Drop for Lease<'_> {
	fn drop(&mut self) {
		self.broker.cancel(
			self.id,
			Failure::ProtocolAt("Verification cancelled; the protected request was not repeated"),
		);
	}
}

impl DiscordApi {
	/// A verifier can publish its result before its worker exits. The broker's proof
	/// transition, rather than JoinHandle completion, authorizes the continuation.
	pub fn mfa_resume_ready(&self, id: u64) -> bool {
		self.mfa.resume_ready(id)
	}
	/// Registers the connection's generation-tagged, bounded event queue. No secrets cross it.
	pub fn set_mfa_notifier(
		&self,
		notify: impl Fn(core::Event) -> Result<(), Failure> + Send + Sync + 'static,
	) -> Result<(), Failure> {
		self.mfa.set_notify(Arc::new(notify))
	}
	pub(super) async fn wait_for_mfa(
		&self,
		target: Target,
		challenge: Challenge,
	) -> Result<Proof, Failure> {
		let (id, receive) = self.mfa.begin(target, challenge)?;
		let _lease = Lease {
			broker: &self.mfa,
			id,
		};
		tokio::time::timeout(core::LIFETIME, receive)
			.await
			.map_err(|_| {
				Failure::ProtocolAt("Verification expired; the protected request was not repeated")
			})?
			.map_err(|_| {
				Failure::ProtocolAt(
					"Verification cancelled; the protected request was not repeated",
				)
			})?
	}
	pub(super) async fn mfa_request(&self, request: core::Request) -> core::Event {
		let id = request.id;
		let result = match request.action {
			Action::Cancel => {
				self.mfa.cancel(
					id,
					Failure::ProtocolAt(
						"Verification cancelled; the protected request was not repeated",
					),
				);
				Ok(Status::Cancelled)
			}
			Action::Resume => self.mfa.resume(id),
			Action::SendSms => match self.mfa.ticket(id, None) {
				Err(failure) => Err(failure),
				Ok(ticket) => {
					#[derive(serde::Serialize)]
					struct Body<'a> {
						ticket: &'a str,
					}
					let body = serde_json::to_vec(&Body { ticket: &ticket })
						.map(Zeroizing::new)
						.map_err(|_| Failure::Protocol);
					let result = match body {
						Ok(body) => self
							.request_secret_limited("/auth/mfa/sms/send", body)
							.await
							.and_then(|bytes| {
								#[derive(serde::Deserialize)]
								struct Sent {
									phone: String,
								}
								let sent: Sent = serde_json::from_slice(&bytes)
									.map_err(|_| Failure::Protocol)?;
								if sent.phone.is_empty() || sent.phone.len() > 64 {
									return Err(Failure::Protocol);
								}
								Ok(None)
							}),
						Err(failure) => Err(failure),
					};
					self.mfa.finish(id, result)
				}
			},
			Action::Verify { method, input } => {
				if !method.accepts(input.expose()) {
					Err(Failure::Protocol)
				} else {
					match self.mfa.ticket(id, Some(method)) {
						Err(failure) => Err(failure),
						Ok(ticket) => {
							#[derive(serde::Serialize)]
							struct Body<'a> {
								ticket: &'a str,
								mfa_type: &'a str,
								data: &'a str,
							}
							#[derive(serde::Deserialize)]
							struct Answer {
								token: String,
							}
							let body = serde_json::to_vec(&Body {
								ticket: &ticket,
								mfa_type: method.wire(),
								data: input.expose(),
							})
							.map(Zeroizing::new)
							.map_err(|_| Failure::Protocol);
							let result = match body {
								Ok(body) => self
									.request_secret_limited("/mfa/finish", body)
									.await
									.and_then(|bytes| {
										let answer: Answer = serde_json::from_slice(&bytes)
											.map_err(|_| Failure::Protocol)?;
										Proof::new(answer.token).map(Some).ok_or(Failure::Protocol)
									}),
								Err(failure) => Err(failure),
							};
							self.mfa.finish(id, result)
						}
					}
				}
			}
		};
		core::Event::Updated { id, result }
	}
	async fn request_secret_limited(
		&self,
		path: &str,
		body: Zeroizing<Vec<u8>>,
	) -> Result<Zeroizing<Vec<u8>>, Failure> {
		if body.len() > MAX_WIRE {
			return Err(Failure::Protocol);
		}
		// The host-owned serialization and response buffers are zeroized. HTTP libraries can
		// retain transient copies; this is not a forensic-erasure claim.
		self.request_with_content(
			reqwest::Method::POST,
			path,
			Some(RequestContent::SecretJson(body)),
			ResponsePolicy::required(MAX_WIRE),
			None,
			None,
		)
		.await
		.map(Zeroizing::new)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::{Command, auth::SessionSecret};
	use serde_json::json;
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::{TcpListener, TcpStream},
	};
	async fn read(stream: &mut TcpStream) -> (String, Vec<u8>) {
		let mut bytes = Vec::new();
		loop {
			let mut chunk = [0u8; 2048];
			let n = stream.read(&mut chunk).await.unwrap();
			assert!(n > 0);
			bytes.extend_from_slice(&chunk[..n]);
			assert!(bytes.len() < 16 * 1024);
			if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
				let end = end + 4;
				let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
				let length = headers
					.lines()
					.find_map(|line| {
						let (name, value) = line.split_once(':')?;
						name.eq_ignore_ascii_case("content-length")
							.then(|| value.trim().parse::<usize>().unwrap())
					})
					.unwrap_or(0);
				if bytes.len() >= end + length {
					return (headers, bytes[end..end + length].to_vec());
				}
			}
		}
	}
	async fn answer(stream: &mut TcpStream, status: &str, body: &str) {
		stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
	}
	fn target() -> Target {
		Target::Admin {
			guild: model::Id(2),
			request: 1,
			action: Box::new(model::server_admin::Action::SetNickname {
				user: model::Id(7),
				nick: "Synthetic name".into(),
			}),
		}
	}
	#[tokio::test]
	async fn mfa_resumes_exact_request_and_preserves_prior_writes_and_unrelated_traffic() {
		tokio::time::timeout(std::time::Duration::from_secs(10),async {
            let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut api=DiscordApi::new(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_MFA_ACCOUNT_TOKEN".into()).unwrap())).unwrap();
            api.base=format!("http://{}",listener.local_addr().unwrap());
            let api=Arc::new(api);
            let (send,mut receive)=tokio::sync::mpsc::channel(4);
            api.set_mfa_notifier(move|notice|send.try_send(notice).map_err(|_|Failure::Capacity)).unwrap();
            let server=tokio::spawn(async move {
                let mut protected=None;
                for index in 0..7 {
                    let (mut stream,_)=listener.accept().await.unwrap();let(headers,body)=read(&mut stream).await;
                    match index {
                        0=>{assert!(headers.starts_with("POST /accepted "));answer(&mut stream,"200 OK","{}").await;},
                        1=>{assert!(headers.starts_with("PATCH /protected "));assert!(!headers.to_lowercase().contains("x-discord-mfa-authorization"));protected=Some((body,headers.lines().find(|line|line.to_lowercase().starts_with("x-audit-log-reason:")).unwrap().to_owned()));answer(&mut stream,"401 Unauthorized",r#"{"code":60003,"mfa":{"ticket":"SYNTHETIC_TICKET","methods":[{"type":"totp","backup_codes_allowed":true},{"type":"sms"}]}}"#).await;},
                        2=>{assert!(headers.starts_with("GET /unrelated "));assert!(!headers.to_lowercase().contains("x-discord-mfa-authorization"));answer(&mut stream,"200 OK","{}").await;},
                        3|4=>{assert!(headers.starts_with("POST /mfa/finish "));assert!(!headers.to_lowercase().contains("x-discord-mfa-authorization"));let body:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["ticket"],"SYNTHETIC_TICKET");assert_eq!(body["mfa_type"],"totp");assert_eq!(body["data"],if index==3 {"000000"}else{"123456"});if index==3{answer(&mut stream,"401 Unauthorized",r#"{"code":60008}"#).await;}else{answer(&mut stream,"200 OK",r#"{"token":"SYNTHETIC_MFA_PROOF"}"#).await;}},
                        5=>{assert!(headers.starts_with("PATCH /protected "));let(previous,reason)=protected.take().unwrap();assert_eq!(body,previous);assert!(headers.lines().any(|line|line==reason));assert!(headers.to_lowercase().contains("x-discord-mfa-authorization: synthetic_mfa_proof"));answer(&mut stream,"200 OK","{}").await;},
                        6=>{assert!(headers.starts_with("GET /readback "));assert!(!headers.to_lowercase().contains("x-discord-mfa-authorization"));answer(&mut stream,"200 OK","{}").await;},
                        _=>unreachable!(),
                    }
                }
            });
            let original=api.clone();
            let operation=tokio::spawn(async move { OPERATION.scope(target(),async {
                original.request(reqwest::Method::POST,"/accepted",Some(json!({"first":"saved once"}))).await?;
                original.request_reason_limited(reqwest::Method::PATCH,"/protected",Some(json!({"name":"Synthetic name"})),4,"Synthetic reason").await?;
                original.request(reqwest::Method::GET,"/readback",None).await
            }).await });
            let Some(core::Event::Required(notice))=receive.recv().await else {panic!("missing action-scoped challenge")};
            assert!(notice.methods.contains(&Method::Totp));assert!(notice.methods.contains(&Method::BackupViaTotp));
            assert!(!api.stopped());assert!(!operation.is_finished());
            api.request(reqwest::Method::GET,"/unrelated",None).await.unwrap();
            let verify=|value:&str|Command::Mfa(Box::new(core::Request{id:notice.id,action:Action::Verify{method:Method::Totp,input:core::Input::new(Method::Totp,value.into()).unwrap()}}));
            assert!(matches!(api.execute(verify("000000")).await,client_core::Event::Mfa(core::Event::Updated{result:Err(Failure::ProtocolAt(_)),..})));
            assert!(!api.stopped());assert!(!operation.is_finished());
            assert!(matches!(api.execute(verify("123456")).await,client_core::Event::Mfa(core::Event::Updated{result:Ok(Status::Verified),..})));
            assert!(!operation.is_finished(),"verification alone cannot resume a stale operation");
            assert!(matches!(api.execute(Command::Mfa(Box::new(core::Request{id:notice.id,action:Action::Resume}))).await,client_core::Event::Mfa(core::Event::Updated{result:Ok(Status::Resumed),..})));
            operation.await.unwrap().unwrap();server.await.unwrap();assert!(!api.stopped());
            assert!(matches!(api.execute(Command::Mfa(Box::new(core::Request{id:notice.id,action:Action::Resume}))).await,client_core::Event::Mfa(core::Event::Updated{result:Err(_),..})));
        }).await.unwrap();
	}

	#[tokio::test]
	async fn channel_edit_challenge_keeps_session_and_resumes_before_verifier_worker_exits() {
		use client_core::channel_actions::{
			Action as ChannelAction, Edit, Event as ChannelEvent, Outcome,
		};
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_MFA_ACCOUNT_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let api = Arc::new(api);
			let (send, mut receive) = tokio::sync::mpsc::channel(4);
			api.set_mfa_notifier(move |notice| {
				send.try_send(notice).map_err(|_| Failure::Capacity)
			})
			.unwrap();
			let server = tokio::spawn(async move {
				let mut rejected = None;
				for index in 0..6 {
					let (mut stream, _) = listener.accept().await.unwrap();
					let (headers, body) = read(&mut stream).await;
					match index {
						0 => {
							assert!(headers.starts_with("POST /accepted "));
							answer(&mut stream, "200 OK", "{}").await;
						}
						1 => {
							assert!(headers.starts_with("GET /channels/3 "));
							answer(
								&mut stream,
								"200 OK",
								r#"{"id":"3","guild_id":"2","type":0,"name":"general","permission_overwrites":[]}"#,
							)
							.await;
						}
						2 => {
							assert!(headers.starts_with("PATCH /channels/3 "));
							assert!(
								!headers
									.to_lowercase()
									.contains("x-discord-mfa-authorization")
							);
							assert_eq!(
								serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
								json!({"name":"updated"})
							);
							rejected = Some(body);
							let challenge = r#"{"code":60003,"mfa":{"ticket":"SYNTHETIC_TICKET","methods":[{"type":"totp"}]}}"#;
							stream.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nx-ratelimit-remaining: 0\r\nx-ratelimit-reset-after: NaN\r\nConnection: close\r\n\r\n{challenge}",challenge.len()).as_bytes()).await.unwrap();
						}
						3 => {
							assert!(headers.starts_with("GET /unrelated "));
							assert!(
								!headers
									.to_lowercase()
									.contains("x-discord-mfa-authorization")
							);
							answer(&mut stream, "200 OK", "{}").await;
						}
						4 => {
							assert!(headers.starts_with("POST /mfa/finish "));
							answer(&mut stream, "200 OK", r#"{"token":"SYNTHETIC_MFA_PROOF"}"#)
								.await;
						}
						5 => {
							assert!(headers.starts_with("PATCH /channels/3 "));
							assert_eq!(Some(body), rejected.take());
							assert!(
								headers
									.to_lowercase()
									.contains("x-discord-mfa-authorization: synthetic_mfa_proof")
							);
							answer(
								&mut stream,
								"200 OK",
								r#"{"id":"3","guild_id":"2","type":0,"name":"updated","permission_overwrites":[]}"#,
							)
							.await;
						}
						_ => unreachable!(),
					}
				}
			});
			let original = api.clone();
			let operation = tokio::spawn(async move {
				original
					.request(
						reqwest::Method::POST,
						"/accepted",
						Some(json!({"saved":"once"})),
					)
					.await
					.unwrap();
				let before = Edit {
					name: "general".into(),
					..Edit::default()
				};
				let after = Edit {
					name: "updated".into(),
					..Edit::default()
				};
				original
					.execute(Command::ChannelAction {
						guild: model::Id(2),
						channel: model::Id(3),
						request: 17,
						action: ChannelAction::Edit { before, after },
					})
					.await
			});
			let Some(core::Event::Required(notice)) = receive.recv().await else {
				panic!("channel edit did not open MFA")
			};
			assert!(matches!(
				notice.target,
				Target::Channel {
					guild: model::Id(2),
					channel: model::Id(3),
					request: 17,
					..
				}
			));
			assert!(!api.stopped());
			api.request(reqwest::Method::GET, "/unrelated", None)
				.await
				.unwrap();
			assert!(!api.mfa_resume_ready(notice.id));
			let verifier = api.clone();
			let id = notice.id;
			let (published, verified) = tokio::sync::oneshot::channel();
			let (release, held) = tokio::sync::oneshot::channel();
			let worker = tokio::spawn(async move {
				let event = verifier
					.execute(Command::Mfa(Box::new(core::Request {
						id,
						action: Action::Verify {
							method: Method::Totp,
							input: core::Input::new(Method::Totp, "123456".into()).unwrap(),
						},
					})))
					.await;
				published.send(event).ok().unwrap();
				held.await.unwrap();
			});
			assert!(matches!(
				verified.await.unwrap(),
				client_core::Event::Mfa(core::Event::Updated {
					result: Ok(Status::Verified),
					..
				})
			));
			assert!(
				!worker.is_finished(),
				"verifier is held after publishing its completion"
			);
			assert!(api.mfa_resume_ready(id));
			assert!(matches!(
				api.execute(Command::Mfa(Box::new(core::Request {
					id,
					action: Action::Resume
				})))
				.await,
				client_core::Event::Mfa(core::Event::Updated {
					result: Ok(Status::Resumed),
					..
				})
			));
			assert!(!api.mfa_resume_ready(id));
			assert!(matches!(
				operation.await.unwrap(),
				client_core::Event::ChannelAction(ChannelEvent::Finished {
					guild: model::Id(2),
					channel: model::Id(3),
					request: 17,
					result: Ok(Outcome::Channel { .. })
				})
			));
			release.send(()).unwrap();
			worker.await.unwrap();
			server.await.unwrap();
			assert!(!api.stopped());
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn suspended_write_uses_updated_proxy_without_reusing_the_previous_route() {
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let origin_url = format!("http://{}",origin.local_addr().unwrap());
			let proxy_url = format!("http://{}",proxy.local_addr().unwrap());
			let (route,updates) = tokio::sync::watch::channel(Some(crate::proxy::ApiProxy::Direct));
			let mut api = DiscordApi::with_proxy(Arc::new(SessionSecret::from_owner_input("SYNTHETIC_MFA_ACCOUNT_TOKEN".into()).unwrap()),updates).unwrap();
			api.base = origin_url.clone();let api = Arc::new(api);
			let (send,mut receive) = tokio::sync::mpsc::channel(4);
			api.set_mfa_notifier(move |notice|send.try_send(notice).map_err(|_|Failure::Capacity)).unwrap();
			let (release,done) = tokio::sync::oneshot::channel::<()>();
			let origin_server = tokio::spawn(async move {
				let (mut stream,_) = origin.accept().await.unwrap();
				let (headers,body) = read(&mut stream).await;
				assert!(headers.starts_with("PATCH /protected "));
				assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(),json!({"name":"Synthetic name"}));
				answer(&mut stream,"401 Unauthorized",r#"{"code":60003,"mfa":{"ticket":"SYNTHETIC_TICKET","methods":[{"type":"totp"}]}}"#).await;
				tokio::select! {
					connection = origin.accept()=>{connection.unwrap();panic!("the retired direct route received another request");},
					_ = done=>{},
				}
			});
			let proxy_server = tokio::spawn(async move {
				for index in 0..3 {
					let (mut stream,_) = proxy.accept().await.unwrap();let(headers,body)=read(&mut stream).await;
					match index {
						0=>{assert!(headers.starts_with(&format!("POST {origin_url}/mfa/finish ")));answer(&mut stream,"200 OK",r#"{"token":"SYNTHETIC_MFA_PROOF"}"#).await;},
						1=>{assert!(headers.starts_with(&format!("PATCH {origin_url}/protected ")));assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(),json!({"name":"Synthetic name"}));assert!(headers.to_lowercase().contains("x-discord-mfa-authorization: synthetic_mfa_proof"));answer(&mut stream,"200 OK","{}").await;},
						_=>{assert!(headers.starts_with(&format!("GET {origin_url}/after ")));assert!(!headers.to_lowercase().contains("x-discord-mfa-authorization"));answer(&mut stream,"200 OK","{}").await;},
					}
				}
			});
			let original=api.clone();
			let operation=tokio::spawn(async move {OPERATION.scope(target(),original.request(reqwest::Method::PATCH,"/protected",Some(json!({"name":"Synthetic name"})))).await});
			let Some(core::Event::Required(notice))=receive.recv().await else {panic!("missing scoped challenge")};
			route.send(Some(crate::proxy::ApiProxy::Url {url:proxy_url})).unwrap();
			assert!(matches!(api.execute(Command::Mfa(Box::new(core::Request {id:notice.id,action:Action::Verify {method:Method::Totp,input:core::Input::new(Method::Totp,"123456".into()).unwrap()}}))).await,client_core::Event::Mfa(core::Event::Updated {result:Ok(Status::Verified),..})));
			assert!(matches!(api.execute(Command::Mfa(Box::new(core::Request {id:notice.id,action:Action::Resume}))).await,client_core::Event::Mfa(core::Event::Updated {result:Ok(Status::Resumed),..})));
			operation.await.unwrap().unwrap();
			api.request(reqwest::Method::GET,"/after",None).await.unwrap();
			release.send(()).unwrap();origin_server.await.unwrap();proxy_server.await.unwrap();
			assert!(!api.stopped());
		}).await.unwrap();
	}
	#[tokio::test]
	async fn unsupported_valid_action_challenge_is_local_but_malformed_401_still_revokes() {
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_MFA_ACCOUNT_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let server = tokio::spawn(async move {
				for index in 0..3 {
					let (mut stream, _) = listener.accept().await.unwrap();
					let _ = read(&mut stream).await;
					match index {
						0 => {
							answer(
								&mut stream,
								"401 Unauthorized",
								r#"{"code":60003,"mfa":{"ticket":"SYNTHETIC_TICKET","methods":[{"type":"webauthn"}]}}"#,
							)
							.await
						}
						1 => answer(&mut stream, "200 OK", "{}").await,
						_ => {
							answer(
								&mut stream,
								"401 Unauthorized",
								r#"{"code":60003,"mfa":{"ticket":"","methods":[]}}"#,
							)
							.await
						}
					}
				}
			});
			assert!(matches!(
				api.request(
					reqwest::Method::PATCH,
					"/unsupported",
					Some(json!({"enabled":true}))
				)
				.await,
				Err(Failure::ProtocolAt(_))
			));
			assert!(!api.stopped());
			api.request(reqwest::Method::GET, "/unrelated", None)
				.await
				.unwrap();
			assert_eq!(
				api.request(
					reqwest::Method::PATCH,
					"/malformed",
					Some(json!({"enabled":true}))
				)
				.await
				.unwrap_err(),
				Failure::Expired
			);
			assert!(api.stopped());
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[tokio::test]
	async fn cancellation_never_repeats_the_rejected_write_and_revocation_still_stops() {
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_MFA_ACCOUNT_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let api = Arc::new(api);
			let (send, mut receive) = tokio::sync::mpsc::channel(2);
			api.set_mfa_notifier(move |event| send.try_send(event).map_err(|_| Failure::Capacity))
				.unwrap();
			let server = tokio::spawn(async move {
				let (mut stream, _) = listener.accept().await.unwrap();
				let (headers, _) = read(&mut stream).await;
				assert!(headers.starts_with("POST /protected "));
				answer(
					&mut stream,
					"401 Unauthorized",
					r#"{"code":60003,"mfa":{"ticket":"SYNTHETIC_TICKET","methods":[{"type":"webauthn"}]}}"#,
				)
				.await;
				let (mut stream, _) = listener.accept().await.unwrap();
				let (headers, _) = read(&mut stream).await;
				assert!(headers.starts_with("GET /revoked "));
				answer(&mut stream, "401 Unauthorized", r#"{"code":0}"#).await;
			});
			let original = api.clone();
			let operation = tokio::spawn(async move {
				OPERATION
					.scope(
						target(),
						original.request(
							reqwest::Method::POST,
							"/protected",
							Some(json!({"id":"7"})),
						),
					)
					.await
			});
			let Some(core::Event::Required(notice)) = receive.recv().await else {
				panic!()
			};
			assert_eq!(notice.methods, vec![Method::Passkey]);
			assert!(matches!(
				api.execute(Command::Mfa(Box::new(core::Request {
					id: notice.id,
					action: Action::Cancel
				})))
				.await,
				client_core::Event::Mfa(core::Event::Updated {
					result: Ok(Status::Cancelled),
					..
				})
			));
			assert!(matches!(
				operation.await.unwrap(),
				Err(Failure::ProtocolAt(_))
			));
			assert!(!api.stopped());
			assert!(matches!(
				api.request(reqwest::Method::GET, "/revoked", None).await,
				Err(Failure::Expired)
			));
			assert!(api.stopped());
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
	#[test]
	fn malformed_challenges_and_secret_debug_output_are_bounded() {
		assert!(
			challenge(br#"{"code":60003,"mfa":{"ticket":"","methods":[{"type":"totp"}]}}"#)
				.is_none()
		);
		assert!(challenge(br#"{"code":60003,"mfa":{"ticket":"ticket","methods":[]}}"#).is_none());
		assert!(challenge(&vec![b' '; MAX_WIRE + 1]).is_none());
		let proof = Proof::new("SYNTHETIC_PROOF".into()).unwrap();
		assert!(format!("{proof:?}").contains("REDACTED"));
		assert!(!format!("{proof:?}").contains("SYNTHETIC"));
		assert!(proof.header().unwrap().is_sensitive());
	}
}
