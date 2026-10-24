//! Reliable event byte admission and deliberately lossy typing delivery.
use super::{
	Arc, Duration, EVENT_SLOTS, Envelope, Event, Failure, Instant, MAX_EVENT_BYTES,
	OwnedSemaphorePermit, Semaphore, egui, mpsc,
};

// Ordinary bursts retain their original budget. Startup uses one reserved slot in this
// same FIFO so subsequent dispatches cannot overtake the account snapshot.
pub(super) const RELIABLE_ITEMS: usize = 4000 + EVENT_SLOTS;

pub(super) const RELIABLE_BYTES: usize = EVENT_SLOTS * MAX_EVENT_BYTES;

pub(super) struct ReliableSender {
	pub(super) send: mpsc::Sender<(Envelope, OwnedSemaphorePermit)>,
	pub(super) bytes: Arc<Semaphore>,
	pub(super) startup: Arc<Semaphore>,
}

pub struct ReliableEvents {
	pub(super) receive: mpsc::Receiver<(Envelope, OwnedSemaphorePermit)>,
	pub(super) wake: egui::Context,
}

impl ReliableEvents {
	pub fn try_recv(&mut self) -> Result<Envelope, mpsc::error::TryRecvError> {
		let (envelope, _permit) = self.receive.try_recv()?;
		// The UI consumes a fixed batch each frame. Schedule another only for remaining work.
		if !self.receive.is_empty() {
			self.wake.request_repaint();
		}
		Ok(envelope)
	}
}

pub(super) fn reliable_events(wake: egui::Context) -> (ReliableSender, ReliableEvents) {
	let (send, receive) = mpsc::channel(RELIABLE_ITEMS);
	(
		ReliableSender {
			send,
			bytes: Arc::new(Semaphore::new(RELIABLE_BYTES)),
			startup: Arc::new(Semaphore::new(1)),
		},
		ReliableEvents { receive, wake },
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
	// Event::bytes includes Event itself; also charge envelope padding and the owned permit.
	let bytes = bytes + overhead;
	let permit = if startup {
		reliable.startup.clone().try_acquire_owned().map_err(|_| {
			Failure::CapacityAt("Account startup snapshot is already queued; connection stopped")
		})?
	} else {
		reliable
			.bytes
			.clone()
			.try_acquire_many_owned(bytes as u32)
			.map_err(|_| {
				Failure::CapacityAt(
					"Account synchronization queue exceeds 32 MiB; connection stopped",
				)
			})?
	};
	reliable
		.send
		.try_send((envelope, permit))
		.map_err(|error| match error {
			mpsc::error::TrySendError::Full(_) => Failure::CapacityAt(
				"Account synchronization event queue is full; connection stopped",
			),
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
