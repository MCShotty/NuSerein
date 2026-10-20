//! Bounded, session-only voice connection information. No credentials or media payloads.
use crate::voice_settings::VideoCodec;
use std::{
	sync::Arc,
	time::{Duration, Instant},
};

pub const SAMPLE_LIFETIME: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionKind {
	#[default]
	Call,
	ScreenSend,
	ScreenReceive,
}

/// Only a validated media hostname may enter a diagnostic snapshot or clipboard report.
#[derive(Debug)]
pub struct VoiceServer {
	host: Box<str>,
}
impl VoiceServer {
	pub fn new(host: &str) -> Option<Self> {
		if host.len() > 253 || !host.is_ascii() {
			return None;
		}
		let host = host.to_ascii_lowercase();
		let prefix = host.strip_suffix(".discord.media")?;
		if prefix.is_empty()
			|| !host.split('.').all(|label| {
				!label.is_empty()
					&& label.len() <= 63
					&& !label.starts_with('-')
					&& !label.ends_with('-')
					&& label
						.bytes()
						.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
			}) {
			return None;
		}
		Some(Self {
			host: host.into_boxed_str(),
		})
	}
	pub fn host(&self) -> &str {
		&self.host
	}
	/// An endpoint convention, not an independently verified physical server location.
	pub fn inferred_region(&self) -> Option<&'static str> {
		let prefix = self.host.strip_suffix(".discord.media")?;
		let prefix = prefix.strip_prefix("c-").unwrap_or(prefix);
		let prefix = prefix.strip_prefix("vip-").unwrap_or(prefix);
		[
			("us-central", "US Central"),
			("us-east", "US East"),
			("us-south", "US South"),
			("us-west", "US West"),
			("southafrica", "South Africa"),
			("south-korea", "South Korea"),
			("rotterdam", "Rotterdam"),
			("hongkong", "Hong Kong"),
			("singapore", "Singapore"),
			("sydney", "Sydney"),
			("brazil", "Brazil"),
			("japan", "Japan"),
			("india", "India"),
			("russia", "Russia"),
			("europe", "Europe"),
			("dubai", "Dubai"),
			("buenos-aires", "Buenos Aires"),
			("bucharest", "Bucharest"),
			("finland", "Finland"),
			("milan", "Milan"),
			("madrid", "Madrid"),
		]
		.into_iter()
		.find_map(|(name, label)| {
			let rest = prefix.strip_prefix(name)?;
			(rest.is_empty() || rest.starts_with('-') || rest.as_bytes()[0].is_ascii_digit())
				.then_some(label)
		})
	}
}

#[derive(Clone, Copy, Debug)]
pub struct Latency {
	pub at: Instant,
	pub milliseconds: u32,
}
impl Latency {
	pub fn fresh(self, now: Instant) -> Option<u32> {
		now.checked_duration_since(self.at)
			.filter(|age| *age < SAMPLE_LIFETIME)
			.map(|_| self.milliseconds)
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacketTotals {
	pub packets: u64,
	pub bytes: u64,
}
impl PacketTotals {
	pub fn add(&mut self, bytes: usize) {
		self.packets = self.packets.saturating_add(1);
		self.bytes = self.bytes.saturating_add(bytes as u64);
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaCounters {
	pub video_packets: u64,
	pub video_not_ready: u64,
	pub unknown_video_sources: u64,
	pub complete_video_frames: u64,
	pub gated_video_frames: u64,
	pub received_keyframes: u64,
	pub keyframes_without_parameters: u64,
	pub awaiting_keyframe_ticks: u64,
	pub video_stall_ticks: u64,
	pub max_picture_gap_ms: u64,
	pub max_decode_queue_ms: u64,
	pub rtx_packets: u64,
	pub incomplete_frames: u64,
	pub pli_sent: u64,
	pub decoded_pictures: u64,
	pub decoder_errors: u64,
	pub queue_drops: u64,
	pub stale_frames: u64,
	pub rejected_frames: u64,
	pub authentication_failures: u64,
	pub decryption_failures: u64,
	pub local_media_drops: u64,
	pub media_stalls: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct VideoConfiguration {
	pub codec: VideoCodec,
	pub width: u32,
	pub height: u32,
	pub fps: u32,
	pub target_bitrate: u32,
}

/// Authenticated incoming RTP payloads. Loss is finalized only after a bounded
/// 64-packet reorder window; it is a receive-side estimate, not an RTCP report.
#[derive(Clone, Copy, Debug, Default)]
pub struct IncomingPackets {
	pub last_received: Option<Instant>,
	pub received: PacketTotals,
	pub finalized: u64,
	pub missing: u64,
	pub duplicates: u64,
	pub bitrate: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioStatistics {
	pub started: Instant,
	pub packets: IncomingPackets,
	/// RFC 3550 interarrival jitter in milliseconds, using the 48 kHz RTP clock.
	pub jitter_ms: Option<f32>,
	pub decoded: u64,
	/// Actual Opus concealment requests, including the bounded tail after speech stops.
	pub concealed: u64,
	pub decoder_errors: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct VideoStatistics {
	pub started: Instant,
	pub packets: IncomingPackets,
	pub codec: Option<VideoCodec>,
	pub complete: u64,
	pub incomplete: u64,
	pub queue_drops: u64,
	pub awaiting_keyframe: bool,
	pub decoded: u64,
	pub decoder_errors: u64,
	pub stale: u64,
	pub dimensions: Option<(u32, u32)>,
	pub decoded_fps_milli: u64,
}
#[derive(Clone, Debug)]
pub struct ParticipantStatistics {
	/// Used only to resolve the current local roster; excluded from copied reports.
	pub user: u64,
	pub audio: Option<AudioStatistics>,
	pub video: Option<VideoStatistics>,
}

#[derive(Clone, Debug)]
pub struct ConnectionSnapshot {
	pub server: Option<Arc<VoiceServer>>,
	pub kind: ConnectionKind,
	pub started: Instant,
	pub at: Instant,
	/// Changes when a resumed transport retires its old latency samples.
	pub latency_generation: u64,
	pub udp_ping: Option<Latency>,
	pub websocket_ping: Option<Latency>,
	pub sent: PacketTotals,
	pub received: PacketTotals,
	pub send_errors: u64,
	pub send_bps: u64,
	pub receive_bps: u64,
	pub secure: bool,
	/// Receiver-reported outgoing video loss, /256; absent when not measured.
	pub reported_loss: Option<(u8, Instant)>,
	pub media: MediaCounters,
	pub video: Option<VideoConfiguration>,
	pub received_video_codec: Option<VideoCodec>,
	/// Measured decoder deliveries, not the configured encoder frame rate.
	pub decoded_fps_milli: u64,
	pub participants: Vec<ParticipantStatistics>,
}
impl ConnectionSnapshot {
	pub fn fresh(&self, now: Instant) -> bool {
		now.checked_duration_since(self.at)
			.is_some_and(|age| age < SAMPLE_LIFETIME)
	}
	pub fn loss_percent(&self, now: Instant) -> Option<f32> {
		let (loss, at) = self.reported_loss?;
		now.checked_duration_since(at)
			.filter(|age| *age < SAMPLE_LIFETIME)
			.map(|_| f32::from(loss) * 100.0 / 256.0)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn server_information_is_bounded_and_cannot_contain_secrets_or_urls() {
		for bad in [
			"",
			"discord.media",
			"voice.example.com",
			"user@us-east.discord.media",
			"us-east.discord.media/path",
			"us-east.discord.media?token=secret",
			"us-east.discord.media:443",
			"us-east..discord.media",
			"-us-east.discord.media",
			"us-east\n.discord.media",
			"é.discord.media",
		] {
			assert!(VoiceServer::new(bad).is_none(), "{bad:?}");
		}
		assert!(VoiceServer::new(&format!("{}.discord.media", "x".repeat(254))).is_none());
		for (host, expected) in [
			("US-EAST123.discord.media", Some("US East")),
			("c-rotterdam-abcd.discord.media", Some("Rotterdam")),
			("vip-japan7.discord.media", Some("Japan")),
			("japanese.discord.media", None),
			("unknown-1.discord.media", None),
		] {
			assert_eq!(VoiceServer::new(host).unwrap().inferred_region(), expected);
		}
	}
	#[test]
	fn expired_and_future_latency_is_not_displayed_as_current() {
		let at = Instant::now();
		let sample = Latency {
			at,
			milliseconds: 42,
		};
		assert_eq!(sample.fresh(at), Some(42));
		assert_eq!(sample.fresh(at + SAMPLE_LIFETIME), None);
		assert_eq!(sample.fresh(at - Duration::from_millis(1)), None);
	}
}
