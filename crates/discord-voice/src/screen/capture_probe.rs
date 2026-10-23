//! Explicit, owner-operated Windows capture isolation. Never used by ordinary startup or CI.
//!
//! Counters contain no pixels, source titles, account identity or signaling. The native
//! capture worker updates this fixed-size state; this module samples totals off its callback.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeMode {
	/// Acquire and release admitted native pictures, without readback or encoding.
	CaptureOnly,
	/// Production capture/readback, consumed and converted without encoding.
	Readback,
}

#[derive(Default)]
pub struct ProbeStats {
	pub(crate) input: AtomicU64,
	pub(crate) admitted: AtomicU64,
	pub(crate) skipped: AtomicU64,
	pub(crate) completed: AtomicU64,
	pub(crate) readback_bytes: AtomicU64,
	pub(crate) readback_ns: AtomicU64,
	pub(crate) readback_max_ns: AtomicU64,
	pub(crate) submit_ns: AtomicU64,
	pub(crate) submit_max_ns: AtomicU64,
	pub(crate) resources_created: AtomicU64,
	pub(crate) gpu_frames: AtomicU64,
	pub(crate) source_width: AtomicU64,
	pub(crate) source_height: AtomicU64,
	pub(crate) output_width: AtomicU64,
	pub(crate) output_height: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeSnapshot {
	pub input: u64,
	pub admitted: u64,
	pub skipped: u64,
	pub completed: u64,
	pub readback_bytes: u64,
	pub readback_ns: u64,
	/// Lifetime maximum, including warmup; unlike totals this cannot be subtracted.
	pub readback_max_ns: u64,
	pub submit_ns: u64,
	/// Lifetime maximum, including warmup.
	pub submit_max_ns: u64,
	pub resources_created: u64,
	pub gpu_frames: u64,
	pub source_width: u64,
	pub source_height: u64,
	pub output_width: u64,
	pub output_height: u64,
}

impl ProbeStats {
	pub fn snapshot(&self) -> ProbeSnapshot {
		let load = |value: &AtomicU64| value.load(Ordering::Relaxed);
		ProbeSnapshot {
			input: load(&self.input),
			admitted: load(&self.admitted),
			skipped: load(&self.skipped),
			completed: load(&self.completed),
			readback_bytes: load(&self.readback_bytes),
			readback_ns: load(&self.readback_ns),
			readback_max_ns: load(&self.readback_max_ns),
			submit_ns: load(&self.submit_ns),
			submit_max_ns: load(&self.submit_max_ns),
			resources_created: load(&self.resources_created),
			gpu_frames: load(&self.gpu_frames),
			source_width: load(&self.source_width),
			source_height: load(&self.source_height),
			output_width: load(&self.output_width),
			output_height: load(&self.output_height),
		}
	}
}

impl ProbeSnapshot {
	/// Monotonic totals across a bounded sample; metadata and maxima come from its end.
	pub fn since(self, start: Self) -> Self {
		Self {
			input: self.input.saturating_sub(start.input),
			admitted: self.admitted.saturating_sub(start.admitted),
			skipped: self.skipped.saturating_sub(start.skipped),
			completed: self.completed.saturating_sub(start.completed),
			readback_bytes: self.readback_bytes.saturating_sub(start.readback_bytes),
			readback_ns: self.readback_ns.saturating_sub(start.readback_ns),
			submit_ns: self.submit_ns.saturating_sub(start.submit_ns),
			resources_created: self
				.resources_created
				.saturating_sub(start.resources_created),
			gpu_frames: self.gpu_frames.saturating_sub(start.gpu_frames),
			..self
		}
	}

	pub fn json(self) -> serde_json::Value {
		serde_json::json!({
			"input": self.input,
			"admitted": self.admitted,
			"skipped": self.skipped,
			"completed": self.completed,
			"readback_bytes": self.readback_bytes,
			"readback_ns": self.readback_ns,
			"readback_max_ns_including_warmup": self.readback_max_ns,
			"submit_ns": self.submit_ns,
			"submit_max_ns_including_warmup": self.submit_max_ns,
			"resources_created": self.resources_created,
			"gpu_frames": self.gpu_frames,
			"source_dimensions": [self.source_width, self.source_height],
			"output_dimensions": [self.output_width, self.output_height],
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn probes_require_the_owner_source_and_exact_encoder_identity() {
		let mut request = Request {
			owner_controlled: true,
			settings: super::super::RequestSettings {
				source: super::super::SourceId::Display(1),
				resolution: client_core::screen::Resolution::Preset(
					model::voice_settings::VideoResolution::P1440,
				),
				fps: 60,
				cursor: true,
				audio: false,
			},
			video: model::voice_settings::VideoSettings::default(),
			adapter: None,
			stage: Stage::Readback,
			warmup_seconds: 10,
			sample_seconds: 30,
		};
		assert!(request.validate().is_ok());
		request.owner_controlled = false;
		assert!(request.validate().is_err());
		request.owner_controlled = true;
		request.settings.audio = true;
		assert!(request.validate().is_err());
		request.settings.audio = false;
		request.stage = Stage::Encode;
		assert!(request.validate().is_err());
		request.adapter = Some(model::VideoAdapter {
			vendor_id: 0x1002,
			device_id: 0x744c,
			identity: model::VideoAdapterIdentity::WindowsLuid(0x1234),
		});
		assert!(request.validate().is_ok());
		request.sample_seconds = 601;
		assert!(request.validate().is_err());
		request.sample_seconds = 30;
		request.settings.source = super::super::SourceId::SystemPicker;
		assert!(request.validate().is_err());
	}

	#[test]
	fn sampling_excludes_warmup_totals_but_preserves_peak_labels_and_geometry() {
		let stats = ProbeStats::default();
		stats.input.store(10, Ordering::Relaxed);
		stats.completed.store(8, Ordering::Relaxed);
		stats.resources_created.store(3, Ordering::Relaxed);
		stats.readback_ns.store(30, Ordering::Relaxed);
		stats.readback_max_ns.store(20, Ordering::Relaxed);
		let start = stats.snapshot();
		stats.input.fetch_add(6, Ordering::Relaxed);
		stats.completed.fetch_add(5, Ordering::Relaxed);
		stats.readback_ns.fetch_add(12, Ordering::Relaxed);
		stats.source_width.store(6144, Ordering::Relaxed);
		stats.source_height.store(2560, Ordering::Relaxed);
		stats.output_width.store(3456, Ordering::Relaxed);
		stats.output_height.store(1440, Ordering::Relaxed);
		let sample = stats.snapshot().since(start);
		assert_eq!((sample.input, sample.completed), (6, 5));
		assert_eq!(sample.resources_created, 0);
		assert_eq!(sample.readback_ns, 12);
		assert_eq!(sample.readback_max_ns, 20);
		assert_eq!((sample.source_width, sample.output_width), (6144, 3456));
		let json = sample.json();
		assert_eq!(json["readback_max_ns_including_warmup"], 20);
		assert_eq!(json["output_dimensions"], serde_json::json!([3456, 1440]));
	}
}

/// Deliberately separate from Discord delivery: Encode drains packets into a local sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
	CaptureOnly,
	Readback,
	Encode,
}
impl Stage {
	pub fn label(self) -> &'static str {
		match self {
			Self::CaptureOnly => "capture-only",
			Self::Readback => "readback",
			Self::Encode => "encode",
		}
	}
}

#[derive(Clone, Copy)]
pub struct Request {
	pub owner_controlled: bool,
	pub settings: super::RequestSettings,
	pub video: model::voice_settings::VideoSettings,
	pub adapter: Option<model::VideoAdapter>,
	pub stage: Stage,
	pub warmup_seconds: u32,
	pub sample_seconds: u32,
}
impl Request {
	pub fn validate(self) -> Result<Self, &'static str> {
		if !self.owner_controlled {
			return Err("Explicit owner-controlled capture is required");
		}
		if !self.settings.valid()
			|| self.settings.audio
			|| !self.video.is_valid()
			|| !matches!(
				self.settings.source,
				super::SourceId::Display(_) | super::SourceId::Window(_)
			) || self.warmup_seconds > 60
			|| !(5..=600).contains(&self.sample_seconds)
		{
			return Err("Invalid bounded capture probe settings");
		}
		if self.stage == Stage::Encode
			&& !self.adapter.is_some_and(|adapter| {
				matches!(
					adapter.identity,
					model::VideoAdapterIdentity::WindowsLuid(_)
				)
			}) {
			return Err("Encode comparison requires the exact Serein Windows adapter helper key");
		}
		Ok(self)
	}
}

/// Runs only after explicit source selection; never opens Discord or an audio device.
#[cfg(target_os = "windows")]
pub fn run(request: Request, mut emit: impl FnMut(serde_json::Value)) -> Result<(), &'static str> {
	use std::{
		sync::{Arc, atomic::AtomicBool, mpsc},
		time::{Duration, Instant},
	};
	let request = request.validate()?;
	let stats = Arc::new(ProbeStats::default());
	let stop = Arc::new(AtomicBool::new(false));
	let mut converted = 0u64;
	let mut conversion_ns = 0u64;
	let mut encoded = 0u64;
	let mut keyframes = 0u64;
	let mut output_bytes = 0u64;
	let mut sink = Vec::new();
	let mut download = Vec::new();
	let mut converter = super::convert::Converter::default();
	let (send, raw) = mpsc::sync_channel(1);
	let native = if request.stage == Stage::Encode {
		None
	} else {
		Some(super::capture::Capture::start_probe(
			request.settings,
			send,
			stop.clone(),
			if request.stage == Stage::CaptureOnly {
				ProbeMode::CaptureOnly
			} else {
				ProbeMode::Readback
			},
			stats.clone(),
			request.adapter,
		)?)
	};
	let mut worker = if request.stage == Stage::Encode {
		Some(super::Worker::start_on_adapter(
			request.settings,
			request.video,
			request.adapter,
			|| {},
		)?)
	} else {
		None
	};
	if let Some((worker, _)) = &worker {
		worker.set_preview_visible(false);
	}
	let started = Instant::now();
	let warmup = Duration::from_secs(u64::from(request.warmup_seconds));
	let duration = Duration::from_secs(u64::from(request.sample_seconds));
	let deadline = started + warmup + duration;
	let mut sample_started = None;
	let mut initial = ProbeSnapshot::default();
	let mut initial_converted = 0;
	let mut initial_conversion_ns = 0;
	let mut initial_encoded = 0;
	let mut initial_keyframes = 0;
	let mut initial_output_bytes = 0;
	let mut first_picture = false;
	let mut source_dimensions = None;
	let mut output_dimensions = None;
	let mut announced = None;
	let result = (|| {
		while Instant::now() < deadline {
			if let Some(capture) = &native {
				if stop.load(Ordering::Acquire) || capture.failed() {
					return Err("Capture ended before the probe completed");
				}
				match raw.recv_timeout(Duration::from_millis(20)) {
					Ok(frame) => {
						capture.release_input();
						first_picture = true;
						let source = frame.source_dimensions();
						let resolved = request
							.settings
							.resolve(source)
							.ok_or("Probe source dimensions are unsupported")?;
						source_dimensions = Some(source);
						output_dimensions = Some((resolved.width, resolved.height));
						conversion_ns = conversion_ns.saturating_add(convert(
							&frame,
							resolved,
							&mut sink,
							&mut download,
							&stats,
							&mut converter,
						)?);
						converted += 1;
					}
					Err(mpsc::RecvTimeoutError::Timeout) => {}
					Err(mpsc::RecvTimeoutError::Disconnected) => {
						return Err("Capture closed before the probe completed");
					}
				}
				let current = stats.snapshot();
				if current.input > 0 {
					first_picture = true;
				}
				if current.source_width > 0 && current.source_height > 0 {
					source_dimensions =
						Some((current.source_width as u32, current.source_height as u32));
					output_dimensions =
						Some((current.output_width as u32, current.output_height as u32));
				}
			}
			if let Some((worker, video)) = &mut worker {
				if let Some(result) = worker.result() {
					result?;
					return Err("Encoder ended before the probe completed");
				}
				if let Some(output) = video.output() {
					first_picture = true;
					source_dimensions = Some(output.source_dimensions);
					output_dimensions = Some((output.settings.width, output.settings.height));
					if announced != Some(output) {
						if !video.enable_output(output) {
							return Err("Probe geometry changed before it could be enabled");
						}
						announced = Some(output);
					}
				}
				while let Ok(frame) = video.frames.try_recv() {
					encoded += 1;
					keyframes += u64::from(frame.keyframe);
					output_bytes += frame.data.len() as u64;
				}
				std::thread::sleep(Duration::from_millis(2));
			}
			let now = Instant::now();
			if !first_picture && now.duration_since(started) > Duration::from_secs(15) {
				return Err(
					"No capture picture within 15 seconds; check the source and screen-recording access",
				);
			}
			if sample_started.is_none() && now.duration_since(started) >= warmup {
				initial = stats.snapshot();
				initial_converted = converted;
				initial_conversion_ns = conversion_ns;
				initial_encoded = encoded;
				initial_keyframes = keyframes;
				initial_output_bytes = output_bytes;
				sample_started = Some(now);
				emit(
					serde_json::json!({"serein_capture_probe":"start", "stage":request.stage.label(),
					"process_id":std::process::id(), "requested_sample_seconds":request.sample_seconds,
					"source_dimensions":source_dimensions,"output_dimensions":output_dimensions}),
				);
			}
		}
		if !first_picture {
			return Err("No capture picture arrived during the probe");
		}
		if request.stage == Stage::Encode && encoded == 0 {
			return Err("No encoded picture arrived during the probe");
		}
		let sample_seconds = sample_started
			.ok_or("Probe never entered the measurement window")?
			.elapsed()
			.as_secs_f64();
		Ok(serde_json::json!({
			"serein_capture_probe":"complete", "stage":request.stage.label(), "process_id":std::process::id(),
			"warmup_seconds":request.warmup_seconds,"sample_seconds":sample_seconds,
			"source_dimensions":source_dimensions,"output_dimensions":output_dimensions,
			"configured_fps":request.settings.fps,"requested_backend":format!("{:?}",request.video.backend),
			"requested_codec":format!("{:?}",request.video.codec),"adapter":request.adapter.map(|adapter|adapter.helper_key()),
			"capture":if request.stage == Stage::Encode {serde_json::Value::Null} else {stats.snapshot().since(initial).json()},
			"converted_pictures":if request.stage == Stage::Readback {Some(converted - initial_converted)} else {None},
			"conversion_ns":if request.stage == Stage::Readback {Some(conversion_ns - initial_conversion_ns)} else {None},
			"encoded_pictures":encoded - initial_encoded,"keyframes":keyframes - initial_keyframes,
			"encoded_bytes":output_bytes - initial_output_bytes,
			"method":"Explicit native capture; encode stage drains into a local sink without Discord transport. Atomic totals can cross a boundary by an in-progress callback. Timing maxima include warmup. Encode-stage capture timing is unavailable in this standalone report; use the production diagnostic output for its effective encoder."
		}))
	})();
	stop.store(true, Ordering::Release);
	let shutdown = if let Some((worker, _)) = worker.take() {
		worker
			.shutdown()
			.recv_timeout(Duration::from_secs(5))
			.map_err(|_| "Capture worker did not retire within five seconds")
			.and_then(|result| result)
	} else {
		Ok(())
	};
	drop(native);
	let report = result?;
	shutdown?;
	emit(report);
	Ok(())
}

#[cfg(target_os = "windows")]
fn convert(
	frame: &super::CapturedFrame,
	settings: super::Settings,
	output: &mut Vec<u8>,
	download: &mut Vec<u8>,
	stats: &ProbeStats,
	converter: &mut super::convert::Converter,
) -> Result<u64, &'static str> {
	frame.validate()?;
	let (width, height) = (settings.width as usize, settings.height as usize);
	output.resize(width * height * 3 / 2, 0);
	let super::CapturedFrame::Gpu(gpu) = frame;
	let started = std::time::Instant::now();
	let description = gpu.download_into(download)?;
	let readback_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
	stats.readback_ns.fetch_add(readback_ns, Ordering::Relaxed);
	stats
		.readback_max_ns
		.fetch_max(readback_ns, Ordering::Relaxed);
	stats
		.readback_bytes
		.fetch_add(download.len() as u64, Ordering::Relaxed);
	use platform::video::capture::PixelFormat;
	let format = match description.format {
		PixelFormat::Bgra8 => super::RawFormat::Bgra8,
		PixelFormat::Nv12 => super::RawFormat::Nv12,
		PixelFormat::LinearRgba16 {
			unit_nits,
			white_nits,
			peak_nits,
		} => super::RawFormat::LinearRgba16 {
			unit_nits,
			white_nits,
			peak_nits,
		},
	};
	let raw = super::RawFrame {
		format,
		width: description.width,
		height: description.height,
		stride: description.stride,
		data: std::mem::take(download),
	};
	let started = std::time::Instant::now();
	let result = converter.convert(&raw, width, height, output);
	let conversion_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
	*download = raw.data;
	result.map(|()| conversion_ns)
}
