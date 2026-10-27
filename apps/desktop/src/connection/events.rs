//! Reliable event byte admission and deliberately lossy typing delivery.
use super::{
	Arc, Duration, EVENT_SLOTS, Envelope, Event, Failure, Instant, MAX_EVENT_BYTES,
	OwnedSemaphorePermit, Semaphore, egui, mpsc,
};

// Ordinary bursts retain their original budget. Startup uses one reserved slot in this
// same FIFO so subsequent dispatches cannot overtake the account snapshot.
pub(super) const RELIABLE_ITEMS: usize = 4000 + EVENT_SLOTS;

pub(super) const RELIABLE_BYTES: usize = EVENT_SLOTS * MAX_EVENT_BYTES;

#[derive(Clone)]
pub(super) struct ReliableSender {
	pub(super) send: mpsc::Sender<(Envelope, OwnedSemaphorePermit)>,
	pub(super) bytes: Arc<Semaphore>,
	pub(super) startup: Arc<Semaphore>,
	pressure: Arc<Pressure>,
}

struct Pressure {
	blocked: tokio::sync::watch::Sender<bool>,
	revision: std::sync::atomic::AtomicU64,
}
impl Default for Pressure {
	fn default() -> Self {
		Self {
			blocked: tokio::sync::watch::channel(false).0,
			revision: std::sync::atomic::AtomicU64::new(0),
		}
	}
}
impl Pressure {
	fn raise(&self) {
		self.blocked.send_if_modified(|blocked| {
			if *blocked {
				return false;
			}
			*blocked = true;
			self.revision.fetch_add(1, super::Ordering::Release);
			true
		});
	}
}
pub struct ReliableEvents {
	pub(super) receive: mpsc::Receiver<(Envelope, OwnedSemaphorePermit)>,
	pub(super) wake: egui::Context,
	bytes: Arc<Semaphore>,
	pressure: Arc<Pressure>,
	reported_pressure: u64,
}

impl ReliableSender {
	pub(super) fn delivery(&self) -> tokio::sync::watch::Receiver<bool> {
		self.pressure.blocked.subscribe()
	}

	pub(super) fn delivery_blocked(&self) -> bool {
		*self.pressure.blocked.borrow()
	}
}

impl ReliableEvents {
	fn resume_delivery(&self) {
		if self.receive.len() <= RELIABLE_ITEMS / 2
			&& self.bytes.available_permits() >= RELIABLE_BYTES / 2
		{
			self.pressure.blocked.send_if_modified(|blocked| {
				let changed = *blocked;
				*blocked = false;
				changed
			});
		}
	}
	pub fn take_pressure(&mut self) -> bool {
		let revision = self.pressure.revision.load(super::Ordering::Acquire);
		let changed = revision != self.reported_pressure;
		self.reported_pressure = revision;
		self.resume_delivery();
		changed
	}
	pub fn try_recv(&mut self) -> Result<Envelope, mpsc::error::TryRecvError> {
		let (envelope, permit) = self.receive.try_recv()?;
		drop(permit);
		self.resume_delivery();
		// The UI consumes a fixed batch each frame. Schedule another only for remaining work.
		if !self.receive.is_empty() {
			self.wake.request_repaint();
		}
		Ok(envelope)
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
		},
	)
}

pub(super) fn emit_event(
	reliable: &ReliableSender,
	typing: &mpsc::Sender<Envelope>,
	envelope: Envelope,
	ctx: &egui::Context,
) -> Result<(), Failure> {
	if matches!(envelope.event, Event::Typing(_)) {
		// Ephemeral signals have separate fixed slots and may be dropped under pressure.
		if typing.try_send(envelope).is_ok() {
			ctx.request_repaint();
		}
		return Ok(());
	}
	let bytes = envelope.event.bytes();
	let startup = matches!(&envelope.event, Event::Startup(_) | Event::Ready { .. });
	let overhead = size_of::<(Envelope, OwnedSemaphorePermit)>() - size_of::<Event>();
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
	if *reliable.pressure.blocked.borrow() {
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
		.try_send((envelope, permit))
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
		signal: client_core::typing::Signal,
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
