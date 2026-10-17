//! Soundboard playback: credential-free CDN download, bounded decode and a session-only
//! PCM cache. Clips are mixed into the connected call's playback; nothing is written to disk.
use discord_voice::{Effects, MAX_CLIP_SAMPLES};
use eframe::egui;
use model::Id;
use std::{
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, Instant},
};

/// Discord caps an uploaded sound at 512 KiB; the margin covers its bundled defaults.
const MAX_SOUND_BYTES: usize = 1024 * 1024;
const MAX_CACHED: usize = 16;
/// Ten full-length clips: at most 11.5 MB of retained PCM.
const MAX_CACHED_SAMPLES: usize = MAX_CLIP_SAMPLES * 10;
const MAX_LOADS: usize = 4;
/// Plays waiting for a download; further plays of an uncached sound are dropped.
const MAX_WAITING: usize = 16;
/// A sound that finishes loading later than this is kept for next time, not played.
const MAX_LATENESS: Duration = Duration::from_secs(3);

type Clip = Arc<[f32]>;
/// One accepted play whose sound is still downloading. It keeps its own call's mixer, so a
/// play in a new call is not lost to a download started in the previous one.
struct Waiting {
	sound: Id,
	source: Id,
	volume: f32,
	effects: Effects,
	requested: Instant,
}
#[derive(Default)]
struct Shared {
	/// Least recently played first.
	clips: Vec<(Id, Clip)>,
	loading: Vec<Id>,
	waiting: Vec<Waiting>,
}
impl Shared {
	fn insert(&mut self, sound: Id, clip: Clip) {
		self.clips.retain(|(id, _)| *id != sound);
		self.clips.push((sound, clip));
		while self.clips.len() > MAX_CACHED
			|| self.clips.iter().map(|(_, clip)| clip.len()).sum::<usize>() > MAX_CACHED_SAMPLES
		{
			self.clips.remove(0);
		}
	}
	/// Queue every waiting play whose sound has arrived, at the listener's gain right now, so
	/// a participant muted, blocked or turned down during the download stays that way. Late
	/// plays and plays whose download failed are dropped.
	fn flush(&mut self, now: Instant, gain: impl Fn(Id) -> f32) {
		let waiting = std::mem::take(&mut self.waiting);
		for play in waiting {
			if now.duration_since(play.requested) > MAX_LATENESS {
				continue;
			}
			if let Some((_, clip)) = self.clips.iter().find(|(id, _)| *id == play.sound) {
				play.effects
					.play(clip.clone(), play.source.0, play.volume, gain(play.source));
			} else if self.loading.contains(&play.sound) {
				self.waiting.push(play);
			}
		}
	}
}
#[derive(Default)]
pub struct Soundboard {
	shared: Arc<Mutex<Shared>>,
	client: Option<reqwest::Client>,
	cancel: Arc<AtomicBool>,
	tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Soundboard {
	pub fn clear(&mut self) {
		self.cancel.store(true, Ordering::Release);
		for task in self.tasks.drain(..) {
			task.abort();
		}
		self.shared = Arc::default();
		self.cancel = Arc::default();
		self.client = None;
	}
	/// Mix `sound` played by `source` into `effects` at `volume * gain`, downloading it first
	/// when it is not cached. A downloaded sound is queued by the next `flush`, which
	/// `context` is woken for. Best effort: a failed or slow download plays nothing.
	#[allow(clippy::too_many_arguments)] // One play: where, what, by whom and how loud.
	pub fn play(
		&mut self,
		runtime: &tokio::runtime::Runtime,
		context: &egui::Context,
		effects: &Effects,
		sound: Id,
		source: Id,
		volume: f32,
		gain: f32,
	) {
		if sound.0 == 0 || !(volume * gain).is_finite() || volume * gain <= 0.0 {
			return;
		}
		self.tasks.retain(|task| !task.is_finished());
		let Ok(mut shared) = self.shared.lock() else {
			return;
		};
		if let Some(index) = shared.clips.iter().position(|(id, _)| *id == sound) {
			let entry = shared.clips.remove(index);
			effects.play(entry.1.clone(), source.0, volume, gain);
			shared.clips.push(entry);
			return;
		}
		let loading = shared.loading.contains(&sound);
		if shared.waiting.len() >= MAX_WAITING
			|| (!loading && (shared.loading.len() >= MAX_LOADS || self.tasks.len() >= MAX_LOADS))
		{
			return;
		}
		// Each accepted play waits on its own; repeated plays share one download.
		shared.waiting.push(Waiting {
			sound,
			source,
			volume,
			effects: effects.clone(),
			requested: Instant::now(),
		});
		if loading {
			return;
		}
		if self.client.is_none() {
			self.client = reqwest::Client::builder()
				.https_only(true)
				.no_proxy()
				.redirect(reqwest::redirect::Policy::none())
				.timeout(Duration::from_secs(10))
				.connect_timeout(Duration::from_secs(5))
				.pool_max_idle_per_host(1)
				.build()
				.ok();
		}
		let Some(client) = self.client.clone() else {
			shared.waiting.retain(|play| play.sound != sound);
			return;
		};
		shared.loading.push(sound);
		drop(shared);
		let shared = self.shared.clone();
		let context = context.clone();
		let cancel = self.cancel.clone();
		self.tasks.push(runtime.spawn(async move {
			let clip = load(&client, sound, cancel.clone()).await;
			if cancel.load(Ordering::Acquire) {
				return;
			}
			if let Ok(mut shared) = shared.lock() {
				shared.loading.retain(|id| *id != sound);
				match clip {
					Ok(clip) => shared.insert(sound, clip),
					Err(_) => shared.waiting.retain(|play| play.sound != sound),
				}
			}
			// The interface thread queues the clip with the listener's current levels.
			context.request_repaint();
		}));
	}

	/// Queue downloaded sounds for their waiting plays, using the listener gain for each
	/// sender as it is now. Called from the interface thread on every voice update.
	pub fn flush(&mut self, gain: impl Fn(Id) -> f32) {
		if let Ok(mut shared) = self.shared.lock()
			&& !shared.waiting.is_empty()
		{
			shared.flush(Instant::now(), gain);
		}
	}
	/// Deafen retires pending plays; downloads may finish only for a later explicit play.
	pub fn discard_waiting(&mut self) {
		if let Ok(mut shared) = self.shared.lock() {
			shared.waiting.clear();
		}
	}
}
impl Drop for Soundboard {
	fn drop(&mut self) {
		self.cancel.store(true, Ordering::Release);
		for task in self.tasks.drain(..) {
			task.abort();
		}
	}
}
async fn load(
	client: &reqwest::Client,
	sound: Id,
	cancel: Arc<AtomicBool>,
) -> Result<Clip, &'static str> {
	// The path holds only the numeric sound ID; no credential or account data is sent.
	let mut response = client
		.get(format!(
			"https://cdn.discordapp.com/soundboard-sounds/{sound}"
		))
		.send()
		.await
		.map_err(|_| "Sound download failed")?;
	if response.status() != reqwest::StatusCode::OK
		|| response
			.content_length()
			.is_some_and(|size| size > MAX_SOUND_BYTES as u64)
	{
		return Err("Sound unavailable");
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| "Sound download interrupted")?
	{
		if chunk.len() > MAX_SOUND_BYTES - bytes.len() {
			return Err("Sound is too large");
		}
		bytes.extend_from_slice(&chunk);
	}
	// Decoding is CPU work: keep it off the async workers and the UI thread.
	tokio::task::spawn_blocking(move || {
		crate::audio::decode_clip_using(bytes, MAX_CLIP_SAMPLES, &|| {
			!cancel.load(Ordering::Acquire)
		})
	})
	.await
	.map_err(|_| "Sound decoding stopped")?
	.map(Arc::from)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn ending_a_call_cancels_decoders_and_retires_cached_pcm() {
		let mut player = Soundboard::default();
		let cancelled = player.cancel.clone();
		let pcm: Clip = vec![0.25; 960].into();
		let old_shared = Arc::downgrade(&player.shared);
		player.shared.lock().unwrap().insert(Id(7), pcm.clone());
		assert_eq!(Arc::strong_count(&pcm), 2);
		player.clear();
		assert!(cancelled.load(Ordering::Acquire));
		assert!(!player.cancel.load(Ordering::Acquire));
		assert!(old_shared.upgrade().is_none());
		assert_eq!(Arc::strong_count(&pcm), 1);
		assert!(player.shared.lock().unwrap().clips.is_empty());
		assert!(crate::audio::decode_clip_using(Vec::new(), 960, &|| false).is_err());
	}
	#[test]
	fn deafen_discards_pending_plays_even_if_the_download_finishes_after_undeafen() {
		let mut board = Soundboard::default();
		let effects = Effects::default();
		board.shared.lock().unwrap().waiting.push(Waiting {
			sound: Id(7),
			source: Id(2),
			volume: 1.0,
			effects: effects.clone(),
			requested: Instant::now(),
		});
		board.discard_waiting();
		board
			.shared
			.lock()
			.unwrap()
			.insert(Id(7), vec![0.25; 960].into());
		board.flush(|_| 1.0);
		assert_eq!(effects.queued(), 0);
		assert_eq!(
			board.shared.lock().unwrap().clips.len(),
			1,
			"cached audio remains available for an explicit new play"
		);
	}
	#[test]
	fn cache_keeps_recent_clips_within_item_and_sample_limits() {
		let mut shared = Shared::default();
		for id in 1..=MAX_CACHED as u64 + 2 {
			shared.insert(Id(id), vec![0.0; 8].into());
		}
		assert_eq!(shared.clips.len(), MAX_CACHED);
		assert_eq!(shared.clips[0].0, Id(3));
		// Replacing an entry never duplicates it; full-length clips evict by samples.
		shared.insert(Id(3), vec![0.0; 16].into());
		assert_eq!(shared.clips.len(), MAX_CACHED);
		assert_eq!(shared.clips.last().unwrap().1.len(), 16);
		for id in 100..112 {
			shared.insert(Id(id), vec![0.0; MAX_CLIP_SAMPLES].into());
		}
		assert_eq!(shared.clips.len(), 10);
		assert!(shared.clips.iter().all(|(id, _)| id.0 >= 102));
	}
	#[test]
	fn waiting_plays_use_current_gain_keep_repeats_and_drop_late_or_failed_downloads() {
		let now = Instant::now();
		let (first, second) = (Effects::default(), Effects::default());
		let waiting = |sound: u64, source: u64, effects: &Effects, age: Duration| Waiting {
			sound: Id(sound),
			source: Id(source),
			volume: 1.0,
			effects: effects.clone(),
			requested: now - age,
		};
		let mut shared = Shared {
			loading: vec![Id(7), Id(8)],
			..Default::default()
		};
		// Two plays of one sound by different people, one in a newer call, plus another
		// sound that is still downloading.
		shared.waiting = vec![
			waiting(7, 2, &first, Duration::ZERO),
			waiting(7, 3, &first, Duration::ZERO),
			waiting(7, 2, &second, Duration::ZERO),
			waiting(8, 2, &first, Duration::ZERO),
		];
		shared.flush(now, |_| 1.0);
		assert_eq!(shared.waiting.len(), 4, "nothing plays before its download");
		assert_eq!((first.queued(), second.queued()), (0, 0));

		// Sender 3 was muted while the download ran: only the other plays are queued.
		shared.loading.retain(|id| *id != Id(7));
		shared.insert(Id(7), vec![0.25; 960].into());
		shared.flush(now, |source| if source == Id(3) { 0.0 } else { 0.5 });
		assert_eq!((first.queued(), second.queued()), (1, 1));
		assert_eq!(shared.waiting.len(), 1);
		assert_eq!(shared.waiting[0].sound, Id(8));

		// A failed download leaves nothing loading, and a late arrival is not played.
		shared.loading.clear();
		shared.flush(now, |_| 1.0);
		assert!(shared.waiting.is_empty());
		shared.waiting = vec![waiting(
			7,
			2,
			&first,
			MAX_LATENESS + Duration::from_millis(1),
		)];
		shared.flush(now, |_| 1.0);
		assert!(shared.waiting.is_empty());
		assert_eq!(first.queued(), 1);
	}
	#[test]
	fn clips_decode_to_bounded_48khz_mono_and_reject_damaged_audio() {
		for bytes in [
			include_bytes!("../tests/fixtures/audio-tone.mp3").to_vec(),
			include_bytes!("../tests/fixtures/voice-message.ogg").to_vec(),
		] {
			let clip = crate::audio::decode_clip(bytes.clone(), MAX_CLIP_SAMPLES).unwrap();
			assert!(!clip.is_empty() && clip.len() <= MAX_CLIP_SAMPLES);
			assert!(clip.iter().all(|sample| sample.abs() <= 1.0));
			assert!(clip.iter().any(|sample| sample.abs() > 0.001));
			// A longer sound is truncated to the budget, never rejected or over-allocated.
			let short = crate::audio::decode_clip(bytes, 4800).unwrap();
			assert_eq!(short.len(), 4800);
			assert_eq!(short[..100], clip[..100]);
		}
		assert!(crate::audio::decode_clip(Vec::new(), MAX_CLIP_SAMPLES).is_err());

		// A file longer than the streaming decoder accepts still yields its start: 601 s of
		// 8 kHz mono exceeds the ten-minute limit that rejects such files elsewhere.
		let data = vec![128u8; 8000 * 601];
		let mut wav = Vec::with_capacity(44 + data.len());
		wav.extend_from_slice(b"RIFF");
		wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
		wav.extend_from_slice(b"WAVEfmt ");
		for field in [16u32, 1 | 1 << 16, 8000, 8000, 1 | 8 << 16] {
			wav.extend_from_slice(&field.to_le_bytes());
		}
		wav.extend_from_slice(b"data");
		wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
		wav.extend_from_slice(&data);
		let clip = crate::audio::decode_clip(wav, 48_000).unwrap();
		assert_eq!(clip.len(), 48_000);
		assert!(crate::audio::decode_clip(vec![0x42; 4096], MAX_CLIP_SAMPLES).is_err());
	}
}
