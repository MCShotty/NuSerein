//! Bounded RFC 3550 clock correlation for watched-stream software presentation.
//! Transport authentication and current SSRC ownership are checked by the caller.
use std::{
	sync::Mutex,
	time::{Duration, Instant},
};

const MAX_USERS: usize = 16;
const REPORT_AGE: Duration = Duration::from_secs(15);
const AUDIO_AGE: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_millis(120);
const NTP_SCALE: i128 = 1i128 << 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
	Audio,
	Video,
}
impl Kind {
	fn rate(self) -> i128 {
		match self {
			Self::Audio => 48_000,
			Self::Video => 90_000,
		}
	}
}
#[derive(Clone, Copy)]
struct Report {
	ssrc: u32,
	rtp: u32,
	ntp: u64,
	received: Instant,
}
impl Report {
	fn time(self, rtp: u32, kind: Kind) -> Option<u64> {
		let delta = i128::from(rtp.wrapping_sub(self.rtp) as i32);
		if delta.abs() > kind.rate() * 15 {
			return None;
		}
		Some(
			self.ntp
				.wrapping_add((delta * NTP_SCALE / kind.rate()) as u64),
		)
	}
}
struct Source {
	user: u64,
	audio: Option<Report>,
	video: Option<Report>,
	playout: Option<(u32, Instant)>,
}
struct Clock {
	sources: Vec<Source>,
	generation: u64,
	active: bool,
}
impl Default for Clock {
	fn default() -> Self {
		Self {
			sources: Vec::new(),
			generation: 0,
			active: true,
		}
	}
}
impl Clock {
	fn clear(&mut self) {
		self.sources.clear();
		self.generation = self.generation.wrapping_add(1);
	}
	fn report(&mut self, user: u64, kind: Kind, report: Report) {
		if user == 0 || report.ssrc == 0 || report.ntp == 0 {
			return;
		}
		if !self.sources.iter().any(|source| source.user == user) {
			if self.sources.len() == MAX_USERS {
				return;
			}
			self.sources.push(Source {
				user,
				audio: None,
				video: None,
				playout: None,
			});
		}
		let source = self
			.sources
			.iter_mut()
			.find(|source| source.user == user)
			.expect("source inserted");
		let slot = match kind {
			Kind::Audio => &mut source.audio,
			Kind::Video => &mut source.video,
		};
		if let Some(previous) = *slot {
			if previous.ssrc != report.ssrc {
				// A replacement source cannot borrow either clock from the old stream.
				source.audio = None;
				source.video = None;
				source.playout = None;
				self.generation = self.generation.wrapping_add(1);
			} else {
				let ntp_delta = report.ntp.wrapping_sub(previous.ntp) as i64;
				if ntp_delta <= 0 {
					return;
				} // Duplicate/reordered reports never refresh expiry.
				let expected = previous.time(report.rtp, kind);
				let inconsistent = expected.is_none_or(|expected| {
					(i128::from(report.ntp.wrapping_sub(expected) as i64)).abs() > NTP_SCALE / 2
				});
				if inconsistent {
					source.audio = None;
					source.video = None;
					source.playout = None;
					self.generation = self.generation.wrapping_add(1);
					return;
				}
			}
		}
		match kind {
			Kind::Audio => source.audio = Some(report),
			Kind::Video => source.video = Some(report),
		}
	}
	fn plan(&self, user: u64, timestamp: u32, now: Instant) -> Option<Instant> {
		let source = self.sources.iter().find(|source| source.user == user)?;
		let (audio, video) = (source.audio?, source.video?);
		if now.saturating_duration_since(audio.received) > REPORT_AGE
			|| now.saturating_duration_since(video.received) > REPORT_AGE
		{
			return None;
		}
		let (played, at) = source.playout?;
		if now.saturating_duration_since(at) > AUDIO_AGE {
			return None;
		}
		let audio = audio.time(played, Kind::Audio)?;
		let video = video.time(timestamp, Kind::Video)?;
		let delta = i128::from(video.wrapping_sub(audio) as i64) * 1_000_000_000 / NTP_SCALE;
		// Unknown/conflicting offsets fall back immediately; never build a video backlog.
		if delta.abs() > MAX_WAIT.as_nanos() as i128 {
			return None;
		}
		let duration = Duration::from_nanos(delta.unsigned_abs() as u64);
		let target = if delta >= 0 {
			at.checked_add(duration)?
		} else {
			at.checked_sub(duration)?
		};
		(target.saturating_duration_since(now) <= MAX_WAIT).then_some(target)
	}
}

#[derive(Default)]
pub(crate) struct StreamSync(Mutex<Clock>);
impl StreamSync {
	pub fn report(&self, user: u64, kind: Kind, ssrc: u32, rtp: u32, ntp: u64, now: Instant) {
		if let Ok(mut clock) = self.0.lock() {
			clock.report(
				user,
				kind,
				Report {
					ssrc,
					rtp,
					ntp,
					received: now,
				},
			);
		}
	}
	pub fn audio(&self, user: u64, timestamp: u32, now: Instant) {
		if let Ok(mut clock) = self.0.lock()
			&& let Some(source) = clock.sources.iter_mut().find(|source| source.user == user)
		{
			source.playout = Some((timestamp, now));
		}
	}
	pub fn mixed_audio(&self) {
		if let Ok(mut clock) = self.0.lock() {
			let mut changed = false;
			for source in &mut clock.sources {
				changed |= source.playout.take().is_some();
			}
			if changed {
				clock.generation = clock.generation.wrapping_add(1);
			}
		}
	}
	pub fn reset(&self) {
		if let Ok(mut clock) = self.0.lock() {
			clock.clear();
		}
	}
	pub fn remove(&self, user: u64) {
		if let Ok(mut clock) = self.0.lock() {
			clock.sources.retain(|source| source.user != user);
			clock.generation = clock.generation.wrapping_add(1);
		}
	}
	pub fn stop(&self) {
		if let Ok(mut clock) = self.0.lock() {
			clock.active = false;
			clock.clear();
		}
	}
	/// Decoder-worker only. No renderer/audio callback waits or locks on this clock.
	pub fn present(&self, user: u64, timestamp: u32) -> bool {
		let started = Instant::now();
		let (generation, deadline) = match self.0.lock() {
			Ok(clock) if clock.active => (clock.generation, clock.plan(user, timestamp, started)),
			_ => return false,
		};
		let Some(deadline) = deadline else {
			return true;
		};
		loop {
			if !self
				.0
				.lock()
				.is_ok_and(|clock| clock.active && clock.generation == generation)
			{
				return false;
			}
			let now = Instant::now();
			if now >= deadline || now.saturating_duration_since(started) >= MAX_WAIT {
				return true;
			}
			std::thread::park_timeout(
				deadline
					.saturating_duration_since(now)
					.min(Duration::from_millis(2)),
			);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn reports(sync: &StreamSync, now: Instant, audio: u32, video: u32) {
		let ntp = 100u64 << 32;
		sync.report(7, Kind::Audio, 11, audio, ntp, now);
		sync.report(7, Kind::Video, 12, video, ntp, now);
		sync.audio(7, audio, now);
	}
	#[test]
	fn common_clock_handles_independent_rtp_origins_and_wrap() {
		let sync = StreamSync::default();
		let now = Instant::now();
		reports(&sync, now, u32::MAX - 479, u32::MAX - 899);
		let clock = sync.0.lock().unwrap();
		let actual = clock.plan(7, 900, now).unwrap();
		let expected = now + Duration::from_millis(20);
		assert!(
			actual.saturating_duration_since(expected) + expected.saturating_duration_since(actual)
				<= Duration::from_nanos(1)
		);
		assert!(clock.plan(8, 900, now).is_none());
		assert!(clock.plan(7, 90_000, now).is_none());
	}
	#[test]
	fn absent_stale_reordered_and_conflicting_reports_never_hold_pictures() {
		let sync = StreamSync::default();
		let now = Instant::now();
		assert!(sync.0.lock().unwrap().plan(7, 0, now).is_none());
		reports(&sync, now, 100, 200);
		sync.report(
			7,
			Kind::Audio,
			11,
			0,
			99u64 << 32,
			now + Duration::from_secs(10),
		);
		assert!(
			sync.0
				.lock()
				.unwrap()
				.plan(7, 200, now + Duration::from_secs(16))
				.is_none()
		);
		sync.report(
			7,
			Kind::Audio,
			11,
			100,
			110u64 << 32,
			now + Duration::from_secs(10),
		);
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_none());
	}
	#[test]
	fn rekey_and_replaced_ssrc_revoke_existing_clock_mapping() {
		let sync = StreamSync::default();
		let now = Instant::now();
		reports(&sync, now, 100, 200);
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_some());
		sync.report(7, Kind::Video, 13, 200, 100u64 << 32, now);
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_none());
		reports(&sync, now, 100, 200);
		sync.reset();
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_none());
		reports(&sync, now, 100, 200);
		sync.remove(7);
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_none());
	}
	#[test]
	fn cancellation_interrupts_decoder_wait_without_waiting_for_its_deadline() {
		let sync = std::sync::Arc::new(StreamSync::default());
		let now = Instant::now();
		reports(&sync, now, 100, 200);
		let worker = sync.clone();
		let thread = std::thread::spawn(move || worker.present(7, 9_200));
		// Exercise cancellation while a valid future presentation is scheduled.
		std::thread::sleep(Duration::from_millis(10));
		assert!(!thread.is_finished());
		let cancelled = Instant::now();
		sync.stop();
		assert!(!thread.join().unwrap());
		assert!(cancelled.elapsed() < MAX_WAIT);
	}
	#[test]
	fn mixed_audio_and_source_budget_keep_unknown_clocks_on_immediate_fallback() {
		let sync = StreamSync::default();
		let now = Instant::now();
		reports(&sync, now, 100, 200);
		sync.mixed_audio();
		assert!(sync.0.lock().unwrap().plan(7, 200, now).is_none());
		for user in 1..100 {
			sync.report(user, Kind::Audio, user as u32, 0, 100u64 << 32, now);
		}
		assert_eq!(sync.0.lock().unwrap().sources.len(), MAX_USERS);
		assert!(sync.present(99, 0));
	}
}
