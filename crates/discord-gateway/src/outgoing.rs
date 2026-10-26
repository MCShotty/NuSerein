//! One bounded socket writer. Admission never awaits the network on the reader's task.
use crate::{Failure, Frame};
use futures_util::{Sink, SinkExt};
use model::Id;
use std::{
	collections::{BTreeMap, VecDeque},
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::{
	sync::{Notify, watch},
	task::JoinHandle,
	time::{Instant, sleep_until, timeout},
};

pub(super) const PACKETS: usize = 16;
pub(super) const BYTES: usize = 64 * 1024;
const WINDOW: Duration = Duration::from_secs(60);
const EVENTS: usize = 120;
const COOL_DOWN_KEYS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
	Presence,
	Subscription(Id, bool),
	CancelSubscription(Id),
	Query(crate::member_search::Purpose),
}
#[derive(Clone, Copy, Debug)]
struct Meta {
	opcode: Option<u8>,
	guild: Option<Id>,
	nonce: Option<u64>,
	key: Option<Key>,
	heartbeat: bool,
	critical: bool,
}
impl Meta {
	fn of(frame: &Frame) -> Self {
		let mut meta = Self {
			opcode: None,
			guild: None,
			nonce: None,
			key: None,
			heartbeat: false,
			critical: true,
		};
		let Frame::Text(text) = frame else {
			return meta;
		};
		let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
			return meta;
		};
		meta.opcode = value["op"].as_u64().and_then(|v| u8::try_from(v).ok());
		match meta.opcode {
			Some(1) => meta.heartbeat = true,
			Some(3) => {
				meta.key = Some(Key::Presence);
				meta.critical = false;
			}
			Some(8) => {
				meta.guild = value["d"]["guild_id"]
					.as_str()
					.and_then(|v| v.parse::<u64>().ok())
					.map(Id);
				meta.nonce = value["d"]["nonce"]
					.as_str()
					.and_then(|v| v.parse::<u64>().ok());
				meta.critical = false;
			}
			Some(37) => {
				if let Some((guild, setting)) = value["d"]["subscriptions"]
					.as_object()
					.and_then(|v| v.iter().next())
				{
					meta.guild = guild.parse::<u64>().ok().map(Id);
					let empty = setting["channels"]
						.as_object()
						.is_none_or(serde_json::Map::is_empty)
						&& setting["thread_member_lists"]
							.as_array()
							.is_none_or(Vec::is_empty);
					// Keep the initial guild subscription before its ranges; only viewport replacements coalesce.
					if setting["typing"] == true {
						meta.key = meta.guild.map(|id| Key::Subscription(id, empty));
						meta.critical = false;
					} else {
						meta.key = meta.guild.map(Key::CancelSubscription);
					}
				}
			}
			_ => {}
		}
		meta
	}
}
struct Packet {
	frame: Frame,
	meta: Meta,
	bytes: usize,
	serial: u64,
}
struct State {
	queue: VecDeque<Packet>,
	bytes: usize,
	in_flight: bool,
	serial: u64,
	subscription_serial: u64,
	heartbeat_serial: u64,
	cooldowns: BTreeMap<(u8, Option<Id>), Instant>,
	blocked_until: Option<Instant>,
}
impl Default for State {
	fn default() -> Self {
		Self {
			queue: VecDeque::with_capacity(PACKETS),
			bytes: 0,
			in_flight: false,
			serial: 0,
			subscription_serial: 0,
			heartbeat_serial: 0,
			cooldowns: BTreeMap::new(),
			blocked_until: None,
		}
	}
}
#[derive(Clone, Copy, Default)]
pub(super) struct Progress {
	pub heartbeat: Option<(u64, Instant)>,
	pub query: [Option<(Id, u64, Instant)>; 2],
	pub subscription: Option<(u64, Instant)>,
	pub failed: bool,
	pub capacity: bool,
}
struct FailureOnExit(watch::Sender<Progress>);
impl Drop for FailureOnExit {
	fn drop(&mut self) {
		self.0.send_modify(|progress| progress.failed = true);
	}
}
pub(super) struct Outgoing {
	state: Arc<Mutex<State>>,
	wake: Arc<Notify>,
	pub progress: watch::Receiver<Progress>,
	report: watch::Sender<Progress>,
	task: JoinHandle<()>,
}
impl Drop for Outgoing {
	fn drop(&mut self) {
		self.task.abort();
	}
}
struct Budget {
	sent: VecDeque<Instant>,
	reserved: usize,
}
impl Budget {
	fn new(interval: Duration) -> Self {
		let heartbeats = WINDOW.as_millis().div_ceil(interval.as_millis().max(1)) as usize;
		Self {
			sent: VecDeque::with_capacity(EVENTS),
			reserved: (heartbeats + 12).min(EVENTS - 1),
		}
	}
	fn earliest(&mut self, now: Instant, meta: Meta) -> Instant {
		while self
			.sent
			.front()
			.is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW)
		{
			self.sent.pop_front();
		}
		if meta.opcode.is_none() {
			return now;
		} // WebSocket Pong is not a Gateway event.
		let limit = if meta.heartbeat {
			EVENTS
		} else if meta.critical {
			EVENTS - self.reserved.saturating_sub(8)
		} else {
			EVENTS - self.reserved
		};
		if self.sent.len() < limit {
			now
		} else {
			self.sent[self.sent.len() - limit] + WINDOW
		}
	}
}
impl Outgoing {
	pub fn start<S>(mut sink: S, interval: Duration) -> Self
	where
		S: Sink<Frame> + Unpin + Send + 'static,
		S::Error: Send,
	{
		let state = Arc::new(Mutex::new(State::default()));
		let wake = Arc::new(Notify::new());
		let (report, progress) = watch::channel(Progress::default());
		let worker_report = report.clone();
		let worker_state = state.clone();
		let worker_wake = wake.clone();
		let task = tokio::spawn(async move {
			let report = worker_report;
			let _failure_on_exit = FailureOnExit(report.clone());
			let mut budget = Budget::new(interval);
			// Identify/Resume was sent before the writer was installed.
			budget.sent.push_back(Instant::now());
			loop {
				let notified = worker_wake.notified();
				let now = Instant::now();
				let selected = {
					let Ok(mut state) = worker_state.lock() else {
						report.send_modify(|p| p.failed = true);
						return;
					};
					state.cooldowns.retain(|_, until| *until > now);
					let mut next = None;
					let mut deadline = None;
					for (index, packet) in state.queue.iter().enumerate() {
						let mut at = budget.earliest(now, packet.meta);
						if !packet.meta.critical
							&& let Some(until) = state.blocked_until
						{
							at = at.max(until);
						}
						if let Some(opcode) = packet.meta.opcode {
							let until = state
								.cooldowns
								.get(&(opcode, packet.meta.guild))
								.copied()
								.into_iter()
								.chain(state.cooldowns.get(&(opcode, None)).copied())
								.chain(state.cooldowns.iter().filter_map(
									|((limited, _), until)| {
										(packet.meta.guild.is_none() && *limited == opcode)
											.then_some(*until)
									},
								))
								.max();
							if let Some(until) = until {
								at = at.max(until);
							}
						}
						if at <= now && (next.is_none() || packet.meta.heartbeat) {
							next = Some(index);
							if packet.meta.heartbeat {
								break;
							}
						}
						deadline = Some(deadline.map_or(at, |previous: Instant| previous.min(at)));
					}
					match next {
						Some(index) => {
							state.in_flight = true;
							Some(Ok(state.queue.remove(index).unwrap()))
						}
						None => deadline.map(Err),
					}
				};
				let packet = match selected {
					Some(Ok(packet)) => packet,
					Some(Err(at)) => {
						tokio::select! { _=notified => {}, _=sleep_until(at)=>{} };
						continue;
					}
					None => {
						notified.await;
						continue;
					}
				};
				if !matches!(
					timeout(Duration::from_secs(5), sink.send(packet.frame)).await,
					Ok(Ok(()))
				) {
					report.send_modify(|p| p.failed = true);
					return;
				}
				let sent = Instant::now();
				if packet.meta.opcode.is_some() {
					budget.sent.push_back(sent);
				}
				if let Ok(mut state) = worker_state.lock() {
					state.bytes = state.bytes.saturating_sub(packet.bytes);
					state.in_flight = false;
					if packet.meta.opcode == Some(8) {
						let until = sent + Duration::from_secs(30);
						if state.cooldowns.len() < COOL_DOWN_KEYS
							|| state.cooldowns.contains_key(&(8, packet.meta.guild))
						{
							state
								.cooldowns
								.entry((8, packet.meta.guild))
								.and_modify(|at| *at = (*at).max(until))
								.or_insert(until);
						} else {
							state.blocked_until =
								Some(state.blocked_until.map_or(until, |at| at.max(until)));
						}
					}
				}
				report.send_modify(|p| {
					if packet.meta.heartbeat {
						p.heartbeat = Some((packet.serial, sent));
					}
					if packet.meta.opcode == Some(37) {
						p.subscription = Some((packet.serial, sent));
					}
					if let (Some(guild), Some(nonce)) = (packet.meta.guild, packet.meta.nonce) {
						p.query.rotate_right(1);
						p.query[0] = Some((guild, nonce, sent));
					}
				});
			}
		});
		Self {
			state,
			wake,
			progress,
			report,
			task,
		}
	}
	pub fn is_finished(&self) -> bool {
		self.task.is_finished()
	}
	pub fn heartbeat_ticket(&self) -> u64 {
		self.state.lock().map(|s| s.heartbeat_serial).unwrap_or(0)
	}
	pub fn occupancy(&self) -> (usize, usize) {
		self.state
			.lock()
			.map(|s| (s.queue.len() + usize::from(s.in_flight), s.bytes))
			.unwrap_or((PACKETS, BYTES))
	}
	pub fn optional_capacity(&self, count: usize) -> bool {
		let (items, bytes) = self.occupancy();
		items + count <= PACKETS - 4 && bytes <= BYTES - 24 * 1024
	}
	pub fn control_capacity(&self) -> bool {
		let (items, bytes) = self.occupancy();
		// A Join can enqueue Sync, Join and channel-info. Keep its complete batch
		// below the critical limit, with the final slot reserved for a heartbeat.
		items <= PACKETS - 4 && bytes <= BYTES - 12 * 1024
	}
	pub fn subscription_ticket(&self) -> u64 {
		self.state
			.lock()
			.map(|s| s.subscription_serial)
			.unwrap_or(0)
	}
	pub fn clear_queries(&self, changed: u8) {
		if let Ok(mut state) = self.state.lock() {
			let mut removed = 0;
			state.queue.retain(|packet| {
				if matches!(packet.meta.key,Some(Key::Query(purpose)) if changed&(1<<purpose.slot())!=0)
				{
					removed += packet.bytes;
					false
				} else {
					true
				}
			});
			state.bytes -= removed;
		}
		self.wake.notify_one();
	}
	pub fn clear_subscriptions(&self) {
		if let Ok(mut state) = self.state.lock() {
			let mut removed = 0;
			state.queue.retain(|p| {
				if matches!(p.meta.key, Some(Key::Subscription(_, false))) {
					removed += p.bytes;
					false
				} else {
					true
				}
			});
			state.bytes = state.bytes.saturating_sub(removed);
		}
		self.wake.notify_one();
	}
	/// Queue admission only; success never claims that Discord accepted the action.
	pub async fn send(&self, frame: Frame) -> Result<(), Failure> {
		self.enqueue(frame, None)
	}
	pub async fn send_query(&self, query: crate::member_search::Query) -> Result<(), Failure> {
		self.enqueue(query.frame, Some(Key::Query(query.purpose)))
	}
	fn enqueue(&self, frame: Frame, key: Option<Key>) -> Result<(), Failure> {
		if self.progress.borrow().failed {
			return Err(Failure::Network);
		}
		let mut meta = Meta::of(&frame);
		if key.is_some() {
			meta.key = key;
		}
		let bytes = frame.len() + std::mem::size_of::<Packet>();
		let mut state = self.state.lock().map_err(|_| Failure::Network)?;
		if let Some(Key::CancelSubscription(guild)) = meta.key {
			let mut removed = 0;
			state.queue.retain(|packet| {
				if matches!(packet.meta.key,Some(Key::Subscription(id,_)) if id==guild) {
					removed += packet.bytes;
					false
				} else {
					true
				}
			});
			state.bytes -= removed;
		}
		if let Some(key) = meta.key
			&& let Some(index) = state.queue.iter().position(|p| p.meta.key == Some(key))
		{
			let old = state.queue.remove(index).unwrap();
			state.bytes -= old.bytes;
		}

		let limit = if meta.heartbeat {
			PACKETS
		} else if meta.critical {
			PACKETS - 1
		} else {
			PACKETS - 4
		};
		let reserve = if meta.critical { 0 } else { 16 * 1024 };
		if state.queue.len() + usize::from(state.in_flight) >= limit
			|| bytes > BYTES.saturating_sub(state.bytes + reserve)
		{
			self.report.send_modify(|p| {
				p.failed = true;
				p.capacity = true;
			});
			return Err(Failure::Network);
		}

		state.serial = state.serial.wrapping_add(1);
		let serial = state.serial;
		if meta.heartbeat {
			state.heartbeat_serial = serial;
		}
		if meta.opcode == Some(37) {
			state.subscription_serial = serial;
		}
		state.bytes += bytes;
		state.queue.push_back(Packet {
			frame,
			meta,
			bytes,
			serial,
		});
		drop(state);
		self.wake.notify_one();
		Ok(())
	}
	pub fn cooldown(&self, opcode: u8, guild: Option<Id>, duration: Duration) {
		if let Ok(mut state) = self.state.lock() {
			let now = Instant::now();
			let until = now + duration;
			state.cooldowns.retain(|_, deadline| *deadline > now);
			if state.cooldowns.len() < COOL_DOWN_KEYS
				|| state.cooldowns.contains_key(&(opcode, guild))
			{
				state
					.cooldowns
					.entry((opcode, guild))
					.and_modify(|at| *at = (*at).max(until))
					.or_insert(until);
			} else {
				state.blocked_until = Some(state.blocked_until.map_or(until, |at| at.max(until)));
			}
		}
		self.wake.notify_one();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{
		pin::Pin,
		sync::atomic::{AtomicBool, Ordering},
		task::{Context, Poll, Waker},
	};
	type Wake = Arc<Mutex<Option<Waker>>>;
	type Frames = Arc<Mutex<Vec<Frame>>>;
	struct Controlled {
		ready: Arc<AtomicBool>,
		wake: Wake,
		frames: Frames,
	}
	impl Sink<Frame> for Controlled {
		type Error = ();
		fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
			if self.ready.load(Ordering::Acquire) {
				Poll::Ready(Ok(()))
			} else {
				*self.wake.lock().unwrap() = Some(cx.waker().clone());
				Poll::Pending
			}
		}
		fn start_send(self: Pin<&mut Self>, frame: Frame) -> Result<(), ()> {
			self.frames.lock().unwrap().push(frame);
			Ok(())
		}
		fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), ()>> {
			Poll::Ready(Ok(()))
		}
		fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), ()>> {
			Poll::Ready(Ok(()))
		}
	}
	fn fixture() -> (Controlled, Arc<AtomicBool>, Wake, Frames) {
		let ready = Arc::new(AtomicBool::new(false));
		let wake = Arc::new(Mutex::new(None));
		let frames = Arc::new(Mutex::new(Vec::new()));
		(
			Controlled {
				ready: ready.clone(),
				wake: wake.clone(),
				frames: frames.clone(),
			},
			ready,
			wake,
			frames,
		)
	}
	fn frame(value: serde_json::Value) -> Frame {
		Frame::Text(value.to_string().into())
	}
	#[test]
	fn rolling_budget_reserves_heartbeat_and_control_headroom() {
		let now = Instant::now();
		let mut budget = Budget::new(Duration::from_secs(1));
		let normal = Meta::of(&frame(serde_json::json!({"op":3,"d":{}})));
		for _ in 0..(EVENTS - budget.reserved) {
			assert_eq!(budget.earliest(now, normal), now);
			budget.sent.push_back(now);
		}
		assert_eq!(budget.earliest(now, normal), now + WINDOW);
		let heartbeat = Meta::of(&frame(serde_json::json!({"op":1,"d":null})));
		for _ in budget.sent.len()..EVENTS {
			assert_eq!(budget.earliest(now, heartbeat), now);
			budget.sent.push_back(now);
		}
		assert_eq!(budget.earliest(now, heartbeat), now + WINDOW);
		assert_eq!(budget.earliest(now + WINDOW, normal), now + WINDOW);
	}
	#[tokio::test]
	async fn blocked_writer_does_not_block_owner_and_only_reports_actual_heartbeat_sends() {
		let (sink, ready, wake, frames) = fixture();
		let mut writer = Outgoing::start(sink, Duration::from_secs(45));
		writer
			.send(frame(serde_json::json!({"op":3,"d":{"status":"online"}})))
			.await
			.unwrap();
		tokio::task::yield_now().await;
		writer
			.send(frame(serde_json::json!({"op":1,"d":42})))
			.await
			.unwrap();
		assert!(writer.progress.borrow().heartbeat.is_none());
		// The owner's task remains usable while the socket write is deliberately stuck.
		for _ in 0..5 {
			tokio::task::yield_now().await;
		}
		assert!(frames.lock().unwrap().is_empty());
		ready.store(true, Ordering::Release);
		if let Some(wake) = wake.lock().unwrap().take() {
			wake.wake();
		}
		timeout(Duration::from_secs(1), async {
			loop {
				writer.progress.changed().await.unwrap();
				if writer.progress.borrow().heartbeat.is_some() {
					break;
				}
			}
		})
		.await
		.unwrap();
		assert!(writer.progress.borrow().heartbeat.unwrap().1 <= Instant::now());
		assert_eq!(frames.lock().unwrap().len(), 2);
	}
	#[tokio::test]
	async fn pending_presence_and_viewports_coalesce_without_losing_initial_subscription() {
		let (sink, ready, wake, frames) = fixture();
		let mut writer = Outgoing::start(sink, Duration::from_secs(45));
		for value in 0..20 {
			writer
				.send(frame(serde_json::json!({"op":3,"d":{"value":value}})))
				.await
				.unwrap();
		}
		assert_eq!(writer.occupancy().0, 1);
		writer
			.send(crate::subscription_packet(Id(2), true, None, false))
			.await
			.unwrap();
		writer
			.send(crate::subscription_packet(
				Id(2),
				true,
				Some((Id(3), &[[0, 99]])),
				false,
			))
			.await
			.unwrap();
		writer.clear_subscriptions();
		writer
			.send(crate::subscription_packet(
				Id(2),
				true,
				Some((Id(3), &[[100, 199]])),
				false,
			))
			.await
			.unwrap();
		assert_eq!(writer.occupancy().0, 3);
		ready.store(true, Ordering::Release);
		if let Some(wake) = wake.lock().unwrap().take() {
			wake.wake();
		}
		timeout(Duration::from_secs(1), async {
			while writer.occupancy().0 > 0 {
				writer.progress.changed().await.unwrap();
			}
		})
		.await
		.unwrap();
		let frames = frames.lock().unwrap();
		assert_eq!(
			serde_json::from_str::<serde_json::Value>(frames[0].to_text().unwrap()).unwrap()["d"]["value"],
			19
		);
		assert_eq!(
			serde_json::from_str::<serde_json::Value>(frames[1].to_text().unwrap()).unwrap()["d"]["subscriptions"]
				["2"]["channels"],
			serde_json::json!({})
		);
		assert_eq!(
			serde_json::from_str::<serde_json::Value>(frames[2].to_text().unwrap()).unwrap()["d"]["subscriptions"]
				["2"]["channels"],
			serde_json::json!({"3":[[100,199]]})
		);
	}
	#[tokio::test]
	async fn queue_limits_preserve_critical_reserve_and_never_discard_a_stop() {
		let (sink, _, _, _) = fixture();
		let writer = Outgoing::start(sink, Duration::from_secs(45));
		for guild in 1..=12 {
			writer
				.send(crate::subscription_packet(Id(guild), true, None, false))
				.await
				.unwrap();
		}
		let stop = frame(serde_json::json!({"op":19,"d":{"stream_key":"synthetic-only"}}));
		writer.send(stop).await.unwrap();
		writer
			.send(frame(serde_json::json!({"op":1,"d":1})))
			.await
			.unwrap();
		{
			let state = writer.state.lock().unwrap();
			assert!(state.queue.iter().any(|p| p.meta.opcode == Some(19)));
			assert_eq!(state.queue.len(), 14);
			assert!(state.bytes <= BYTES);
		}
		assert!(
			writer
				.send(Frame::Text("x".repeat(BYTES + 1).into()))
				.await
				.is_err()
		);
		assert!(writer.progress.borrow().capacity);
		assert!(writer.occupancy().0 <= PACKETS && writer.occupancy().1 <= BYTES);
	}
	#[tokio::test]
	async fn a_join_batch_keeps_heartbeat_headroom_when_the_writer_is_blocked() {
		for occupied in [12, 13] {
			let (sink, _, _, _) = fixture();
			let writer = Outgoing::start(sink, Duration::from_secs(45));
			for _ in 0..occupied {
				writer
					.send(frame(serde_json::json!({"op":4,"d":{}})))
					.await
					.unwrap();
			}
			tokio::task::yield_now().await;
			assert_eq!(writer.occupancy().0, occupied);
			assert_eq!(writer.control_capacity(), occupied == 12);
			if writer.control_capacity() {
				// The same admission guard protects the production Sync/Join/info batch.
				for opcode in [12, 4, 17] {
					writer
						.send(frame(serde_json::json!({"op":opcode,"d":{}})))
						.await
						.unwrap();
				}
				writer
					.send(frame(serde_json::json!({"op":1,"d":null})))
					.await
					.unwrap();
				assert_eq!(writer.occupancy().0, PACKETS);
			}
			assert!(!writer.progress.borrow().failed);
		}
	}
	#[tokio::test(start_paused = true)]
	async fn delayed_numeric_search_and_author_lookup_keep_their_explicit_slots() {
		use crate::member_search::{Purpose, Query};
		let (sink, ready, _, frames) = fixture();
		ready.store(true, Ordering::Release);
		let mut writer = Outgoing::start(sink, Duration::from_secs(45));
		writer.cooldown(8, None, Duration::from_secs(60));
		let query = |purpose, guild, nonce| Query {
			purpose,
			// Numeric composer searches and author lookups both encode user_ids.
			frame: frame(serde_json::json!({"op":8,"d":{
				"guild_id":guild,"nonce":nonce,"user_ids":["123456789"]
			}})),
		};
		writer
			.send_query(query(Purpose::Composer, "2", "1"))
			.await
			.unwrap();
		writer
			.send_query(query(Purpose::Authors, "3", "2"))
			.await
			.unwrap();
		writer.clear_queries(1 << Purpose::Composer.slot());
		assert_eq!(writer.occupancy().0, 1);
		writer
			.send_query(query(Purpose::Composer, "2", "3"))
			.await
			.unwrap();
		assert_eq!(writer.occupancy().0, 2);
		tokio::task::yield_now().await;
		assert!(frames.lock().unwrap().is_empty());
		tokio::time::advance(Duration::from_secs(60)).await;
		timeout(Duration::from_secs(1), async {
			while writer.occupancy().0 > 0 {
				writer.progress.changed().await.unwrap();
			}
		})
		.await
		.unwrap();
		let nonces: Vec<_> = frames
			.lock()
			.unwrap()
			.iter()
			.map(|frame| {
				serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap()["d"]
					["nonce"]
					.as_str()
					.unwrap()
					.to_owned()
			})
			.collect();
		assert_eq!(nonces, ["2", "3"]);
		assert!(!writer.progress.borrow().failed);
	}
	#[tokio::test]
	async fn each_requested_heartbeat_has_its_own_send_ticket() {
		let (sink, ready, wake, frames) = fixture();
		let mut writer = Outgoing::start(sink, Duration::from_secs(45));
		writer
			.send(frame(serde_json::json!({"op":1,"d":1})))
			.await
			.unwrap();
		let first = writer.heartbeat_ticket();
		writer
			.send(frame(serde_json::json!({"op":1,"d":2})))
			.await
			.unwrap();
		let second = writer.heartbeat_ticket();
		assert!(
			second > first,
			"server requests and recovery probes cannot stand in for each other"
		);
		assert_eq!(writer.occupancy().0, 2);
		ready.store(true, Ordering::Release);
		if let Some(wake) = wake.lock().unwrap().take() {
			wake.wake();
		}
		timeout(Duration::from_secs(1), async {
			loop {
				writer.progress.changed().await.unwrap();
				if writer
					.progress
					.borrow()
					.heartbeat
					.is_some_and(|(ticket, _)| ticket == second)
				{
					break;
				}
			}
		})
		.await
		.unwrap();
		assert_eq!(frames.lock().unwrap().len(), 2);
	}
	#[tokio::test]
	async fn cooldown_overflow_defers_optional_work_without_holding_an_unrelated_stop() {
		let (sink, ready, _, frames) = fixture();
		ready.store(true, Ordering::Release);
		let mut writer = Outgoing::start(sink, Duration::from_secs(45));
		{
			let mut state = writer.state.lock().unwrap();
			state.blocked_until = Some(Instant::now() + Duration::from_secs(60));
		}
		writer
			.send(frame(
				serde_json::json!({"op":8,"d":{"guild_id":"2","nonce":"1","query":"synthetic"}}),
			))
			.await
			.unwrap();
		writer
			.send(frame(
				serde_json::json!({"op":19,"d":{"stream_key":"synthetic-only"}}),
			))
			.await
			.unwrap();
		timeout(Duration::from_secs(1), async {
			while frames.lock().unwrap().is_empty() {
				writer.progress.changed().await.unwrap();
			}
		})
		.await
		.unwrap();
		let sent = frames.lock().unwrap();
		assert_eq!(sent.len(), 1);
		assert_eq!(
			serde_json::from_str::<serde_json::Value>(sent[0].to_text().unwrap()).unwrap()["op"],
			19
		);
		assert_eq!(writer.occupancy().0, 1);
	}
}
