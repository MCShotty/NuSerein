//! Reliable event byte admission and deliberately lossy typing delivery.
use super::{
	Arc, Duration, EVENT_SLOTS, Envelope, Event, Failure, Instant, MAX_EVENT_BYTES,
	OwnedSemaphorePermit, Semaphore, egui, mpsc,
};

// Ordinary bursts retain their original budget. Startup uses one reserved slot in this
// same FIFO so subsequent dispatches cannot overtake the account snapshot.
pub(super) const RELIABLE_ITEMS: usize = 4000 + EVENT_SLOTS;

pub(super) const RELIABLE_BYTES: usize = EVENT_SLOTS * MAX_EVENT_BYTES;

pub(super) type QueuedEvent = (Envelope, OwnedSemaphorePermit, u64);
#[derive(Clone)]
pub(super) struct ReliableSender {
	pub(super) send: mpsc::Sender<QueuedEvent>,
	pub(super) bytes: Arc<Semaphore>,
	pub(super) startup: Arc<Semaphore>,
	pressure: Arc<Pressure>,
}

struct Pressure {
	blocked: tokio::sync::watch::Sender<discord_gateway::DeliveryState>,
	revision: std::sync::atomic::AtomicU64,
}
impl Default for Pressure {
	fn default() -> Self {
		Self {
			blocked: tokio::sync::watch::channel(discord_gateway::DeliveryState::default()).0,
			revision: std::sync::atomic::AtomicU64::new(0),
		}
	}
}
impl Pressure {
	fn raise(&self) {
		self.blocked.send_if_modified(|blocked| {
			if blocked.blocked {
				return false;
			}
			blocked.blocked = true;
			blocked.recovering = true;
			blocked.generation = self
				.revision
				.fetch_add(1, super::Ordering::Release)
				.wrapping_add(1);
			true
		});
	}
}
pub struct ReliableEvents {
	pub(super) receive: mpsc::Receiver<QueuedEvent>,
	pub(super) wake: egui::Context,
	bytes: Arc<Semaphore>,
	pressure: Arc<Pressure>,
	reported_pressure: u64,
	delivery_generation: u64,
	awaiting_snapshot: Option<(u64, bool)>,
}

impl ReliableSender {
	pub(super) fn delivery(&self) -> tokio::sync::watch::Receiver<discord_gateway::DeliveryState> {
		self.pressure.blocked.subscribe()
	}

	pub(super) fn delivery_blocked(&self) -> bool {
		self.pressure.blocked.borrow().blocked
	}
}

impl ReliableEvents {
	fn observe_pressure(&mut self) {
		let revision = self.pressure.revision.load(super::Ordering::Acquire);
		if revision != self.delivery_generation {
			self.delivery_generation = revision;
			self.awaiting_snapshot = Some((revision, false));
		}
	}
	fn resume_delivery(&self) {
		if self.receive.len() <= RELIABLE_ITEMS / 2
			&& self.bytes.available_permits() >= RELIABLE_BYTES / 2
		{
			self.pressure.blocked.send_if_modified(|blocked| {
				let changed = blocked.blocked;
				blocked.blocked = false;
				changed
			});
		}
	}
	pub fn take_pressure(&mut self) -> bool {
		self.observe_pressure();
		let revision = self.pressure.revision.load(super::Ordering::Acquire);
		let changed = revision != self.reported_pressure;
		self.reported_pressure = revision;
		self.resume_delivery();
		// Recovery can complete while a frame is draining events. Do not reset the
		// newly installed snapshot on the next frame's delayed pressure notification.
		changed && self.awaiting_snapshot.is_some()
	}
	pub fn is_current(&self, epoch: u64) -> bool {
		epoch == self.pressure.revision.load(super::Ordering::Acquire)
	}
	#[cfg(test)]
	pub fn try_recv(&mut self) -> Result<Envelope, mpsc::error::TryRecvError> {
		self.try_recv_epoch().map(|(event, _)| event)
	}
	pub fn try_recv_epoch(&mut self) -> Result<(Envelope, u64), mpsc::error::TryRecvError> {
		self.observe_pressure();
		// Retire old work without spending an unbounded UI frame draining the FIFO.
		for _ in 0..64 {
			let (envelope, permit, epoch) = self.receive.try_recv()?;
			drop(permit);
			self.resume_delivery();
			if !self.receive.is_empty() {
				self.wake.request_repaint();
			}
			self.observe_pressure();
			if epoch != self.delivery_generation {
				continue;
			}
			if let Some((needed, barrier)) = &mut self.awaiting_snapshot {
				if epoch != *needed {
					continue;
				}
				if matches!(&envelope.event, Event::Resync) {
					*barrier = true;
				} else if *barrier
					&& matches!(&envelope.event, Event::Startup(_) | Event::Ready { .. })
				{
					self.awaiting_snapshot = None;
					self.pressure.blocked.send_if_modified(|state| {
						if state.generation != epoch || !state.recovering {
							return false;
						}
						state.recovering = false;
						true
					});
				} else {
					continue;
				}
			}
			return Ok((envelope, epoch));
		}
		Err(mpsc::error::TryRecvError::Empty)
	}
}

pub(super) fn reliable_events(wake: egui::Context) -> (ReliableSender, ReliableEvents) {
	let (send, receive) = mpsc::channel(RELIABLE_ITEMS);
	let bytes = Arc::new(Semaphore::new(RELIABLE_BYTES));
	let pressure = Arc::new(Pressure::default());
	(
		ReliableSender {
			send,
			bytes: bytes.clone(),
			pressure: pressure.clone(),
			startup: Arc::new(Semaphore::new(1)),
		},
		ReliableEvents {
			receive,
			wake,
			bytes,
			pressure,
			reported_pressure: 0,
			delivery_generation: 0,
			awaiting_snapshot: None,
		},
	)
}

#[cfg(test)]
pub(super) fn emit_event(
	reliable: &ReliableSender,
	typing: &mpsc::Sender<Envelope>,
	envelope: Envelope,
	ctx: &egui::Context,
) -> Result<(), Failure> {
	emit_event_at(reliable, typing, envelope, ctx, None)
}

pub(super) fn emit_event_at(
	reliable: &ReliableSender,
	typing: &mpsc::Sender<Envelope>,
	envelope: Envelope,
	ctx: &egui::Context,
	epoch: Option<u64>,
) -> Result<(), Failure> {
	let epoch = epoch.unwrap_or_else(|| reliable.pressure.revision.load(super::Ordering::Acquire));
	if matches!(envelope.event, Event::Typing(_)) {
		// Optional member labels use the same eight lossy slots, with a per-slot byte cap.
		if envelope.event.bytes() > 32 * 1024 {
			return Ok(());
		}
		if reliable.pressure.blocked.borrow().blocked
			|| epoch != reliable.pressure.revision.load(super::Ordering::Acquire)
		{
			return Err(Failure::Busy);
		}
		// Ephemeral signals have separate fixed slots and may be dropped under pressure.
		if typing.try_send(envelope).is_ok() {
			ctx.request_repaint();
		}
		return Ok(());
	}
	let bytes = envelope.event.bytes();
	let startup = matches!(&envelope.event, Event::Startup(_) | Event::Ready { .. });
	let overhead = size_of::<QueuedEvent>() - size_of::<Event>();
	if startup && bytes.saturating_add(overhead) > model::account::MAX_BYTES {
		return Err(Failure::CapacityAt(
			"Account startup snapshot exceeds 128 MiB; connection stopped",
		));
	}
	if !startup && bytes > MAX_EVENT_BYTES {
		return Err(Failure::CapacityAt(
			"Account synchronization event exceeds 4 MiB; connection stopped",
		));
	}
	if reliable.pressure.blocked.borrow().blocked
		|| epoch != reliable.pressure.revision.load(super::Ordering::Acquire)
	{
		return Err(Failure::Busy);
	}
	let congested = || {
		reliable.pressure.raise();
		ctx.request_repaint();
		Failure::Busy
	};
	// Event::bytes includes Event itself; also charge envelope padding and the owned permit.
	let bytes = bytes + overhead;
	let permit = if startup {
		reliable
			.startup
			.clone()
			.try_acquire_owned()
			.map_err(|_| congested())?
	} else {
		reliable
			.bytes
			.clone()
			.try_acquire_many_owned(bytes as u32)
			.map_err(|_| congested())?
	};
	reliable
		.send
		.try_send((envelope, permit, epoch))
		.map_err(|error| match error {
			mpsc::error::TrySendError::Full(_) => congested(),
			mpsc::error::TrySendError::Closed(_) => Failure::Network,
		})?;
	ctx.request_repaint();
	Ok(())
}

/// At most eight typing wakeups per two seconds in the selected conversation.
#[derive(Default)]
pub(super) struct TypingGate {
	pub(super) channel: u64,
	pub(super) users: [Option<(model::Id, Instant)>; 8],
}

impl TypingGate {
	pub(super) fn accept(
		&mut self,
		signal: &client_core::typing::Signal,
		active: u64,
		now: Instant,
	) -> bool {
		if active == 0 || signal.channel.0 != active || signal.user.0 == 0 {
			return false;
		}
		if self.channel != active {
			self.channel = active;
			self.users.fill(None);
		}
		for slot in &mut self.users {
			if slot.is_some_and(|(_, time)| {
				now.saturating_duration_since(time) >= Duration::from_secs(2)
			}) {
				*slot = None;
			}
		}
		if self
			.users
			.iter()
			.flatten()
			.any(|(user, _)| *user == signal.user)
		{
			return false;
		}
		let Some(slot) = self.users.iter_mut().find(|slot| slot.is_none()) else {
			return false;
		};
		*slot = Some((signal.user, now));
		true
	}
}
