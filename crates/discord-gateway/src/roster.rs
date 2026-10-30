//! One explicitly authorized, bounded roster refresh. Results never enter the account event queue.
use crate::{Failure, Frame, Id};
use client_core::force_sync_apply::RosterPermit;
use discord_protocol::MemberDto;
use model::Member;
use serde::{
	Deserialize,
	de::{Error, SeqAccess, Visitor},
};
use std::{
	collections::VecDeque,
	fmt,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::{sync::watch, time::Instant};

const QUEUE_ITEMS: usize = 8;
const QUEUE_BYTES: usize = 8 * 1024 * 1024;
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_MEMBERS: usize = 250_000;
const MAX_CHUNKS: usize = MAX_MEMBERS;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
	pub guild: Id,
	pub job: u64,
}
impl Request {
	fn nonce(self) -> String {
		format!("serein-sync-{}", self.job)
	}
}

pub enum Event {
	Chunk {
		request: Request,
		index: usize,
		total: usize,
		members: Vec<Member>,
	},
	Finished(Request),
	Failed {
		request: Request,
		failure: Failure,
	},
}
impl Event {
	fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Chunk { members, .. } => {
					members.capacity() * size_of::<Member>()
						+ members
							.iter()
							.map(|m| m.bytes().saturating_sub(size_of::<Member>()))
							.sum::<usize>()
				}
				_ => 0,
			}
	}
}
#[derive(Default)]
struct Queue {
	request: Option<Request>,
	rows: VecDeque<Event>,
	bytes: usize,
	terminal: Option<Event>,
}

pub struct RosterHandle {
	send: watch::Sender<Option<Request>>,
	queue: Arc<Mutex<Queue>>,
}
#[derive(Clone)]
pub struct RosterInput {
	pub(super) receive: watch::Receiver<Option<Request>>,
	queue: Arc<Mutex<Queue>>,
}
pub fn channel() -> (RosterHandle, RosterInput) {
	let (send, receive) = watch::channel(None);
	let queue = Arc::new(Mutex::new(Queue::default()));
	(
		RosterHandle {
			send,
			queue: queue.clone(),
		},
		RosterInput { receive, queue },
	)
}
impl RosterHandle {
	/// The permit is issued only from current authenticated permission state.
	pub fn start_roster(&self, permit: RosterPermit, job: u64) -> Result<(), Failure> {
		if job == 0 {
			return Err(Failure::Protocol);
		}
		let request = Request {
			guild: permit.guild(),
			job,
		};
		let mut queue = self.queue.lock().map_err(|_| Failure::Busy)?;
		if queue.request == Some(request) {
			return Ok(());
		}
		if queue.request.is_some() {
			return Err(Failure::Busy);
		}
		*queue = Queue {
			request: Some(request),
			..Queue::default()
		};
		self.send.send_replace(Some(request));
		Ok(())
	}
	pub fn cancel(&self) {
		self.send.send_replace(None);
		if let Ok(mut queue) = self.queue.lock() {
			*queue = Queue::default();
		}
	}
	pub fn try_recv(&self) -> Option<Event> {
		let mut queue = self.queue.lock().ok()?;
		if let Some(event) = queue.rows.pop_front() {
			queue.bytes = queue.bytes.saturating_sub(event.bytes());
			return Some(event);
		}
		let terminal = queue.terminal.take();
		if terminal.is_some() {
			queue.request = None;
		}
		terminal
	}
}
impl Drop for RosterHandle {
	fn drop(&mut self) {
		self.cancel();
	}
}
impl RosterInput {
	fn push(&self, request: Request, event: Event) -> bool {
		let Ok(mut queue) = self.queue.lock() else {
			return false;
		};
		if queue.request != Some(request) || queue.terminal.is_some() {
			return false;
		}
		let bytes = event.bytes();
		if queue.rows.len() == QUEUE_ITEMS || queue.bytes.saturating_add(bytes) > QUEUE_BYTES {
			queue.terminal = Some(Event::Failed {
				request,
				failure: Failure::Busy,
			});
			return false;
		}
		queue.bytes += bytes;
		queue.rows.push_back(event);
		true
	}
	fn finish(&self, request: Request, failure: Option<Failure>) {
		if let Ok(mut queue) = self.queue.lock()
			&& queue.request == Some(request)
			&& queue.terminal.is_none()
		{
			queue.terminal = Some(match failure {
				Some(failure) => Event::Failed { request, failure },
				None => Event::Finished(request),
			});
		}
	}
}

#[derive(Deserialize)]
struct Chunk {
	guild_id: Id,
	nonce: Option<String>,
	chunk_index: usize,
	chunk_count: usize,
	#[serde(deserialize_with = "bounded_members")]
	members: Vec<MemberDto>,
}
fn bounded_members<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MemberDto>, D::Error> {
	struct Members;
	impl<'de> Visitor<'de> for Members {
		type Value = Vec<MemberDto>;
		fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			f.write_str("at most 1000 members")
		}
		fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
			let mut members = Vec::new();
			while let Some(member) = seq.next_element()? {
				if members.len() == 1000 {
					return Err(A::Error::custom("Roster chunk exceeds 1000 members"));
				}
				members.push(member);
			}
			Ok(members)
		}
	}
	d.deserialize_seq(Members)
}
struct Active {
	request: Request,
	queued: bool,
	sent: Option<Instant>,
	deadline: Option<Instant>,
	chunks: Vec<bool>,
	members: usize,
	users: Vec<Id>,
}
#[derive(Default)]
pub(super) struct Worker {
	input: Option<RosterInput>,
	active: Option<Active>,
}
impl Worker {
	pub fn new(input: Option<RosterInput>) -> Self {
		Self {
			input,
			active: None,
		}
	}
	pub fn update(&mut self, request: Option<Request>) {
		if self.active.as_ref().map(|active| active.request) == request {
			return;
		}
		self.active = request.map(|request| Active {
			request,
			queued: false,
			sent: None,
			deadline: None,
			chunks: vec![],
			members: 0,
			users: vec![],
		});
	}
	pub fn pending(&self) -> bool {
		self.active.as_ref().is_some_and(|active| !active.queued)
	}
	pub fn frame(&self) -> Option<Frame> {
		let active = self.active.as_ref().filter(|active| !active.queued)?;
		Some(Frame::Text(serde_json::json!({"op":8,"d":{"guild_id":active.request.guild.to_string(),"query":"","limit":0,"presences":false,"nonce":active.request.nonce()}}).to_string().into()))
	}
	pub fn queued(&mut self) {
		if let Some(active) = &mut self.active {
			active.queued = true;
		}
	}
	pub fn sent(&mut self, guild: Id, job: u64, sent: Instant) {
		if let Some(active) = &mut self.active
			&& active.request == (Request { guild, job })
			&& active.sent.is_none()
		{
			active.sent = Some(sent);
			active.deadline = Some(sent + IDLE_TIMEOUT);
		}
	}
	pub fn deadline(&self) -> Option<Instant> {
		let active = self.active.as_ref()?;
		Some(active.deadline?.min(active.sent? + TOTAL_TIMEOUT))
	}
	pub fn expire(&mut self) {
		if self
			.deadline()
			.is_some_and(|deadline| deadline <= Instant::now())
		{
			self.fail(Failure::ProtocolAt(
				"Member roster refresh timed out; retry to continue",
			));
		}
	}
	pub fn fail(&mut self, failure: Failure) {
		if let Some(active) = self.active.take()
			&& let Some(input) = &self.input
		{
			input.finish(active.request, Some(failure));
		}
	}
	pub fn limited(&mut self, guild: Option<Id>, nonce: Option<&str>) {
		if self.active.as_ref().is_some_and(|active| {
			guild.is_none_or(|guild| guild == active.request.guild)
				&& nonce.is_none_or(|nonce| nonce == active.request.nonce())
		}) {
			self.fail(Failure::RateLimited);
		}
	}
	/// Returns true only for a matching refresh chunk; ordinary searches keep their own parser.
	pub fn chunk(&mut self, bytes: &[u8]) -> bool {
		let Some(active) = &mut self.active else {
			return false;
		};
		// Inspect a small scope before parsing the potentially large member list.
		#[derive(Deserialize)]
		struct Scope {
			guild_id: Id,
			nonce: Option<String>,
		}
		let Ok(scope) = discord_protocol::decode::<Scope>(bytes) else {
			return false;
		};
		if scope.guild_id != active.request.guild
			|| scope.nonce.as_deref() != Some(active.request.nonce().as_str())
		{
			return false;
		}
		if bytes.len() > CHUNK_BYTES {
			self.fail(Failure::ResponseLimit);
			return true;
		}
		let Ok(chunk) = discord_protocol::decode::<Chunk>(bytes) else {
			self.fail(Failure::Protocol);
			return true;
		};
		if chunk.guild_id != active.request.guild
			|| chunk.nonce != scope.nonce
			|| chunk.chunk_count == 0
			|| chunk.chunk_count > MAX_CHUNKS
			|| chunk.chunk_index >= chunk.chunk_count
			|| !active.chunks.is_empty() && active.chunks.len() != chunk.chunk_count
		{
			self.fail(Failure::ResponseLimit);
			return true;
		}
		if active.chunks.is_empty() {
			active.chunks.resize(chunk.chunk_count, false);
		}
		if active.chunks[chunk.chunk_index] {
			return true;
		}
		let mut members = Vec::with_capacity(chunk.members.len());
		let mut bytes = 0_usize;
		let mut seen = std::collections::BTreeSet::new();
		for member in chunk.members {
			let Some(member) = member.into_guild_member(active.request.guild) else {
				self.fail(Failure::Protocol);
				return true;
			};
			bytes = bytes.saturating_add(member.bytes());
			if !member.valid()
				|| bytes > CHUNK_BYTES
				|| !seen.insert(member.user.id)
				|| active.users.binary_search(&member.user.id).is_ok()
			{
				self.fail(Failure::ResponseLimit);
				return true;
			}
			members.push(member);
		}
		if active.members.saturating_add(members.len()) > MAX_MEMBERS {
			self.fail(Failure::ResponseLimit);
			return true;
		}
		active.members += members.len();
		active.users.reserve_exact(members.len());
		active
			.users
			.extend(members.iter().map(|member| member.user.id));
		active.users.sort_unstable();
		active.chunks[chunk.chunk_index] = true;
		active.deadline = Some(Instant::now() + IDLE_TIMEOUT);
		let request = active.request;
		let complete = active.chunks.iter().all(|seen| *seen);
		if let Some(input) = &self.input {
			if !input.push(
				request,
				Event::Chunk {
					request,
					index: chunk.chunk_index,
					total: chunk.chunk_count,
					members,
				},
			) {
				self.active = None;
				return true;
			}
			if complete {
				input.finish(request, None);
				self.active = None;
			}
		}
		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	fn fixture() -> (RosterHandle, Worker, Request) {
		let (handle, input) = channel();
		let request = Request {
			guild: Id(9),
			job: 7,
		};
		handle.queue.lock().unwrap().request = Some(request);
		let mut worker = Worker::new(Some(input));
		worker.update(Some(request));
		(handle, worker, request)
	}
	fn chunk(request: Request, index: usize, total: usize, count: usize) -> Vec<u8> {
		json!({"guild_id":request.guild,"nonce":request.nonce(),"chunk_index":index,"chunk_count":total,
			"members":(0..count).map(|i|json!({"user":{"id":(index*1000+i+1).to_string(),"username":"Synthetic"},"nick":"Current nickname","roles":[]})).collect::<Vec<_>>()}).to_string().into_bytes()
	}
	#[test]
	fn thousand_member_chunks_stream_out_of_order_without_account_events() {
		let (handle, mut worker, request) = fixture();
		let frame = worker.frame().unwrap();
		let Frame::Text(text) = frame else { panic!() };
		let packet: serde_json::Value = serde_json::from_str(&text).unwrap();
		assert_eq!(packet["d"]["query"], "");
		assert_eq!(packet["d"]["limit"], 0);
		assert_eq!(packet["d"]["presences"], false);
		assert!(worker.chunk(&chunk(request, 1, 2, 1000)));
		assert!(
			worker.chunk(&chunk(request, 1, 2, 1000)),
			"duplicate index is ignored"
		);
		assert!(worker.chunk(&chunk(request, 0, 2, 1000)));
		for index in [1, 0] {
			let Some(Event::Chunk {
				members,
				index: actual,
				..
			}) = handle.try_recv()
			else {
				panic!()
			};
			assert_eq!(actual, index);
			assert_eq!(members.len(), 1000);
			assert_eq!(members[0].nick.as_deref(), Some("Current nickname"));
		}
		assert!(matches!(handle.try_recv(), Some(Event::Finished(done)) if done == request));
		assert!(handle.try_recv().is_none());
	}
	#[test]
	fn stale_chunks_and_cancelled_results_cannot_repopulate_queue() {
		let (handle, mut worker, request) = fixture();
		assert!(!worker.chunk(&chunk(Request { job: 6, ..request }, 0, 1, 1)));
		assert!(!worker.chunk(&chunk(
			Request {
				guild: Id(10),
				..request
			},
			0,
			1,
			1
		)));
		handle.cancel();
		worker.update(None);
		assert!(!worker.chunk(&chunk(request, 0, 1, 1)));
		assert!(handle.try_recv().is_none());
	}
	#[test]
	fn queue_exhaustion_is_a_roster_failure_after_accepted_chunks() {
		let (handle, mut worker, request) = fixture();
		for index in 0..9 {
			assert!(worker.chunk(&chunk(request, index, 10, 1)));
		}
		for _ in 0..QUEUE_ITEMS {
			assert!(matches!(handle.try_recv(), Some(Event::Chunk { .. })));
		}
		assert!(matches!(
			handle.try_recv(),
			Some(Event::Failed {
				failure: Failure::Busy,
				..
			})
		));
		assert!(!worker.pending());
	}
	#[test]
	fn inconsistent_oversized_and_missing_chunks_fail_explicitly() {
		let (handle, mut worker, request) = fixture();
		assert!(worker.chunk(&chunk(request, 0, MAX_CHUNKS + 1, 1)));
		assert!(matches!(
			handle.try_recv(),
			Some(Event::Failed {
				failure: Failure::ResponseLimit,
				..
			})
		));
		let (handle, mut worker, request) = fixture();
		worker.queued();
		assert!(
			worker.deadline().is_none(),
			"rate admission does not start output timeout"
		);
		worker.sent(
			request.guild,
			request.job,
			Instant::now() - IDLE_TIMEOUT - Duration::from_secs(1),
		);
		worker.expire();
		assert!(matches!(
			handle.try_recv(),
			Some(Event::Failed {
				failure: Failure::ProtocolAt(_),
				..
			})
		));
		let (handle, mut worker, request) = fixture();
		assert!(worker.chunk(&chunk(request, 0, 2, 1)));
		assert!(worker.chunk(&chunk(request, 1, 3, 1)));
		assert!(matches!(handle.try_recv(), Some(Event::Chunk { .. })));
		assert!(matches!(handle.try_recv(), Some(Event::Failed { .. })));
	}

	#[tokio::test]
	async fn socket_refresh_uses_shared_writer_and_bypasses_search_result_limit() {
		use crate::tests::{packet, ready, send};
		use futures_util::SinkExt;
		use tokio_tungstenite::{
			accept_async,
			tungstenite::protocol::{CloseFrame, frame::coding::CloseCode},
		};
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
			let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
			let (handle, input) = channel();
			let request = Request {
				guild: Id(9),
				job: 42,
			};
			handle.queue.lock().unwrap().request = Some(request);
			handle.send.send_replace(Some(request));
			let account_search_events = std::sync::atomic::AtomicUsize::new(0);
			let client = crate::run_inner(
				Arc::new(
					client_core::auth::SessionSecret::from_owner_input(
						"synthetic-roster-token".into(),
					)
					.unwrap(),
				),
				"wss://gateway.discord.gg/".into(),
				watch::channel(None).1,
				tokio::sync::mpsc::channel(1).1,
				Some(crate::ActivityInput {
					receiver: watch::channel(None).1,
					own_presence: watch::channel(Default::default()).1,
					member_queries: watch::channel(Default::default()).1,
					member_watches: None,
					roster: Some(input),
					spotify: watch::channel(None).1,
					observe: &|_| Ok(()),
				}),
				|event| {
					if matches!(event, client_core::Event::MemberSearch { .. }) {
						account_search_events.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
					}
					Ok(())
				},
				Some(&endpoint),
			);
			let server = async {
				let (stream, _) = listener.accept().await.unwrap();
				let mut socket = accept_async(stream).await.unwrap();
				send(
					&mut socket,
					json!({"op":10,"d":{"heartbeat_interval":30000}}),
				)
				.await;
				assert_eq!(packet(&mut socket).await["op"], 2);
				send(&mut socket, ready(1, "synthetic-roster")).await;
				loop {
					let outgoing = packet(&mut socket).await;
					if outgoing["op"] == 8 {
						assert_eq!(outgoing["d"]["limit"], 0);
						assert_eq!(outgoing["d"]["nonce"], request.nonce());
						break;
					}
					if outgoing["op"] == 1 {
						send(&mut socket, json!({"op":11,"d":null})).await;
					}
				}
				let data: serde_json::Value =
					serde_json::from_slice(&chunk(request, 0, 1, 200)).unwrap();
				send(
					&mut socket,
					json!({"op":0,"t":"GUILD_MEMBERS_CHUNK","s":2,"d":data}),
				)
				.await;
				loop {
					if let Some(event) = handle.try_recv() {
						match event {
							Event::Chunk { members, .. } => assert_eq!(members.len(), 200),
							Event::Finished(done) => {
								assert_eq!(done, request);
								break;
							}
							Event::Failed { failure, .. } => panic!("roster failed: {failure}"),
						}
					}
					tokio::task::yield_now().await;
				}
				socket
					.send(Frame::Close(Some(CloseFrame {
						code: CloseCode::from(4004),
						reason: "".into(),
					})))
					.await
					.unwrap();
			};
			let (result, ()) = tokio::join!(client, server);
			assert_eq!(result, Err(Failure::Expired));
			assert_eq!(
				account_search_events.load(std::sync::atomic::Ordering::Relaxed),
				0
			);
		})
		.await
		.unwrap();
	}
}
