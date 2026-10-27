//! Fixed, session-only connection causes. Never retains remote text or account identities.
use std::collections::VecDeque;
use std::sync::{
	Arc,
	atomic::{AtomicU64, Ordering},
};
use tokio::time::Instant;

const ENTRIES: usize = 32;
const BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionReason {
	Connecting,
	Connected,
	Resumed,
	ServerReconnect,
	ServerClose,
	RateLimited,
	SocketRead,
	SocketWrite,
	DialTimeout,
	DialFailed,
	HelloTimeout,
	HandshakeTimeout,
	ReadyTimeout,
	HeartbeatTimeout,
	ProbeTimeout,
	InvalidSession,
	LocalCapacity,
	DeliveryCongestion,
	PermissionRefresh,
	Protocol,
	SessionEnded,
}
impl ConnectionReason {
	pub fn label(self) -> &'static str {
		match self {
			Self::Connecting => "Connecting to Discord",
			Self::Connected => "Discord session established",
			Self::Resumed => "Discord session resumed",
			Self::ServerReconnect => "Discord requested a reconnect",
			Self::ServerClose => "Discord closed the connection",
			Self::RateLimited => "Discord Gateway rate limit",
			Self::SocketRead => "Gateway connection interrupted while receiving",
			Self::SocketWrite => "Gateway connection interrupted while sending",
			Self::DialTimeout => "Gateway connection attempt timed out",
			Self::DialFailed => "Gateway connection attempt failed",
			Self::HelloTimeout => "Gateway greeting timed out",
			Self::HandshakeTimeout => "Gateway handshake timed out",
			Self::ReadyTimeout => "Gateway session did not become ready",
			Self::HeartbeatTimeout => "Gateway heartbeat was not acknowledged",
			Self::ProbeTimeout => "Gateway recovery probe was not acknowledged",
			Self::InvalidSession => "Discord requested session recovery",
			Self::PermissionRefresh => "Permission metadata requires fresh synchronization",
			Self::DeliveryCongestion => "Account synchronization paused while queued updates drain",
			Self::LocalCapacity => "Gateway queue reached its safe limit",
			Self::Protocol => "Gateway response could not be handled safely",
			Self::SessionEnded => "Discord account session ended",
		}
	}
	fn failure(self) -> bool {
		!matches!(self, Self::Connecting | Self::Connected | Self::Resumed)
	}
}
#[derive(Clone, Copy)]
struct Observation {
	elapsed_ms: u64,
	reason: ConnectionReason,
	code: Option<u16>,
	attempt: u8,
	packets: u8,
	bytes: u32,
}
/// A small diagnostic ring owned by one account connection. Its owner clears it on teardown.
pub struct ConnectionDiagnostics {
	started: Instant,
	observations: VecDeque<Observation>,
	last: Option<ConnectionReason>,
	revision: Arc<AtomicU64>,
}
impl Default for ConnectionDiagnostics {
	fn default() -> Self {
		Self {
			started: Instant::now(),
			observations: VecDeque::with_capacity(ENTRIES),
			last: None,
			revision: Arc::new(AtomicU64::new(0)),
		}
	}
}
impl ConnectionDiagnostics {
	/// Lets the host refresh its bounded UI copy without locking on unchanged frames.
	pub fn revision(&self) -> Arc<AtomicU64> {
		self.revision.clone()
	}
	pub(crate) fn latest_reason(&self) -> Option<ConnectionReason> {
		self.observations
			.back()
			.map(|observation| observation.reason)
	}
	pub(crate) fn ended_with_close(&self) -> bool {
		self.observations.back().is_some_and(|observation| {
			observation.code.is_some() && observation.reason == ConnectionReason::ServerClose
		})
	}
	pub fn last_reason(&self) -> Option<&'static str> {
		self.last.map(ConnectionReason::label)
	}
	pub fn summary(&self) -> String {
		use std::fmt::Write;
		let mut text = String::with_capacity(2048);
		for observation in &self.observations {
			let _ = writeln!(
				text,
				"+{}ms: {}; code={}; attempt={}; queue={}/{}B",
				observation.elapsed_ms,
				observation.reason.label(),
				observation
					.code
					.map_or_else(|| "none".to_owned(), |v| v.to_string()),
				observation.attempt,
				observation.packets,
				observation.bytes
			);
		}
		text.truncate(text.len().min(BYTES));
		text
	}
	pub(crate) fn record(
		&mut self,
		reason: ConnectionReason,
		code: Option<u16>,
		attempt: u32,
		queue: (usize, usize),
	) {
		if reason.failure() {
			self.last = Some(reason);
		}
		if self.observations.len() == ENTRIES {
			self.observations.pop_front();
		}
		self.observations.push_back(Observation {
			elapsed_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
			reason,
			code,
			attempt: attempt.min(u32::from(u8::MAX)) as u8,
			packets: queue.0.min(usize::from(u8::MAX)) as u8,
			bytes: queue.1.min(u32::MAX as usize) as u32,
		});
		self.revision.fetch_add(1, Ordering::Release);
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn observations_notify_the_host_even_without_a_connection_state_event() {
		let mut diagnostic = ConnectionDiagnostics::default();
		let revision = diagnostic.revision();
		assert_eq!(revision.load(Ordering::Acquire), 0);
		diagnostic.record(ConnectionReason::RateLimited, None, 0, (2, 512));
		assert_eq!(revision.load(Ordering::Acquire), 1);
		assert!(
			diagnostic
				.summary()
				.contains(ConnectionReason::RateLimited.label())
		);
		let _ = diagnostic.summary();
		assert_eq!(revision.load(Ordering::Acquire), 1);
	}
	#[test]
	fn causes_remain_bounded_and_success_does_not_hide_the_last_failure() {
		let mut diagnostic = ConnectionDiagnostics::default();
		diagnostic.record(ConnectionReason::ServerClose, Some(4008), 1, (16, 65536));
		for _ in 0..100 {
			diagnostic.record(ConnectionReason::Resumed, None, 1, (0, 0));
		}
		assert_eq!(
			diagnostic.last_reason(),
			Some(ConnectionReason::ServerClose.label())
		);
		assert_eq!(diagnostic.observations.len(), ENTRIES);
		assert!(diagnostic.summary().len() <= BYTES);
	}
}
