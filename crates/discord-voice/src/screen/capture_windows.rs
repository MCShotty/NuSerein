#![allow(unsafe_code)] // Narrow checked COM-pointer handoff to the platform capture owner.
use super::{
	MAX_FRAME_HEIGHT, MAX_FRAME_WIDTH, MAX_SOURCE_HEIGHT, MAX_SOURCE_WIDTH, MAX_SOURCES,
	bounded_name,
};
use crate::screen::probe::{ProbeMode, ProbeSnapshot, ProbeStats};
use crate::screen::{
	AudioChunk, CapturedFrame, RequestSettings as CaptureSettings, Source, SourceId,
};
use platform::video::capture::{PixelFormat, Processor, Stats as NativeStats};
use std::sync::{
	Arc, Condvar, Mutex,
	atomic::{AtomicBool, AtomicU64, Ordering},
	mpsc::SyncSender,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_capture::{
	capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
	frame::Frame,
	graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
	monitor::Monitor,
	settings::{
		ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
		MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
	},
	window::Window,
};
use windows_capture_core::Interface;

#[path = "audio_windows.rs"]
mod audio;

fn monitor_id(monitor: &Monitor) -> u64 {
	u64::try_from(monitor.as_raw_hmonitor() as usize).unwrap_or(0)
}

fn window_id(window: &Window) -> u64 {
	u64::try_from(window.as_raw_hwnd() as usize).unwrap_or(0)
}

pub(crate) fn sources() -> Result<Vec<Source>, &'static str> {
	let mut sources = Vec::with_capacity(MAX_SOURCES);
	for monitor in Monitor::enumerate().map_err(|_| "Displays could not be enumerated")? {
		// EnumDisplaySettings reports physical mode pixels, independent of DPI scaling.
		let (Ok(width), Ok(height)) = (monitor.width(), monitor.height()) else {
			continue;
		};
		if !crate::screen::valid_dimensions(width, height)
			|| width > MAX_SOURCE_WIDTH
			|| height > MAX_SOURCE_HEIGHT
		{
			continue;
		}
		let name = monitor
			.name()
			.or_else(|_| monitor.device_string())
			.unwrap_or_else(|_| "Display".to_owned());
		sources.push(Source {
			id: SourceId::Display(monitor_id(&monitor)),
			name: bounded_name(name),
			dimensions: Some((width, height)),
		});
		if sources.len() == MAX_SOURCES {
			return Ok(sources);
		}
	}

	for window in Window::enumerate().map_err(|_| "Windows could not be enumerated")? {
		let Ok(title) = window.title() else {
			continue;
		};
		let (Ok(width), Ok(height)) = (window.width(), window.height()) else {
			continue;
		};
		let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
			continue;
		};
		if title.trim().is_empty()
			|| width == 0
			|| height == 0
			|| width > MAX_SOURCE_WIDTH
			|| height > MAX_SOURCE_HEIGHT
		{
			continue;
		}
		sources.push(Source {
			id: SourceId::Window(window_id(&window)),
			name: bounded_name(title),
			dimensions: None,
		});
		if sources.len() == MAX_SOURCES {
			break;
		}
	}
	Ok(sources)
}

#[derive(Clone)]
struct Flags {
	frames: SyncSender<CapturedFrame>,
	stop: Arc<AtomicBool>,
	cancel: Arc<AtomicBool>,
	pending: Arc<AtomicBool>,
	next_frame: Instant,
	interval: Duration,
	luminance: Option<platform::video::ffmpeg::CaptureLuminance>,
	source: SourceId,
	next_color: Instant,
	settings: CaptureSettings,
	adapter: Option<model::VideoAdapter>,
	gpu: bool,
	probe: Option<ProbeMode>,
	stats: Arc<ProbeStats>,
	processing: Arc<(Mutex<Processing>, Condvar)>,
}

#[derive(Default)]
struct Processing {
	processor: Option<Processor>,
	signature: Option<(u32, u32, u32, u32, bool)>,
	previous: NativeStats,
	gpu_disabled: bool,
}

struct Handler(Flags);

impl GraphicsCaptureApiHandler for Handler {
	type Flags = Flags;
	type Error = &'static str;

	fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
		let flags = context.flags;
		// SAFETY: windows-capture owns matching live D3D interfaces; the native
		// Processor immediately takes its own COM references.
		let processor = unsafe {
			Processor::new(
				context.device.as_raw(),
				context.device_context.as_raw(),
				flags.adapter,
				flags.gpu,
			)
		}?;
		flags
			.processing
			.0
			.lock()
			.map_err(|_| "Capture processing unavailable")?
			.processor = Some(processor);
		Ok(Self(flags))
	}

	fn on_frame_arrived(
		&mut self,
		frame: &mut Frame,
		control: InternalCaptureControl,
	) -> Result<(), Self::Error> {
		let flags = &mut self.0;
		if flags.stop.load(Ordering::Acquire) || flags.cancel.load(Ordering::Acquire) {
			control.stop();
			return Ok(());
		}
		flags.stats.input.fetch_add(1, Ordering::Relaxed);
		let now = Instant::now();
		if now + Duration::from_millis(2) < flags.next_frame
			|| flags.pending.swap(true, Ordering::AcqRel)
		{
			flags.stats.skipped.fetch_add(1, Ordering::Relaxed);
			return Ok(());
		}
		flags.next_frame = (flags.next_frame + flags.interval).max(now + flags.interval / 2);
		let source = (frame.width(), frame.height());
		if !crate::screen::valid_dimensions(source.0, source.1) {
			flags.pending.store(false, Ordering::Release);
			return Err("Captured frame exceeds the 8K limit");
		}
		let resolved = flags
			.settings
			.resolve(source)
			.ok_or("Unsupported screen source dimensions")?;
		flags
			.stats
			.source_width
			.store(u64::from(source.0), Ordering::Relaxed);
		flags
			.stats
			.source_height
			.store(u64::from(source.1), Ordering::Relaxed);
		flags
			.stats
			.output_width
			.store(u64::from(resolved.width), Ordering::Relaxed);
		flags
			.stats
			.output_height
			.store(u64::from(resolved.height), Ordering::Relaxed);
		flags.stats.admitted.fetch_add(1, Ordering::Relaxed);
		if flags.probe == Some(ProbeMode::CaptureOnly) {
			flags.stats.completed.fetch_add(1, Ordering::Relaxed);
			flags.pending.store(false, Ordering::Release);
			return Ok(());
		}
		let format = match frame.color_format() {
			ColorFormat::Bgra8 => PixelFormat::Bgra8,
			ColorFormat::Rgba16F => {
				if now >= flags.next_color {
					flags.luminance = match flags.source {
						SourceId::Display(id) => {
							platform::video::ffmpeg::capture_luminance(id, false)
						}
						SourceId::Window(id) => {
							platform::video::ffmpeg::capture_luminance(id, true)
						}
						_ => None,
					};
					flags.next_color = now + Duration::from_secs(1);
				}
				let light = flags
					.luminance
					.unwrap_or(platform::video::ffmpeg::CaptureLuminance {
						unit_nits: 80,
						white_nits: 80,
						peak_nits: 80,
					});
				PixelFormat::LinearRgba16 {
					unit_nits: light.unit_nits,
					white_nits: light.white_nits,
					peak_nits: light.peak_nits,
				}
			}
			_ => {
				flags.pending.store(false, Ordering::Release);
				return Err("Unsupported capture color format");
			}
		};
		let Ok(mut processing) = flags.processing.0.try_lock() else {
			flags.pending.store(false, Ordering::Release);
			flags.stats.skipped.fetch_add(1, Ordering::Relaxed);
			return Ok(());
		};
		let signature = (
			source.0,
			source.1,
			resolved.width,
			resolved.height,
			matches!(format, PixelFormat::LinearRgba16 { .. }),
		);
		let cpu_requested = processing
			.processor
			.as_ref()
			.is_some_and(Processor::wants_cpu);
		if processing
			.signature
			.is_some_and(|previous| previous != signature)
			|| cpu_requested
		{
			// New geometry receives a new pool. Frames and hardware refs from
			// the previous pool remain valid and retire independently.
			processing.gpu_disabled |= cpu_requested;
			if let Some(processor) = processing.processor.as_ref() {
				processing.processor =
					Some(processor.replacement(flags.gpu && !processing.gpu_disabled)?);
			}
			processing.previous = NativeStats::default();
		}
		processing.signature = Some(signature);
		let started = Instant::now();
		let mut submitted = false;
		for attempt in 0..3 {
			let processor = processing
				.processor
				.as_mut()
				.ok_or("Capture processing unavailable")?;
			// SAFETY: This live texture belongs to the device supplied in new().
			submitted = unsafe {
				processor.submit(
					frame.as_raw_texture().as_raw(),
					source,
					(resolved.width, resolved.height),
					format,
				)
			}?;
			if submitted || !processor.wants_cpu() || attempt == 2 {
				break;
			}
			// Retry this same WGC picture. An idle source might never produce
			// another callback after the first unsupported processor operation.
			let current = processor.stats();
			let next = processor.replacement(false)?;
			flags.stats.resources_created.fetch_add(
				current
					.resources_created
					.saturating_sub(processing.previous.resources_created),
				Ordering::Relaxed,
			);
			processing.processor = Some(next);
			processing.previous = NativeStats::default();
			processing.gpu_disabled = true;
		}
		let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
		flags.stats.submit_ns.fetch_add(elapsed, Ordering::Relaxed);
		flags
			.stats
			.submit_max_ns
			.fetch_max(elapsed, Ordering::Relaxed);
		if !submitted {
			flags.pending.store(false, Ordering::Release);
			flags.stats.skipped.fetch_add(1, Ordering::Relaxed);
		}
		flags.processing.1.notify_one();
		Ok(())
	}

	fn on_closed(&mut self) -> Result<(), Self::Error> {
		self.0.stop.store(true, Ordering::Release);
		self.0.processing.1.notify_one();
		Ok(())
	}
}

fn poll_processing(flags: Flags, poll_stop: Arc<AtomicBool>, failed: Arc<AtomicBool>) {
	while !poll_stop.load(Ordering::Acquire)
		&& !flags.stop.load(Ordering::Acquire)
		&& !flags.cancel.load(Ordering::Acquire)
	{
		let mut processing = match flags.processing.0.try_lock() {
			Ok(processing) => processing,
			Err(std::sync::TryLockError::WouldBlock) => {
				// The capture callback can be opening/replacing a native pool.
				// Keep cancellation observable instead of waiting behind that call.
				std::thread::sleep(Duration::from_millis(1));
				continue;
			}
			Err(std::sync::TryLockError::Poisoned(_)) => {
				failed.store(true, Ordering::Release);
				break;
			}
		};
		let mut waiting_gpu = false;
		if let Some(processor) = processing.processor.as_mut() {
			let result = processor.take();
			let current = processor.stats();
			waiting_gpu = current.submitted > current.completed;
			let previous = processing.previous;
			flags.stats.resources_created.fetch_add(
				current
					.resources_created
					.saturating_sub(previous.resources_created),
				Ordering::Relaxed,
			);
			if flags.probe.is_none() {
				flags.stats.readback_bytes.fetch_add(
					current
						.readback_bytes
						.saturating_sub(previous.readback_bytes),
					Ordering::Relaxed,
				);
				flags.stats.readback_ns.fetch_add(
					current.readback_ns.saturating_sub(previous.readback_ns),
					Ordering::Relaxed,
				);
			}
			processing.previous = current;
			match result {
				Ok(Some(frame)) => {
					flags.stats.completed.fetch_add(1, Ordering::Relaxed);
					flags.stats.gpu_frames.fetch_add(
						u64::from(!frame.as_encoder_frame().is_null()),
						Ordering::Relaxed,
					);
					if flags.frames.try_send(CapturedFrame::gpu(frame)).is_err() {
						flags.pending.store(false, Ordering::Release);
					}
				}
				Ok(None) => {}
				Err(_) => {
					failed.store(true, Ordering::Release);
					flags.stop.store(true, Ordering::Release);
					break;
				}
			}
		}
		// A completed frame can remain queued while encoding consumes the
		// preceding picture. Poll quickly only for an actual GPU transfer;
		// the next admitted capture wakes this condition variable itself.
		let delay = if waiting_gpu {
			Duration::from_millis(1)
		} else {
			Duration::from_millis(100)
		};
		let Ok((_guard, _)) = flags.processing.1.wait_timeout(processing, delay) else {
			failed.store(true, Ordering::Release);
			break;
		};
	}
}

pub(crate) struct Capture {
	control: Option<CaptureControl<Handler, &'static str>>,
	audio: Option<audio::Audio>,
	settings: CaptureSettings,
	frames: SyncSender<CapturedFrame>,
	stop: Arc<AtomicBool>,
	cancel: Arc<AtomicBool>,
	pending: Arc<AtomicBool>,
	adapter: Option<model::VideoAdapter>,
	gpu: bool,
	probe: Option<ProbeMode>,
	stats: Arc<ProbeStats>,
	processing: Arc<(Mutex<Processing>, Condvar)>,
	poll_stop: Arc<AtomicBool>,
	poll_failed: Arc<AtomicBool>,
	poll: Option<JoinHandle<()>>,
}

impl Capture {
	pub(crate) fn request_frame(
		&mut self,
		frames: &std::sync::mpsc::Receiver<CapturedFrame>,
	) -> Result<(), &'static str> {
		if self.cancel.load(Ordering::Acquire) {
			return Ok(());
		}
		self.poll_stop.store(true, Ordering::Release);
		self.processing.1.notify_all();
		if let Some(poll) = self.poll.take() {
			let _ = poll.join();
		}
		if let Some(control) = self.control.take() {
			control
				.stop()
				.map_err(|_| "Screen capture could not be refreshed")?;
		}
		while frames.try_recv().is_ok() {}
		self.pending.store(false, Ordering::Release);
		if self.cancel.load(Ordering::Acquire) {
			return Ok(());
		}
		let mut capture = Self::start_video(
			self.settings,
			self.frames.clone(),
			self.stop.clone(),
			self.cancel.clone(),
			self.pending.clone(),
			self.adapter,
			self.gpu,
			self.probe,
			self.stats.clone(),
		)?;
		capture.audio = self.audio.take();
		*self = capture;
		Ok(())
	}

	#[allow(clippy::too_many_arguments)] // One capture's explicit media/control outputs.
	pub(crate) fn start(
		settings: CaptureSettings,
		frames: SyncSender<CapturedFrame>,
		audio: Option<tokio::sync::mpsc::Sender<AudioChunk>>,
		stop: Arc<AtomicBool>,
		cancel: Arc<AtomicBool>,
		ready: Arc<AtomicBool>,
		audio_epoch: Arc<AtomicU64>,
		pending: Arc<AtomicBool>,
		adapter: Option<model::VideoAdapter>,
		gpu: bool,
	) -> Result<Self, &'static str> {
		if !settings.valid() {
			return Err("Invalid screen capture settings");
		}
		let mut capture = Self::start_video(
			settings,
			frames,
			stop.clone(),
			cancel.clone(),
			pending,
			adapter,
			gpu,
			None,
			Arc::new(ProbeStats::default()),
		)?;
		if let Some(send) = audio {
			capture.audio = Some(audio::Audio::start(send, stop, cancel, ready, audio_epoch)?);
		}
		Ok(capture)
	}

	#[allow(clippy::too_many_arguments)] // Shared state for normal, refresh and probe entry points.
	fn start_video(
		settings: CaptureSettings,
		frames: SyncSender<CapturedFrame>,
		stop: Arc<AtomicBool>,
		cancel: Arc<AtomicBool>,
		pending: Arc<AtomicBool>,
		adapter: Option<model::VideoAdapter>,
		gpu: bool,
		probe: Option<ProbeMode>,
		stats: Arc<ProbeStats>,
	) -> Result<Self, &'static str> {
		if cancel.load(Ordering::Acquire) {
			return Err(crate::video_encode::ENCODE_CANCELLED);
		}
		match settings.source {
			SourceId::Display(id) => {
				let monitor = Monitor::enumerate()
					.map_err(|_| "Displays could not be enumerated")?
					.into_iter()
					.find(|monitor| monitor_id(monitor) == id)
					.ok_or("Selected display is no longer available")?;
				start_item(
					settings,
					monitor,
					frames,
					stop.clone(),
					cancel.clone(),
					pending,
					adapter,
					gpu,
					probe,
					stats,
				)
			}
			SourceId::Window(id) => {
				let window = Window::enumerate()
					.map_err(|_| "Windows could not be enumerated")?
					.into_iter()
					.find(|window| window_id(window) == id)
					.ok_or("Selected window is no longer available")?;
				start_item(
					settings,
					window,
					frames,
					stop.clone(),
					cancel.clone(),
					pending,
					adapter,
					gpu,
					probe,
					stats,
				)
			}
			#[allow(unreachable_patterns)] // Portal may be absent from platform-scoped models.
			_ => Err("The desktop screen picker is available only on Linux"),
		}
	}

	pub(crate) fn start_probe(
		settings: CaptureSettings,
		frames: SyncSender<CapturedFrame>,
		stop: Arc<AtomicBool>,
		mode: ProbeMode,
		stats: Arc<ProbeStats>,
		adapter: Option<model::VideoAdapter>,
	) -> Result<Self, &'static str> {
		Self::start_video(
			settings,
			frames,
			stop.clone(),
			stop,
			Arc::new(AtomicBool::new(false)),
			adapter,
			false,
			Some(mode),
			stats,
		)
	}
	pub(crate) fn release_input(&self) {
		self.pending.store(false, Ordering::Release);
	}
	pub(crate) fn stats(&self) -> ProbeSnapshot {
		self.stats.snapshot()
	}
	pub(crate) fn failed(&self) -> bool {
		self.audio.as_ref().is_some_and(audio::Audio::failed)
			|| self.poll_failed.load(Ordering::Acquire)
			|| self
				.control
				.as_ref()
				.is_some_and(CaptureControl::is_finished)
	}
}

trait NativeSource:
	TryInto<windows_capture::settings::GraphicsCaptureItemType> + Clone + Send + 'static
{
	fn dimensions(&self) -> Option<(u32, u32)>;
}

impl NativeSource for Monitor {
	fn dimensions(&self) -> Option<(u32, u32)> {
		self.width().ok().zip(self.height().ok())
	}
}

impl NativeSource for Window {
	fn dimensions(&self) -> Option<(u32, u32)> {
		self.width()
			.ok()
			.and_then(|width| u32::try_from(width).ok())
			.zip(
				self.height()
					.ok()
					.and_then(|height| u32::try_from(height).ok()),
			)
	}
}

#[allow(clippy::too_many_arguments)] // A selected source plus the same bounded worker state.
fn start_item<T>(
	settings: CaptureSettings,
	item: T,
	frames: SyncSender<CapturedFrame>,
	stop: Arc<AtomicBool>,
	cancel: Arc<AtomicBool>,
	pending: Arc<AtomicBool>,
	adapter: Option<model::VideoAdapter>,
	gpu: bool,
	probe: Option<ProbeMode>,
	stats: Arc<ProbeStats>,
) -> Result<Capture, &'static str>
where
	T: NativeSource,
{
	let Some((width, height)) = item.dimensions() else {
		return Err("Selected source dimensions are unavailable");
	};
	if width == 0 || height == 0 || width > MAX_FRAME_WIDTH || height > MAX_FRAME_HEIGHT {
		return Err("Selected source exceeds the 8K capture limit");
	}
	let luminance = match settings.source {
		SourceId::Display(id) => platform::video::ffmpeg::capture_luminance(id, false),
		SourceId::Window(id) => platform::video::ffmpeg::capture_luminance(id, true),
		_ => None,
	};
	let flags = Flags {
		frames: frames.clone(),
		stop: stop.clone(),
		cancel: cancel.clone(),
		pending: pending.clone(),
		next_frame: Instant::now(),
		interval: Duration::from_secs_f64(1.0 / f64::from(settings.fps)),
		luminance,
		source: settings.source,
		next_color: Instant::now() + Duration::from_secs(1),
		settings,
		adapter,
		gpu,
		probe,
		stats: stats.clone(),
		processing: Arc::new((Mutex::new(Processing::default()), Condvar::new())),
	};
	let processing = flags.processing.clone();
	let poll_stop = Arc::new(AtomicBool::new(false));
	let poll_failed = Arc::new(AtomicBool::new(false));
	let worker_flags = flags.clone();
	let interval = if GraphicsCaptureApi::is_minimum_update_interval_supported().unwrap_or(false) {
		MinimumUpdateIntervalSettings::Custom(flags.interval)
	} else {
		MinimumUpdateIntervalSettings::Default
	};
	let native = Settings::new(
		item.clone(),
		if settings.cursor {
			CursorCaptureSettings::Default
		} else {
			CursorCaptureSettings::WithoutCursor
		},
		// Hide the border where supported; older Windows must keep the system default.
		if GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false) {
			DrawBorderSettings::WithoutBorder
		} else {
			DrawBorderSettings::Default
		},
		SecondaryWindowSettings::Default,
		interval,
		DirtyRegionSettings::Default,
		if luminance.is_some() {
			ColorFormat::Rgba16F
		} else {
			ColorFormat::Bgra8
		},
		flags.clone(),
	);
	let control = Handler::start_free_threaded(native)
		.or_else(|_| {
			// A reported optional property can still be rejected by the driver.
			// Retry the same HDR/SDR format with OS defaults before SDR fallback.
			Handler::start_free_threaded(Settings::new(
				item.clone(),
				if settings.cursor {
					CursorCaptureSettings::Default
				} else {
					CursorCaptureSettings::WithoutCursor
				},
				DrawBorderSettings::Default,
				SecondaryWindowSettings::Default,
				MinimumUpdateIntervalSettings::Default,
				DirtyRegionSettings::Default,
				if luminance.is_some() {
					ColorFormat::Rgba16F
				} else {
					ColorFormat::Bgra8
				},
				flags.clone(),
			))
		})
		.or_else(|_| {
			// Older capture devices may not accept a half-float surface. Retain
			// their existing SDR path, using the same approved source and handlers.
			Handler::start_free_threaded(Settings::new(
				item,
				if settings.cursor {
					CursorCaptureSettings::Default
				} else {
					CursorCaptureSettings::WithoutCursor
				},
				DrawBorderSettings::Default,
				SecondaryWindowSettings::Default,
				MinimumUpdateIntervalSettings::Default,
				DirtyRegionSettings::Default,
				ColorFormat::Bgra8,
				flags,
			))
		})
		.map_err(|_| "Screen capture could not be started")?;
	let worker_stop = poll_stop.clone();
	let worker_failed = poll_failed.clone();
	let poll = match std::thread::Builder::new()
		.name("serein-capture-readback".into())
		.spawn(move || poll_processing(worker_flags, worker_stop, worker_failed))
	{
		Ok(poll) => poll,
		Err(_) => {
			let _ = control.stop();
			return Err("Capture readback worker could not start");
		}
	};
	Ok(Capture {
		control: Some(control),
		audio: None,
		settings,
		frames,
		stop,
		cancel,
		pending,
		adapter,
		gpu,
		probe,
		stats,
		processing,
		poll_stop,
		poll_failed,
		poll: Some(poll),
	})
}

impl Drop for Capture {
	fn drop(&mut self) {
		self.poll_stop.store(true, Ordering::Release);
		self.processing.1.notify_all();
		if let Some(poll) = self.poll.take() {
			let _ = poll.join();
		}
		self.audio.take();
		if let Some(control) = self.control.take() {
			let _ = control.stop();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cancellation_exits_readback_without_waiting_for_a_busy_native_owner() {
		let (frames, _receive) = std::sync::mpsc::sync_channel(1);
		let stop = Arc::new(AtomicBool::new(false));
		let cancel = Arc::new(AtomicBool::new(true));
		let processing = Arc::new((Mutex::new(Processing::default()), Condvar::new()));
		let guard = processing.0.lock().unwrap();
		let flags = Flags {
			frames,
			stop: stop.clone(),
			cancel,
			pending: Arc::new(AtomicBool::new(false)),
			next_frame: Instant::now(),
			interval: Duration::from_millis(16),
			luminance: None,
			source: SourceId::Display(1),
			next_color: Instant::now(),
			settings: crate::screen::Settings {
				source: SourceId::Display(1),
				width: 1280,
				height: 720,
				fps: 60,
				cursor: true,
				audio: false,
			}
			.into(),
			adapter: None,
			gpu: true,
			probe: None,
			stats: Arc::default(),
			processing: processing.clone(),
		};
		let failed = Arc::new(AtomicBool::new(false));
		let failed_worker = failed.clone();
		let (done, receive) = std::sync::mpsc::sync_channel(1);
		let worker = std::thread::spawn(move || {
			poll_processing(flags, Arc::new(AtomicBool::new(false)), failed_worker);
			done.send(()).unwrap();
		});
		receive.recv_timeout(Duration::from_secs(1)).unwrap();
		drop(guard);
		worker.join().unwrap();
		assert!(!failed.load(Ordering::Acquire));
		assert!(
			!stop.load(Ordering::Acquire),
			"user cancellation is not source closure"
		);
	}
}
