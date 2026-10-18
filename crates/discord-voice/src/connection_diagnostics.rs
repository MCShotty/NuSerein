//! Latest-value diagnostic snapshots. No packet payloads, URLs or credentials are retained.
use crate::diagnostics::Incoming;
use model::voice_diagnostics::{
	ConnectionKind, ConnectionSnapshot, Latency, MediaCounters, PacketTotals, SAMPLE_LIFETIME,
	VideoConfiguration, VoiceServer,
};
use std::{
	sync::Arc,
	time::{Duration, Instant},
};

const PROBES: usize = 4;
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Pings {
	pending: [Option<(u64, Instant)>; PROBES],
	next: usize,
	latest: Option<Latency>,
	latest_sent: Option<Instant>,
}
impl Pings {
	fn sent(&mut self, nonce: u64, now: Instant) {
		self.pending[self.next] = Some((nonce, now));
		self.next = (self.next + 1) % PROBES;
	}
	fn received(&mut self, nonce: u64, now: Instant) {
		let Some(slot) = self
			.pending
			.iter_mut()
			.find(|slot| slot.is_some_and(|(id, _)| id == nonce))
		else {
			return;
		};
		let (_, sent) = slot.take().expect("matching pending probe");
		let Some(elapsed) = now
			.checked_duration_since(sent)
			.filter(|age| *age < SAMPLE_LIFETIME)
		else {
			return;
		};
		// A delayed older reply must not replace the latest measured route latency.
		if self.latest_sent.is_none_or(|latest| sent >= latest) {
			self.latest_sent = Some(sent);
			self.latest = Some(Latency {
				at: now,
				milliseconds: elapsed.as_millis().min(u128::from(u32::MAX)) as u32,
			});
		}
	}
}

#[derive(Default)]
pub(crate) struct AudioMonitor {
	pub incoming: Incoming,
	pub decoded: u64,
	pub concealed: u64,
	pub errors: u64,
	previous: Option<(Instant, u32)>,
	jitter: Option<f64>,
}
impl AudioMonitor {
	pub fn packet(&mut self, sequence: u16, timestamp: Option<u32>, bytes: usize, now: Instant) {
		self.incoming.packet(sequence, bytes, now);
		if let Some(timestamp) = timestamp {
			if let Some((at, previous)) = self.previous
				&& let Some(elapsed) = now.checked_duration_since(at)
			{
				if elapsed < SAMPLE_LIFETIME {
					let deviation = (elapsed.as_secs_f64() * 1000.0
						- f64::from(timestamp.wrapping_sub(previous) as i32) / 48.0)
						.abs();
					let jitter = self.jitter.unwrap_or(0.0);
					self.jitter = Some(jitter + (deviation - jitter) / 16.0);
				} else {
					self.jitter = None;
				}
			}
			self.previous = Some((now, timestamp));
		}
	}
	pub fn snapshot(&self) -> model::voice_diagnostics::AudioStatistics {
		model::voice_diagnostics::AudioStatistics {
			started: self.incoming.started,
			packets: self.incoming.totals,
			jitter_ms: self.jitter.map(|v| v as f32),
			decoded: self.decoded,
			concealed: self.concealed,
			decoder_errors: self.errors,
		}
	}
	pub fn reset_timing(&mut self) {
		self.previous = None;
		self.jitter = None;
	}
}

pub(crate) struct Connection {
	server: Option<Arc<VoiceServer>>,
	kind: ConnectionKind,
	started: Instant,
	published: Option<Instant>,
	latency_generation: u64,
	udp: Pings,
	websocket: Pings,
	received: PacketTotals,
	previous_sent: PacketTotals,
	previous_received: PacketTotals,
	previous_pictures: u64,
	loss: Option<(u8, Instant)>,
	pub received_video_codec: Option<model::voice_settings::VideoCodec>,
	previous_participants: Vec<model::voice_diagnostics::ParticipantStatistics>,
	participants_at: Option<Instant>,
}
impl Connection {
	pub fn new(url: &str, kind: ConnectionKind, now: Instant) -> Self {
		let server = url::Url::parse(url)
			.ok()
			.and_then(|url| VoiceServer::new(url.host_str()?))
			.map(Arc::new);
		Self {
			server,
			kind,
			started: now,
			published: None,
			latency_generation: 0,
			udp: Pings::default(),
			websocket: Pings::default(),
			received: PacketTotals::default(),
			previous_sent: PacketTotals::default(),
			previous_received: PacketTotals::default(),
			previous_pictures: 0,
			loss: None,
			received_video_codec: None,
			previous_participants: Vec::new(),
			participants_at: None,
		}
	}
	/// Called only when a one-second connection snapshot is published. Tables are
	/// bounded by the existing speaker/source budgets and replace their predecessors.
	pub fn participants(
		&mut self,
		mixer: &crate::mixer::Mixer,
		receivers: &crate::video_receive::Receivers,
		decoder: Option<&crate::video_receive::DecoderQueue>,
		now: Instant,
	) -> Vec<model::voice_diagnostics::ParticipantStatistics> {
		use model::voice_diagnostics::ParticipantStatistics;
		let mut participants: Vec<_> = mixer
			.diagnostics()
			.map(|(user, audio)| ParticipantStatistics {
				user,
				audio: Some(audio),
				video: None,
			})
			.collect();
		for (user, video) in receivers.diagnostics(decoder) {
			if let Some(peer) = participants.iter_mut().find(|p| p.user == user) {
				peer.video = Some(video);
			} else {
				participants.push(ParticipantStatistics {
					user,
					audio: None,
					video: Some(video),
				});
			}
		}
		let rate =
			|total: u64, started: Instant, previous: Option<(u64, Instant)>, multiplier: u64| {
				let (before, at) = previous
					.filter(|(_, generation)| *generation == started)
					.map_or((0, started), |(value, _)| {
						(value, self.participants_at.unwrap_or(started))
					});
				let elapsed = now.saturating_duration_since(at);
				if elapsed.is_zero() {
					0
				} else {
					(u128::from(total.saturating_sub(before))
						* u128::from(multiplier)
						* 1_000_000_000 / elapsed.as_nanos())
					.min(u128::from(u64::MAX)) as u64
				}
			};
		for peer in &mut participants {
			let previous = self
				.previous_participants
				.iter()
				.find(|p| p.user == peer.user);
			if let Some(audio) = &mut peer.audio {
				audio.packets.bitrate = rate(
					audio.packets.received.bytes,
					audio.started,
					previous
						.and_then(|p| p.audio)
						.map(|a| (a.packets.received.bytes, a.started)),
					8,
				);
			}
			if let Some(video) = &mut peer.video {
				let old = previous.and_then(|p| p.video);
				video.packets.bitrate = rate(
					video.packets.received.bytes,
					video.started,
					old.map(|v| (v.packets.received.bytes, v.started)),
					8,
				);
				video.decoded_fps_milli = rate(
					video.decoded,
					video.started,
					old.map(|v| (v.decoded, v.started)),
					1000,
				);
			}
		}
		self.previous_participants = participants.clone();
		self.participants_at = Some(now);
		participants
	}

	pub fn reset_latency(&mut self) {
		self.udp = Pings::default();
		self.websocket = Pings::default();
		self.latency_generation = self.latency_generation.wrapping_add(1);
	}
	pub fn udp_sent(&mut self, sequence: u32, now: Instant) {
		self.udp.sent(u64::from(sequence), now);
	}
	pub fn websocket_sent(&mut self, nonce: u64, now: Instant) {
		self.websocket.sent(nonce, now);
	}
	pub fn websocket_ack(&mut self, nonce: Option<u64>, now: Instant) {
		if let Some(nonce) = nonce {
			self.websocket.received(nonce, now);
		}
	}
	pub fn received(&mut self, bytes: usize) {
		self.received.add(bytes);
	}
	pub fn udp_echo(&mut self, packet: &[u8], now: Instant) -> bool {
		if packet.len() != 8 || packet[..4] != [0x13, 0x37, 0xca, 0xfe] {
			return false;
		}
		let nonce = u32::from_le_bytes(packet[4..8].try_into().expect("eight-byte keepalive"));
		self.udp.received(u64::from(nonce), now);
		true
	}
	pub fn loss(&mut self, loss: Option<u8>, now: Instant) {
		if let Some(loss) = loss {
			self.loss = Some((loss, now));
		}
	}
	pub fn snapshot(
		&mut self,
		now: Instant,
		sent: PacketTotals,
		send_errors: u64,
		media: MediaCounters,
		video: Option<VideoConfiguration>,
		secure: bool,
	) -> Option<ConnectionSnapshot> {
		if self
			.published
			.is_some_and(|at| now.duration_since(at) < SNAPSHOT_INTERVAL)
		{
			return None;
		}
		let elapsed = now.duration_since(self.published.unwrap_or(self.started));
		let per_second = |value: u64, multiplier: u64| -> u64 {
			if elapsed.is_zero() {
				return 0;
			}
			(u128::from(value) * u128::from(multiplier) * 1_000_000_000 / elapsed.as_nanos())
				.min(u128::from(u64::MAX)) as u64
		};
		let result = ConnectionSnapshot {
			server: self.server.clone(),
			kind: self.kind,
			started: self.started,
			at: now,
			latency_generation: self.latency_generation,
			udp_ping: self.udp.latest,
			websocket_ping: self.websocket.latest,
			sent,
			received: self.received,
			send_errors,
			send_bps: per_second(sent.bytes.saturating_sub(self.previous_sent.bytes), 8),
			receive_bps: per_second(
				self.received
					.bytes
					.saturating_sub(self.previous_received.bytes),
				8,
			),
			secure,
			reported_loss: self.loss,
			media,
			video,
			received_video_codec: self.received_video_codec,
			participants: Vec::new(),
			decoded_fps_milli: per_second(
				media
					.decoded_pictures
					.saturating_sub(self.previous_pictures),
				1000,
			),
		};
		self.published = Some(now);
		self.previous_sent = sent;
		self.previous_received = self.received;
		self.previous_pictures = media.decoded_pictures;
		Some(result)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn participant_rtp_loss_waits_for_repairs_and_survives_wrap_and_large_gaps() {
		let now = Instant::now();
		let mut received = Incoming::default();
		let mut unique = std::collections::HashSet::new();
		// A bounded out-of-order arrival pattern, duplicates and deterministic loss.
		for base in (0u16..1000).step_by(4) {
			for offset in [0, 2, 1, 3] {
				let sequence = base + offset;
				if sequence % 7 == 3 {
					continue;
				}
				received.packet(sequence.wrapping_add(u16::MAX - 20), 100, now);
				unique.insert(sequence);
				if sequence.is_multiple_of(12) {
					received.packet(sequence.wrapping_add(u16::MAX - 20), 100, now);
				}
			}
		}
		let finalized = u64::from(*unique.iter().max().unwrap()) + 1 - 64;
		assert_eq!(received.totals.finalized, finalized);
		assert_eq!(
			received.totals.missing,
			finalized
				- unique
					.iter()
					.filter(|sequence| u64::from(**sequence) < finalized)
					.count() as u64
		);
		assert!(received.totals.duplicates > 0);
		let before = received.totals.missing;
		received.packet(u16::MAX - 17, 100, now); // Too late to change a finalized gap.
		assert_eq!(received.totals.missing, before);
		let mut jump = Incoming::default();
		jump.packet(0, 1, now);
		jump.packet(1000, 1, now);
		assert_eq!((jump.totals.finalized, jump.totals.missing), (937, 936));
		let mut repaired = Incoming::default();
		for sequence in [0, 2, 1, 64, 65] {
			repaired.packet(sequence, 1, now);
		}
		assert_eq!((repaired.totals.finalized, repaired.totals.missing), (2, 0));
	}
	#[test]
	fn participant_audio_jitter_uses_rtp_time_and_resets_after_a_gap() {
		let now = Instant::now();
		let mut audio = AudioMonitor::default();
		audio.packet(1, Some(u32::MAX - 959), 100, now);
		assert!(audio.snapshot().jitter_ms.is_none());
		audio.packet(2, Some(0), 100, now + Duration::from_millis(20));
		assert_eq!(audio.snapshot().jitter_ms, Some(0.0));
		audio.packet(3, Some(960), 100, now + Duration::from_millis(80));
		assert_eq!(audio.snapshot().jitter_ms, Some(2.5));
		audio.packet(4, Some(1920), 100, now + Duration::from_secs(30));
		assert!(audio.snapshot().jitter_ms.is_none());
		audio.reset_timing();
		assert_eq!(audio.snapshot().packets.received.packets, 4);
	}
	#[test]
	fn participant_snapshots_attribute_rates_and_retire_departed_or_replaced_sources() {
		let mut mixer = crate::mixer::Mixer::default();
		let mut receivers = crate::video_receive::Receivers::default();
		mixer.announce(1, 11).unwrap();
		receivers.announce(1, 12).unwrap();
		receivers.announce(2, 22).unwrap();
		let now = Instant::now();
		let mut c = Connection::new("wss://us-east1.discord.media", ConnectionKind::Call, now);
		assert_eq!(c.participants(&mixer, &receivers, None, now).len(), 2);
		mixer.received(11, 1, 0, 100, now + Duration::from_millis(20));
		mixer.received(99, 1, 0, 900, now); // Unannounced source cannot gain a row.
		receivers.push(12, 1, 0, true, &[0x65, 1]);
		receivers.push(22, 1, 0, true, &[0x65, 1, 2]);
		let peers = c.participants(&mixer, &receivers, None, now + Duration::from_secs(1));
		let first = peers.iter().find(|p| p.user == 1).unwrap();
		assert_eq!(first.audio.unwrap().packets.bitrate, 800);
		assert_eq!(first.video.unwrap().packets.bitrate, 16);
		let second = peers.iter().find(|p| p.user == 2).unwrap();
		assert!(second.audio.is_none());
		assert_eq!(second.video.unwrap().packets.bitrate, 24);
		mixer.announce(1, 33).unwrap();
		receivers.announce(2, 12).unwrap(); // Ownership reassignment resets this SSRC.
		let peers = c.participants(&mixer, &receivers, None, now + Duration::from_secs(2));
		let first = peers.iter().find(|p| p.user == 1).unwrap();
		assert!(first.video.is_none());
		assert_eq!(first.audio.unwrap().packets.received.packets, 0);
		assert_eq!(first.audio.unwrap().packets.bitrate, 0);
		mixer.remove(1);
		receivers.remove(2);
		assert!(
			c.participants(&mixer, &receivers, None, now + Duration::from_secs(3))
				.is_empty()
		);
		assert!(c.previous_participants.is_empty());
	}
	#[test]
	fn participant_video_rates_do_not_jump_when_an_overlapping_layer_changes() {
		use std::sync::atomic::Ordering;
		let (decoder, _) = crate::video_receive::spawn_decoder(Arc::new(|_| {})).unwrap();
		let mixer = crate::mixer::Mixer::default();
		let mut receivers = crate::video_receive::Receivers::default();
		receivers.announce(1, 11).unwrap();
		crate::video_receive::retain_sources(&decoder, &receivers);
		let lifetime = decoder.source_lifetime(1).unwrap();
		lifetime.pictures.store(100, Ordering::Relaxed);
		let now = Instant::now();
		let mut c = Connection::new("wss://japan1.discord.media", ConnectionKind::Call, now);
		c.participants(&mixer, &receivers, Some(&decoder), now);
		receivers.announce(1, 12).unwrap();
		crate::video_receive::retain_sources(&decoder, &receivers);
		lifetime.pictures.store(130, Ordering::Relaxed);
		let peer = c.participants(
			&mixer,
			&receivers,
			Some(&decoder),
			now + Duration::from_secs(1),
		);
		assert_eq!(peer[0].video.unwrap().decoded_fps_milli, 30000);
		receivers.retain_user_sources(1, &[12]);
		crate::video_receive::retain_sources(&decoder, &receivers);
		lifetime.pictures.store(160, Ordering::Relaxed);
		let peer = c.participants(
			&mixer,
			&receivers,
			Some(&decoder),
			now + Duration::from_secs(2),
		);
		assert_eq!(peer[0].video.unwrap().decoded_fps_milli, 30000);
	}
	fn echo(sequence: u32) -> [u8; 8] {
		let mut packet = [0x13, 0x37, 0xca, 0xfe, 0, 0, 0, 0];
		packet[4..].copy_from_slice(&sequence.to_le_bytes());
		packet
	}
	fn snapshot(c: &mut Connection, now: Instant) -> ConnectionSnapshot {
		c.snapshot(
			now,
			PacketTotals::default(),
			0,
			MediaCounters::default(),
			None,
			false,
		)
		.unwrap()
	}
	#[test]
	fn ping_requires_an_exact_pending_echo_and_is_measured_once() {
		let now = Instant::now();
		let mut c = Connection::new(
			"wss://us-east1.discord.media/?v=8",
			ConnectionKind::Call,
			now,
		);
		c.udp_sent(7, now);
		assert!(c.udp_echo(&echo(8), now + Duration::from_millis(10)));
		assert!(!c.udp_echo(&[0; 8], now));
		assert!(c.udp.latest.is_none());
		assert!(c.udp_echo(&echo(7), now + Duration::from_millis(42)));
		assert_eq!(c.udp.latest.unwrap().milliseconds, 42);
		c.udp_echo(&echo(7), now + Duration::from_millis(99));
		assert_eq!(c.udp.latest.unwrap().milliseconds, 42);
		c.websocket_sent(9, now);
		c.websocket_ack(Some(9), now + Duration::from_millis(75));
		assert_eq!(c.websocket.latest.unwrap().milliseconds, 75);
		assert_eq!(
			snapshot(&mut c, now + Duration::from_secs(1))
				.server
				.unwrap()
				.host(),
			"us-east1.discord.media"
		);
	}
	#[test]
	fn missing_delayed_wrapped_and_replaced_probes_stay_bounded() {
		let now = Instant::now();
		let mut c = Connection::new("wss://test.discord.media", ConnectionKind::Call, now);
		for n in 0..100 {
			c.udp_sent(n, now);
		}
		assert_eq!(c.udp.pending.iter().flatten().count(), PROBES);
		c.udp_echo(&echo(0), now + Duration::from_millis(10));
		assert!(c.udp.latest.is_none());
		c.udp_sent(u32::MAX, now);
		c.udp_echo(&echo(u32::MAX), now + Duration::from_millis(40));
		assert_eq!(c.udp.latest.unwrap().milliseconds, 40);
		c.reset_latency();
		c.udp_echo(&echo(u32::MAX), now + Duration::from_millis(50));
		assert!(c.udp.latest.is_none());
		c.udp_sent(0, now);
		c.udp_echo(&echo(0), now + SAMPLE_LIFETIME);
		assert!(c.udp.latest.is_none());
		c.udp_sent(1, now);
		c.udp_echo(&echo(1), now - Duration::from_millis(1));
		assert!(c.udp.latest.is_none());
	}
	#[test]
	fn snapshots_are_throttled_and_wire_rates_use_the_actual_window() {
		let now = Instant::now();
		let mut c = Connection::new("wss://x.discord.media", ConnectionKind::ScreenSend, now);
		c.received(2000);
		let sent = PacketTotals {
			packets: 10,
			bytes: 1000,
		};
		let media = MediaCounters {
			decoded_pictures: 30,
			..Default::default()
		};
		let s = c
			.snapshot(now + Duration::from_secs(2), sent, 2, media, None, true)
			.unwrap();
		assert_eq!(
			(s.send_bps, s.receive_bps, s.decoded_fps_milli),
			(4000, 8000, 15000)
		);
		assert_eq!(s.send_errors, 2);
		assert!(
			c.snapshot(
				now + Duration::from_millis(2500),
				sent,
				2,
				media,
				None,
				true
			)
			.is_none()
		);
		let s = c
			.snapshot(now + Duration::from_secs(3), sent, 2, media, None, true)
			.unwrap();
		assert_eq!((s.send_bps, s.receive_bps, s.decoded_fps_milli), (0, 0, 0));
	}

	#[test]
	fn reordered_replies_do_not_replace_a_newer_probe_and_resume_preserves_rate_window() {
		let now = Instant::now();
		let mut c = Connection::new("wss://x.discord.media", ConnectionKind::Call, now);
		c.udp_sent(1, now);
		c.udp_sent(2, now + Duration::from_millis(5));
		c.udp_sent(3, now + Duration::from_millis(10));
		c.udp_echo(&echo(2), now + Duration::from_millis(20));
		c.udp_echo(&echo(1), now + Duration::from_millis(25));
		assert_eq!(c.udp.latest.unwrap().milliseconds, 15);
		c.udp_echo(&echo(3), now + Duration::from_millis(30));
		assert_eq!(c.udp.latest.unwrap().milliseconds, 20);
		snapshot(&mut c, now + Duration::from_secs(10));
		c.reset_latency();
		let s = c
			.snapshot(
				now + Duration::from_secs(11),
				PacketTotals {
					packets: 1,
					bytes: 1000,
				},
				0,
				MediaCounters::default(),
				None,
				false,
			)
			.unwrap();
		assert!(s.udp_ping.is_none() && s.websocket_ping.is_none());
		assert_eq!((s.latency_generation, s.send_bps), (1, 8000));
	}
}
