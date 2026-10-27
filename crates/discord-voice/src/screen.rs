//! Explicitly selected, memory-only screen capture and video encoding.
pub use client_core::screen::{RequestSettings, Settings, Source, SourceId, valid_dimensions};
use model::voice_settings::{VideoCodec, VideoSettings};
#[cfg(target_os = "linux")]
#[path = "screen/audio_linux.rs"]
mod audio_linux;
#[cfg(all(test, not(target_os = "windows")))]
#[path = "screen/audio_windows.rs"]
mod audio_windows;
#[cfg(not(target_os = "linux"))]
#[path = "screen/capture.rs"]
mod capture;
#[path = "screen/convert.rs"]
mod convert;
#[cfg(target_os = "linux")]
#[path = "screen/gstreamer.rs"]
mod gstreamer;
#[cfg(target_os = "linux")]
#[path = "screen/linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
#[path = "screen/portal_linux.rs"]
mod portal_linux;
#[cfg(any(test, target_os = "windows"))]
#[path = "screen/capture_probe.rs"]
pub mod probe;

use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
	mpsc,
};

use std::time::{Duration, Instant};

pub const MAX_RAW_BYTES: usize = 7680 * 4320 * 8;
pub const MAX_ENCODED_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub enum RawFormat {
	#[default]
	Bgra8,
	/// Limited-range BT.601 NV12, preserving the existing SDR encoder matrix.
	Nv12,
	/// Linear Rec.709 half floats. Windows scRGB uses 80 nits per unit;
	/// Apple EDR capture uses the canonical 203-nit reference white.
	LinearRgba16 {
		unit_nits: u32,
		white_nits: u32,
		peak_nits: u32,
	},
	P010 {
		transfer: u32,
		primaries: u32,
		matrix: u32,
		full_range: bool,
		peak_nits: u32,
	},
}
impl RawFormat {
	fn bytes(self) -> usize {
		match self {
			Self::Bgra8 => 4,
			Self::Nv12 => 1,
			Self::LinearRgba16 { .. } => 8,
			Self::P010 { .. } => 2,
		}
	}
}
pub struct RawFrame {
	pub format: RawFormat,
	pub width: u32,
	pub height: u32,
	pub stride: usize,
	pub data: Vec<u8>,
}

/// Physical capture geometry is independent of a GPU-scaled encoder picture.
#[cfg(not(target_os = "linux"))]
pub(super) enum CapturedFrame {
	#[cfg(not(target_os = "windows"))]
	Cpu(RawFrame),
	#[cfg(target_os = "windows")]
	Gpu(platform::video::capture::Frame),
}

#[cfg(not(target_os = "linux"))]
impl CapturedFrame {
	#[cfg(not(target_os = "windows"))]
	fn cpu(frame: RawFrame) -> Self {
		Self::Cpu(frame)
	}
	#[cfg(target_os = "windows")]
	fn gpu(frame: platform::video::capture::Frame) -> Self {
		Self::Gpu(frame)
	}
	fn source_dimensions(&self) -> (u32, u32) {
		match self {
			#[cfg(not(target_os = "windows"))]
			Self::Cpu(frame) => (frame.width, frame.height),
			#[cfg(target_os = "windows")]
			Self::Gpu(frame) => frame.source_dimensions(),
		}
	}
	fn validate(&self) -> Result<(), &'static str> {
		let (width, height) = self.source_dimensions();
		if !valid_dimensions(width, height) {
			return Err("Screen capture returned unsupported source dimensions");
		}
		match self {
			#[cfg(not(target_os = "windows"))]
			Self::Cpu(frame) => validate_frame(frame).map(|_| ()),
			#[cfg(target_os = "windows")]
			Self::Gpu(frame) => {
				let (width, height) = frame.dimensions();
				if valid_dimensions(width, height) {
					Ok(())
				} else {
					Err("Screen capture returned unsupported processed dimensions")
				}
			}
		}
	}
	fn preview(&self) -> Result<Option<image::RgbaImage>, &'static str> {
		match self {
			#[cfg(not(target_os = "windows"))]
			Self::Cpu(frame) => preview_frame(frame).map(Some),
			#[cfg(target_os = "windows")]
			Self::Gpu(frame) => frame.preview(640, 360),
		}
	}
	#[cfg(target_os = "windows")]
	fn with_cpu<T>(
		&self,
		bytes: &mut Vec<u8>,
		use_frame: impl FnOnce(&RawFrame) -> Result<T, &'static str>,
	) -> Result<T, &'static str> {
		match self {
			#[cfg(target_os = "windows")]
			Self::Gpu(frame) => {
				let description = frame.download_into(bytes)?;
				use platform::video::capture::PixelFormat;
				let format = match description.format {
					PixelFormat::Bgra8 => RawFormat::Bgra8,
					PixelFormat::Nv12 => RawFormat::Nv12,
					PixelFormat::LinearRgba16 {
						unit_nits,
						white_nits,
						peak_nits,
					} => RawFormat::LinearRgba16 {
						unit_nits,
						white_nits,
						peak_nits,
					},
				};
				let raw = RawFrame {
					format,
					width: description.width,
					height: description.height,
					stride: description.stride,
					data: std::mem::take(bytes),
				};
				let result = use_frame(&raw);
				*bytes = raw.data;
				result
			}
		}
	}
}

pub struct EncodedFrame {
	pub codec: VideoCodec,
	pub data: Vec<u8>,
	pub timestamp: u32,
	pub keyframe: bool,
	/// Prediction generation; a restart invalidates every older queued access unit.
	pub epoch: u64,
	/// Capture reset acknowledged before this encoder session accepted its input.
	pub reset_generation: u64,
	/// Output-size generation, independent of encryption and prediction resets.
	pub geometry_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
	pub settings: Settings,
	pub rates: model::voice_settings::VideoBitrate,
	pub source_dimensions: (u32, u32),
	pub generation: u64,
}

/// Publish only validated capture geometry. A resize pauses sending until its
/// metadata is announced; old queued packets carry the previous generation.
pub(super) fn publish_output(
	slot: &Mutex<Option<Output>>,
	settings: Settings,
	source_dimensions: (u32, u32),
	ready: &AtomicBool,
	keyframe: &AtomicBool,
	bitrate: &AtomicU32,
	bandwidth: model::voice_settings::VideoBandwidth,
) -> Result<Output, &'static str> {
	if !settings.valid()
		|| !valid_dimensions(source_dimensions.0, source_dimensions.1)
		|| settings.width > source_dimensions.0
		|| settings.height > source_dimensions.1
	{
		return Err("Invalid resolved screen dimensions");
	}
	let mut output = slot.lock().map_err(|_| "Screen dimensions unavailable")?;
	if let Some(current) = *output
		&& current.settings == settings
		&& current.source_dimensions == source_dimensions
		&& current.rates == bandwidth.rates(settings.width, settings.height, settings.fps)
	{
		return Ok(current);
	}
	let generation = match *output {
		Some(current) => current
			.generation
			.checked_add(1)
			.ok_or("Screen size generation exhausted")?,
		None => 0,
	};
	ready.store(false, Ordering::Release);
	keyframe.store(true, Ordering::Release);
	let rates = bandwidth.rates(settings.width, settings.height, settings.fps);
	bitrate.store(rates.maximum, Ordering::Release);
	let current = Output {
		settings,
		rates,
		source_dimensions,
		generation,
	};
	*output = Some(current);
	Ok(current)
}

/// Longest system-audio chunk accepted from the OS: 100 ms of 48 kHz stereo.
pub const MAX_AUDIO_SAMPLES: usize = 4800 * 2;

#[derive(Debug)]
pub struct AudioChunk {
	pub samples: Vec<f32>,
	/// Capture generation; old buffers must not cross an encryption transition.
	pub epoch: u64,
}

/// Native work is observed independently of the thread performing it. A driver
/// call which stops returning cannot also be responsible for its own watchdog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOperation {
	ChoosingSource,
	OpeningCapture,
	RefreshingCapture,
	OpeningEncoder,
	Encoding,
	Draining,
	Retiring,
}

#[derive(Clone, Copy, Default)]
pub struct ProgressSnapshot {
	pub generation: u64,
	pub accepted: u64,
	pub produced: u64,
	pub sent: u64,
	pub captured: u64,
	pub last_capture: Option<Instant>,
	pub last_output: Option<Instant>,
	pub pending_since: Option<Instant>,
	pub operation: Option<(NativeOperation, Instant)>,
}

#[derive(Default)]
struct ProgressState {
	snapshot: ProgressSnapshot,
	encoder_epoch: Option<u64>,
	failure: Option<&'static str>,
}

/// One bounded, memory-only record per share; old sessions never update a new
/// session's supervisor. No titles, pixels or credentials enter this record.
#[derive(Default)]
pub struct Progress(Mutex<ProgressState>);

impl Progress {
	pub fn snapshot(&self) -> ProgressSnapshot {
		self.0
			.lock()
			.map_or_else(|_| ProgressSnapshot::default(), |state| state.snapshot)
	}

	pub fn native_failure(&self) -> Option<&'static str> {
		self.native_failure_at(Instant::now())
	}

	fn native_failure_at(&self, now: Instant) -> Option<&'static str> {
		if let Some(failure) = self.0.lock().ok().and_then(|state| state.failure) {
			return Some(failure);
		}
		let (operation, started) = self.snapshot().operation?;
		let limit = match operation {
			NativeOperation::ChoosingSource => Duration::from_secs(120),
			NativeOperation::OpeningCapture => Duration::from_secs(30),
			NativeOperation::Retiring => Duration::from_secs(5),
			_ => Duration::from_secs(15),
		};
		(now.saturating_duration_since(started) >= limit).then_some(match operation {
			NativeOperation::ChoosingSource => "Screen selection timed out; choose a source again",
			NativeOperation::OpeningCapture => "Screen capture did not finish starting",
			NativeOperation::RefreshingCapture => "Screen capture did not finish refreshing",
			NativeOperation::OpeningEncoder => "Screen video encoder did not finish starting",
			NativeOperation::Encoding | NativeOperation::Draining => {
				"Screen video encoder stopped responding"
			}
			NativeOperation::Retiring => "Screen capture is still releasing native resources",
		})
	}

	pub(super) fn operation(&self, operation: NativeOperation) -> Operation<'_> {
		let started = Instant::now();
		if let Ok(mut state) = self.0.lock() {
			state.snapshot.operation = Some((operation, started));
		}
		Operation {
			progress: self,
			operation,
			started,
		}
	}

	pub(crate) fn reset(&self, generation: u64) {
		if let Ok(mut state) = self.0.lock() {
			let operation = state.snapshot.operation;
			*state = ProgressState::default();
			state.snapshot.generation = generation;
			state.snapshot.operation = operation;
		}
	}

	pub(super) fn captured(&self) {
		if let Ok(mut state) = self.0.lock() {
			state.snapshot.captured = state.snapshot.captured.saturating_add(1);
			state.snapshot.last_capture = Some(Instant::now());
		}
	}

	pub(crate) fn packet(&self, generation: u64, packet: &crate::video_encode::EncodedPacket) {
		if let Ok(mut state) = self.0.lock() {
			if state.snapshot.generation != generation {
				return;
			}
			if state.encoder_epoch != Some(packet.epoch) {
				state.encoder_epoch = Some(packet.epoch);
				state.snapshot.accepted = 0;
				state.snapshot.produced = 0;
				state.snapshot.pending_since = None;
			}
			let snapshot = &mut state.snapshot;
			snapshot.accepted = snapshot
				.accepted
				.saturating_add(u64::from(packet.input_accepted));
			if !packet.data.is_empty() {
				snapshot.produced = snapshot.produced.saturating_add(1);
				snapshot.last_output = Some(Instant::now());
			}
		}
	}

	pub(crate) fn pending(&self, generation: u64, needed: bool) {
		if let Ok(mut state) = self.0.lock()
			&& state.snapshot.generation == generation
		{
			if needed {
				state
					.snapshot
					.pending_since
					.get_or_insert_with(Instant::now);
			} else {
				state.snapshot.pending_since = None;
			}
		}
	}

	pub(crate) fn transmitted(&self, generation: u64) {
		if let Ok(mut state) = self.0.lock()
			&& state.snapshot.generation == generation
		{
			state.snapshot.sent = state.snapshot.sent.saturating_add(1);
		}
	}

	pub(super) fn result<T>(&self, result: Result<T, &'static str>) -> Result<T, &'static str> {
		if let Err(error) = result.as_ref()
			&& *error != crate::video_encode::ENCODE_CANCELLED
			&& let Ok(mut state) = self.0.lock()
		{
			state.failure.get_or_insert(error);
		}
		result
	}

	pub(super) fn retire<T>(&self, owner: &mut Option<T>) {
		if owner.is_some() {
			let _retiring = self.operation(NativeOperation::Retiring);
			*owner = None;
		}
	}
}

pub(super) struct Operation<'a> {
	progress: &'a Progress,
	operation: NativeOperation,
	started: Instant,
}

impl Drop for Operation<'_> {
	fn drop(&mut self) {
		if let Ok(mut state) = self.progress.0.lock()
			&& state.snapshot.operation == Some((self.operation, self.started))
		{
			state.snapshot.operation = None;
		}
	}
}

pub(super) struct RetireOnExit<'a>(pub(super) &'a Progress);

impl Drop for RetireOnExit<'_> {
	fn drop(&mut self) {
		if let Ok(mut state) = self.0.0.lock() {
			state.snapshot.operation = Some((NativeOperation::Retiring, Instant::now()));
		}
	}
}

pub struct Video {
	pub progress: Arc<Progress>,
	pub video_settings: VideoSettings,
	pub output: Arc<Mutex<Option<Output>>>,
	pub frames: tokio::sync::mpsc::Receiver<EncodedFrame>,
	pub ready: Arc<AtomicBool>,
	pub keyframe: Arc<AtomicBool>,
	/// Transport feedback target in bits per second, bounded by the selected quality.
	pub bitrate: Arc<AtomicU32>,
	/// Interleaved 48 kHz stereo system audio, present only when the share requested it.
	pub audio: Option<tokio::sync::mpsc::Receiver<AudioChunk>>,
	pub audio_epoch: Arc<AtomicU64>,
}

impl Video {
	pub(crate) fn output(&self) -> Option<Output> {
		self.output
			.lock()
			.ok()
			.and_then(|output| *output)
			.filter(|output| {
				output.settings.valid()
					&& valid_dimensions(output.source_dimensions.0, output.source_dimensions.1)
					&& output.settings.width <= output.source_dimensions.0
					&& output.settings.height <= output.source_dimensions.1
			})
	}
	pub(crate) fn enable_output(&self, announced: Output) -> bool {
		let Ok(output) = self.output.lock() else {
			return false;
		};
		if *output != Some(announced) {
			return false;
		}
		self.keyframe.store(true, Ordering::Release);
		self.ready.store(true, Ordering::Release);
		true
	}
}

/// Whether this platform can capture system audio with the screen.
pub fn audio_supported() -> bool {
	supported()
}

pub fn supported() -> bool {
	cfg!(any(
		target_os = "macos",
		target_os = "windows",
		target_os = "linux"
	))
}

pub fn sources() -> Result<Vec<Source>, &'static str> {
	#[cfg(target_os = "linux")]
	{
		let mut sources = vec![Source {
			id: SourceId::Portal,
			name: "Choose in the system picker".into(),
			dimensions: None,
		}];
		if linux::x11_session() {
			sources.push(Source {
				id: SourceId::X11Desktop,
				name: "Entire X11 desktop · all monitors · no portal".into(),
				dimensions: None,
			});
		}
		Ok(sources)
	}
	#[cfg(not(target_os = "linux"))]
	capture::sources()
}

pub struct Worker {
	stop: Arc<AtomicBool>,
	progress: Arc<Progress>,
	ready: Arc<AtomicBool>,
	output: Arc<Mutex<Option<Output>>>,
	bitrate: Arc<AtomicU32>,
	preview: Arc<Mutex<Option<image::RgbaImage>>>,
	done: Option<mpsc::Receiver<Result<(), &'static str>>>,
	#[cfg(target_os = "linux")]
	status: Arc<Mutex<&'static str>>,
	preview_visible: Arc<AtomicBool>,
}

impl Worker {
	pub fn start(
		settings: impl Into<RequestSettings>,
		video_settings: VideoSettings,
		wake: impl Fn() + Send + 'static,
	) -> Result<(Self, Video), &'static str> {
		Self::start_on_adapter(settings, video_settings, None, wake)
	}

	pub fn start_on_adapter(
		settings: impl Into<RequestSettings>,
		video_settings: VideoSettings,
		adapter: Option<model::VideoAdapter>,
		wake: impl Fn() + Send + 'static,
	) -> Result<(Self, Video), &'static str> {
		let settings = settings.into();
		if !settings.valid() || !supported() {
			return Err("Screen sharing is unavailable for these settings or this platform");
		}
		let stop = Arc::new(AtomicBool::new(false));
		let progress = Arc::new(Progress::default());
		let worker_progress = progress.clone();
		let ready = Arc::new(AtomicBool::new(false));
		let audio_epoch = Arc::new(AtomicU64::new(0));
		let worker_audio_epoch = audio_epoch.clone();
		let keyframe = Arc::new(AtomicBool::new(true));
		let bitrate = Arc::new(AtomicU32::new(model::voice_settings::VideoBitrate::MINIMUM));
		let output = Arc::new(Mutex::new(None));
		let worker_output = output.clone();
		let worker_bitrate = bitrate.clone();
		let preview = Arc::new(Mutex::new(None));
		let worker_preview = preview.clone();
		#[cfg(target_os = "linux")]
		let status = Arc::new(Mutex::new("Choose a screen or window in the system picker"));
		#[cfg(target_os = "linux")]
		let worker_status = status.clone();
		let preview_visible = Arc::new(AtomicBool::new(false));
		let worker_preview_visible = preview_visible.clone();
		// A few frames of slack absorbs send jitter without forcing keyframes on every hiccup.
		let (send, frames) = tokio::sync::mpsc::channel(3);
		let (complete, done) = mpsc::sync_channel(1);
		let (audio_send, audio) = if settings.audio && audio_supported() {
			let (send, receive) = tokio::sync::mpsc::channel(4);
			(Some(send), Some(receive))
		} else {
			(None, None)
		};
		let (worker_stop, worker_ready, worker_keyframe) =
			(stop.clone(), ready.clone(), keyframe.clone());
		std::thread::Builder::new()
			.name("screen-encoder".into())
			.spawn(move || {
				// The MFT event poll and capture pacing sleep 1 ms at a time.
				let _resolution = crate::timer::Resolution::acquire();
				let finished_ready = worker_ready.clone();
				#[cfg(target_os = "linux")]
				let result = linux::run(
					settings,
					worker_output,
					video_settings,
					adapter,
					worker_stop,
					worker_ready,
					worker_keyframe,
					worker_bitrate,
					send,
					audio_send,
					worker_audio_epoch,
					worker_preview,
					worker_status,
					worker_preview_visible,
					worker_progress.clone(),
					&wake,
				);
				#[cfg(not(target_os = "linux"))]
				let result = encode_loop(
					settings,
					worker_output,
					video_settings,
					adapter,
					worker_stop,
					worker_ready,
					worker_keyframe,
					worker_bitrate,
					send,
					audio_send,
					worker_audio_epoch,
					worker_preview,
					worker_preview_visible,
					worker_progress.clone(),
					&wake,
				);
				finished_ready.store(false, Ordering::Release);
				if let Ok(mut state) = worker_progress.0.lock() {
					state.snapshot.operation = None;
				}
				// The desktop only shows the latest status, which a later stop overwrites.
				// Name the cause once so a share that ends by itself is never a mystery.
				if std::env::var_os("SEREIN_VOICE_DIAGNOSTICS").is_some_and(|value| value == "1") {
					match &result {
						Ok(()) => eprintln!("[Serein voice Screen] capture_stopped=ok"),
						Err(reason) => eprintln!("[Serein voice Screen] capture_stopped={reason}"),
					}
				}
				let _ = complete.try_send(result);
				wake();
			})
			.map_err(|_| "Could not start screen capture worker")?;
		Ok((
			Self {
				stop,
				progress: progress.clone(),
				ready: ready.clone(),
				output: output.clone(),
				bitrate: bitrate.clone(),
				preview,
				done: Some(done),
				#[cfg(target_os = "linux")]
				status,
				preview_visible,
			},
			Video {
				progress,
				video_settings,
				output,
				frames,
				ready,
				keyframe,
				bitrate,
				audio,
				audio_epoch,
			},
		))
	}

	pub fn set_preview_visible(&self, visible: bool) {
		self.preview_visible.store(visible, Ordering::Release);
		if !visible && let Ok(mut preview) = self.preview.try_lock() {
			*preview = None;
		}
	}

	pub fn capture_status(&self) -> Option<&'static str> {
		#[cfg(target_os = "linux")]
		return self.status.try_lock().ok().map(|status| *status);
		#[cfg(not(target_os = "linux"))]
		None
	}
	/// A simple presentation signal from the existing adaptive sender target.
	pub fn adapting(&self) -> bool {
		let Some(output) = self.output.try_lock().ok().and_then(|output| *output) else {
			return false;
		};
		!self.stop.load(Ordering::Acquire)
			&& self.ready.load(Ordering::Acquire)
			&& output.settings.valid()
			&& self.bitrate.load(Ordering::Acquire) < output.rates.maximum
	}

	pub fn result(&self) -> Option<Result<(), &'static str>> {
		self.done
			.as_ref()?
			.try_recv()
			.ok()
			.or_else(|| self.progress.native_failure().map(Err))
	}

	pub fn progress(&self) -> Arc<Progress> {
		self.progress.clone()
	}

	/// One local RGBA image, bounded to 640 by 360 pixels and replaced at most ten times a second.
	pub fn take_preview(&self) -> Option<image::RgbaImage> {
		let mut preview = self.preview.try_lock().ok()?;
		if !self.preview_visible.load(Ordering::Acquire) {
			*preview = None;
			return None;
		}
		preview.take()
	}

	pub fn shutdown(mut self) -> mpsc::Receiver<Result<(), &'static str>> {
		self.done.take().expect("screen worker completion")
	}
}

impl Drop for Worker {
	fn drop(&mut self) {
		self.ready.store(false, Ordering::Release);
		self.stop.store(true, Ordering::Release);
	}
}

/// Delayed encoders may need a few copies to release the last fresh picture.
/// Pending output alone must not keep a static screen encoding forever.
#[derive(Default)]
pub(super) struct CaptureDelivery {
	latest: u64,
	delivered: u64,
	epoch: Option<u64>,
	inputs: std::collections::VecDeque<(u32, u64)>,
}

impl CaptureDelivery {
	pub(super) fn fresh(&mut self) -> Result<(), &'static str> {
		self.latest = self
			.latest
			.checked_add(1)
			.ok_or("Screen capture sequence exhausted")?;
		Ok(())
	}
	pub(super) fn needs_picture(&self) -> bool {
		self.latest != self.delivered
	}
	pub(super) fn retire(&mut self) {
		self.inputs.clear();
		self.epoch = None;
		self.delivered = 0;
	}
	fn submitted(
		&mut self,
		timestamp: u32,
		epoch: u64,
		accepted: bool,
	) -> Result<(), &'static str> {
		if self.epoch != Some(epoch) {
			self.retire();
			self.epoch = Some(epoch);
		}
		if !accepted {
			return Ok(());
		}
		if self.inputs.len() >= crate::video_encode::MAX_PENDING_PICTURES {
			return Err("Screen encoder exceeded its pending picture limit");
		}
		self.inputs.push_back((timestamp, self.latest));
		Ok(())
	}
	fn received(&mut self, timestamp: u32, epoch: u64) {
		if self.epoch == Some(epoch)
			&& let Some(index) = self
				.inputs
				.iter()
				.position(|(input, _)| *input == timestamp)
			&& let Some((_, source)) = self.inputs.remove(index)
		{
			self.delivered = self.delivered.max(source);
		}
	}
	pub(super) fn record_packet(
		&mut self,
		timestamp: u32,
		packet: &crate::video_encode::EncodedPacket,
	) -> Result<(), &'static str> {
		self.submitted(timestamp, packet.epoch, packet.input_accepted)?;
		if !packet.data.is_empty() {
			self.received(packet.timestamp, packet.epoch);
		}
		Ok(())
	}
}

pub(super) fn check_output_deadline(
	deadline: &mut Option<Instant>,
	needs_picture: bool,
	progressed: bool,
	now: Instant,
) -> Result<(), &'static str> {
	if !needs_picture || progressed {
		*deadline = None;
	} else if now >= *deadline.get_or_insert(now + Duration::from_secs(15)) {
		return Err("Screen video encoder did not produce a picture");
	}
	Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::too_many_arguments)] // Media outputs of one explicitly started capture.
fn encode_loop(
	settings: RequestSettings,
	output: Arc<Mutex<Option<Output>>>,
	video_settings: VideoSettings,
	adapter: Option<model::VideoAdapter>,
	stop: Arc<AtomicBool>,
	ready: Arc<AtomicBool>,
	keyframe: Arc<AtomicBool>,
	bitrate: Arc<AtomicU32>,
	send: tokio::sync::mpsc::Sender<EncodedFrame>,
	audio: Option<tokio::sync::mpsc::Sender<AudioChunk>>,
	audio_epoch: Arc<AtomicU64>,
	preview: Arc<Mutex<Option<image::RgbaImage>>>,
	preview_visible: Arc<AtomicBool>,
	progress: Arc<Progress>,
	wake: &impl Fn(),
) -> Result<(), &'static str> {
	if stop.load(Ordering::Acquire) || send.is_closed() {
		return Ok(());
	}
	let origin = Instant::now();
	let mut reset_generation = audio_epoch.load(Ordering::Acquire);
	let (raw_send, raw) = mpsc::sync_channel(1);
	let capture_stop = Arc::new(AtomicBool::new(false));
	#[cfg(target_os = "windows")]
	let raw_pending = Arc::new(AtomicBool::new(false));
	let opening_capture = progress.operation(NativeOperation::OpeningCapture);
	#[cfg(target_os = "windows")]
	let mut _native = capture::Capture::start(
		settings,
		raw_send,
		audio,
		capture_stop.clone(),
		stop.clone(),
		ready.clone(),
		audio_epoch.clone(),
		raw_pending.clone(),
		adapter,
		true,
	)?;
	// The worker's stop also reaches a pending macOS picker, so call teardown closes it now.
	#[cfg(not(target_os = "windows"))]
	let mut _native = capture::Capture::start(
		settings,
		raw_send,
		audio,
		capture_stop.clone(),
		ready.clone(),
		audio_epoch.clone(),
		stop.clone(),
		&progress,
	)?;
	drop(opening_capture);
	let mut encoding: Option<ScreenEncoder> = None;
	let mut first_frame_deadline = Some(Instant::now() + Duration::from_secs(15));
	let mut first_output_deadline = None;
	let mut next_frame = Instant::now();
	let mut next_preview = Instant::now();
	let mut preview_supported = true;
	let mut was_preview_visible = true;
	let mut latest_frame = None;
	let mut next_epoch = 0;
	let mut current_output: Option<Output> = None;
	let mut delivery = CaptureDelivery::default();
	let mut metrics = crate::diagnostics::Metrics::new(crate::diagnostics::Scope::ScreenVideo);
	#[cfg(target_os = "windows")]
	if metrics.start().is_some() {
		eprintln!(
			"[Serein voice ScreenCapture] adapter_helper={} backend={:?} codec={:?} fps={}",
			adapter.map_or_else(|| "unidentified".to_owned(), |adapter| adapter.helper_key()),
			"FFmpeg",
			video_settings.codec,
			settings.fps
		);
	}
	#[cfg(target_os = "windows")]
	let mut capture_reports = (0u8, Instant::now() + Duration::from_secs(5));
	// Declared after the owned media resources: mark retirement before their
	// destructors run, including when a native error exits through `?`.
	let _retirement = RetireOnExit(&progress);

	while !stop.load(Ordering::Acquire) && !send.is_closed() {
		#[cfg(target_os = "windows")]
		if _native.failed() {
			return Err("Screen sharing stopped; check the selected source or audio device");
		}
		if capture_stop.load(Ordering::Acquire) {
			return Err("The selected screen or window stopped sharing");
		}
		#[cfg(target_os = "macos")]
		{
			let _refresh = progress.operation(NativeOperation::RefreshingCapture);
			progress.result(_native.refresh_dimensions())?;
		}
		let interval = Duration::from_secs_f64(1.0 / f64::from(settings.fps));
		let wait = if delivery.needs_picture() || keyframe.load(Ordering::Acquire) {
			interval.min(Duration::from_millis(100))
		} else {
			Duration::from_millis(100)
		};
		let frame = match raw.recv_timeout(wait) {
			Ok(frame) => {
				#[cfg(target_os = "windows")]
				raw_pending.store(false, Ordering::Release);
				Some(frame)
			}
			Err(_) if stop.load(Ordering::Acquire) || send.is_closed() => break,
			Err(mpsc::RecvTimeoutError::Timeout)
				if first_frame_deadline.is_none_or(|deadline| Instant::now() < deadline) =>
			{
				None
			}
			Err(_) => {
				return Err(
					"No screen frames received; check screen recording permission and the selected source",
				);
			}
		};
		if stop.load(Ordering::Acquire) || send.is_closed() {
			break;
		}
		let reset = audio_epoch.load(Ordering::Acquire);
		if reset == u64::MAX {
			return Err("Screen security reset generation exhausted");
		}
		if reset != reset_generation {
			reset_generation = reset;
			progress.reset(reset);
			if let Some(encoder) = &encoding {
				next_epoch = encoder
					.epoch()
					.checked_add(1)
					.ok_or("Screen encoder epoch exhausted")?;
			}
			progress.retire(&mut encoding);
			latest_frame = None;
			delivery.retire();
			first_output_deadline = None;
			keyframe.store(true, Ordering::Release);
			// This input can span the transition. Restart the selected capture so an
			// idle desktop supplies a new snapshot instead of waiting for damage.
			let _refresh = progress.operation(NativeOperation::RefreshingCapture);
			_native.request_frame(&raw)?;
			first_frame_deadline = Some(Instant::now() + Duration::from_secs(15));
			next_frame = Instant::now();
			continue;
		}
		let now = Instant::now();
		#[cfg(target_os = "windows")]
		if capture_reports.0 < 8 && now >= capture_reports.1 {
			if metrics.start().is_some() {
				// Fixed-size counts only; never include source titles, handles or pixels.
				eprintln!("[Serein voice ScreenCapture] {}", _native.stats().json());
			}
			capture_reports.0 += 1;
			capture_reports.1 = now + Duration::from_secs(5);
		}
		if let Some(frame) = &frame {
			progress.captured();
			frame.validate()?;
			let source_dimensions = frame.source_dimensions();
			let resolved = settings
				.resolve(source_dimensions)
				.ok_or("Unsupported screen source dimensions")?;
			let current = publish_output(
				&output,
				resolved,
				source_dimensions,
				&ready,
				&keyframe,
				&bitrate,
				video_settings.bandwidth,
			)?;
			if current_output != Some(current) {
				if let Some(encoder) = &encoding {
					next_epoch = encoder
						.epoch()
						.checked_add(1)
						.ok_or("Screen encoder epoch exhausted")?;
				}
				progress.retire(&mut encoding);
				latest_frame = None;
				delivery.retire();
				first_output_deadline = None;
				current_output = Some(current);
				preview_supported = true;
				next_frame = now;
			}
			first_frame_deadline = None;
		}
		let fresh = frame.is_some();
		metrics.poll(false, u64::from(send.capacity() == 0), !fresh, 0);
		if let Some(frame) = frame {
			delivery.fresh()?;
			latest_frame = Some(frame);
		}
		let visible = preview_visible.load(Ordering::Acquire);
		if visible && !was_preview_visible {
			preview_supported = true;
		}
		was_preview_visible = visible;
		if visible && preview_supported {
			if now >= next_preview
				&& let Some(frame) = &latest_frame
			{
				match frame.preview() {
					Ok(Some(image)) => {
						if let Ok(mut slot) = preview.try_lock() {
							*slot = Some(image);
						}
						wake();
					}
					Ok(None) => {}
					// Optional local presentation must not end an otherwise valid
					// outgoing stream or retry a failed processor on every frame.
					Err(_) => preview_supported = false,
				}
				next_preview = now + Duration::from_millis(100);
			}
		} else {
			// Showing a static source again must not wait for new capture damage.
			next_preview = now;
		}
		let encode = ready.load(Ordering::Acquire)
			&& latest_frame.is_some()
			&& (delivery.needs_picture()
				|| keyframe.load(Ordering::Acquire)
				|| encoding.as_ref().is_some_and(ScreenEncoder::pending));
		// Local capture remains available while alone; only secure media is encoded or queued.
		if !ready.load(Ordering::Acquire) {
			if let Some(encoder) = &encoding {
				next_epoch = encoder
					.epoch()
					.checked_add(1)
					.ok_or("Screen encoder epoch exhausted")?;
			}
			progress.retire(&mut encoding);
			delivery.retire();
			first_output_deadline = None;
			keyframe.store(true, Ordering::Release);
			continue;
		}
		if !encode {
			continue;
		}
		// Pace on an accumulating schedule with a little tolerance: capture timing jitter must
		// not skip every other frame, and a stalled encoder resumes from now instead of bursting.
		if now + Duration::from_millis(2) < next_frame {
			continue;
		}
		next_frame = (next_frame + interval).max(now + interval / 2);
		// Drop before encoding while the transport is behind, so the encoder never references
		// a picture the receiver did not get.
		if send.capacity() == 0 {
			first_output_deadline = None;
			continue;
		}
		let current = current_output.ok_or("Screen dimensions are unresolved")?;
		let resolved = current.settings;
		let target = bitrate.load(Ordering::Acquire).clamp(
			model::voice_settings::VideoBitrate::MINIMUM,
			current_output
				.ok_or("Screen dimensions are unresolved")?
				.rates
				.maximum,
		);
		if encoding.is_none() {
			let _opening = progress.operation(NativeOperation::OpeningEncoder);
			encoding = Some(progress.result(ScreenEncoder::new_for_capture(
				latest_frame.as_ref().expect("resolved screen frame"),
				resolved,
				target,
				video_settings,
				adapter,
				next_epoch,
				reset_generation,
				stop.clone(),
			))?);
		}
		let reconfigure = progress.operation(NativeOperation::OpeningEncoder);
		if progress.result(
			encoding
				.as_mut()
				.expect("secure screen encoder")
				.set_bitrate(target),
		)? {
			keyframe.store(true, Ordering::Release);
		}
		drop(reconfigure);
		// Retain one source snapshot for keyframe requests on an unchanged desktop.
		let force_keyframe = keyframe.swap(false, Ordering::AcqRel);
		if audio_epoch.load(Ordering::Acquire) != reset_generation {
			continue;
		}
		let encoder = encoding.as_mut().expect("secure screen encoder");
		let drained = if !fresh && !force_keyframe {
			let _drain = progress.operation(NativeOperation::Draining);
			progress.result(encoder.drain())?
		} else {
			None
		};
		let (packet, submitted_at) = if let Some(packet) = drained {
			(packet, 0)
		} else if delivery.needs_picture() || force_keyframe {
			let timestamp = (origin.elapsed().as_micros() * 90 / 1000) as u32;
			let started = metrics.start();
			let _encode = progress.operation(NativeOperation::Encoding);
			let packet = progress.result(encoder.encode_capture_at(
				latest_frame.as_ref().expect("latest screen frame"),
				force_keyframe,
				timestamp,
			))?;
			metrics.finish(crate::diagnostics::Stage::Encode, started);
			(packet, timestamp)
		} else {
			continue;
		};
		delivery.record_packet(submitted_at, &packet)?;
		progress.packet(reset_generation, &packet);
		progress.pending(
			reset_generation,
			delivery.needs_picture() || encoder.awaiting_keyframe(),
		);
		// Dropped inputs consume no picture credits. Bound missing output
		// separately so a native encoder cannot leave the viewer waiting forever.
		check_output_deadline(
			&mut first_output_deadline,
			encoder.awaiting_keyframe() || delivery.needs_picture(),
			!packet.data.is_empty(),
			Instant::now(),
		)?;
		if packet.data.is_empty() {
			continue;
		}
		if !ready.load(Ordering::Acquire)
			|| stop.load(Ordering::Acquire)
			|| audio_epoch.load(Ordering::Acquire) != reset_generation
		{
			continue;
		}
		let frame = EncodedFrame {
			geometry_generation: current.generation,
			codec: video_settings.codec,
			data: packet.data,
			timestamp: packet.timestamp,
			keyframe: packet.keyframe,
			epoch: packet.epoch,
			reset_generation: encoding
				.as_ref()
				.expect("secure screen encoder")
				.reset_generation,
		};
		if send.try_send(frame).is_err() {
			let _restart = progress.operation(NativeOperation::OpeningEncoder);
			progress.result(encoding.as_mut().expect("secure screen encoder").restart())?;
			keyframe.store(true, Ordering::Release);
			delivery.retire();
		}
	}
	capture_stop.store(true, Ordering::Release);
	Ok(())
}

#[cfg(any(test, target_os = "linux"))]
pub(super) fn retain_screen_frame(
	latest: &mut Option<RawFrame>,
	frame: Option<RawFrame>,
	ready: bool,
	keyframe: bool,
) -> Result<bool, &'static str> {
	let fresh = frame.is_some();
	if let Some(frame) = frame {
		validate_frame(&frame)?;
		*latest = Some(frame);
	}
	Ok(ready && latest.is_some() && (fresh || keyframe))
}

/// Selected encoder shared by every platform; native capture remains independent.
pub(super) struct ScreenEncoder {
	stop: Arc<AtomicBool>,
	diagnostics: crate::diagnostics::EncoderRegistration,
	encoder: Option<crate::video_encode::Encoder>,
	video_settings: VideoSettings,
	i420: Vec<u8>,
	converter: convert::Converter,
	#[cfg(target_os = "windows")]
	capture_bytes: Vec<u8>,
	#[cfg(target_os = "windows")]
	surface_input: bool,
	settings: Settings,
	bitrate: u32,
	adapter: Option<model::VideoAdapter>,
	epoch_base: u64,
	awaiting_keyframe: bool,
	reset_generation: u64,
}

impl ScreenEncoder {
	pub(super) fn awaiting_keyframe(&self) -> bool {
		self.awaiting_keyframe
	}

	#[cfg(test)]
	pub(super) fn new_on_adapter(
		settings: Settings,
		bitrate: u32,
		video_settings: VideoSettings,
		adapter: Option<model::VideoAdapter>,
		epoch_base: u64,
		reset_generation: u64,
	) -> Result<Self, &'static str> {
		Self::new_controlled(
			settings,
			bitrate,
			video_settings,
			adapter,
			epoch_base,
			reset_generation,
			Arc::new(AtomicBool::new(false)),
		)
	}

	pub(super) fn new_controlled(
		settings: Settings,
		bitrate: u32,
		video_settings: VideoSettings,
		adapter: Option<model::VideoAdapter>,
		epoch_base: u64,
		reset_generation: u64,
		stop: Arc<AtomicBool>,
	) -> Result<Self, &'static str> {
		if !settings.valid() || reset_generation == u64::MAX {
			return Err("Invalid screen encoder settings");
		}
		let mut config = Self::config(settings, bitrate, video_settings);
		config.adapter = adapter;
		let encoder = crate::video_encode::Encoder::new_with_control(config, stop.clone())?;
		Ok(Self::with_encoder(
			settings,
			bitrate,
			video_settings,
			adapter,
			epoch_base,
			reset_generation,
			encoder,
			stop,
		))
	}

	#[allow(clippy::too_many_arguments)]
	fn with_encoder(
		settings: Settings,
		bitrate: u32,
		video_settings: VideoSettings,
		adapter: Option<model::VideoAdapter>,
		epoch_base: u64,
		reset_generation: u64,
		encoder: crate::video_encode::Encoder,
		stop: Arc<AtomicBool>,
	) -> Self {
		Self {
			stop,
			diagnostics: crate::diagnostics::EncoderRegistration::new(
				true,
				encoder.hardware(),
				encoder.amf_split(),
			),
			encoder: Some(encoder),
			video_settings,
			i420: Vec::new(),
			converter: convert::Converter::default(),
			#[cfg(target_os = "windows")]
			capture_bytes: Vec::new(),
			#[cfg(target_os = "windows")]
			surface_input: false,
			settings,
			bitrate,
			adapter,
			epoch_base,
			awaiting_keyframe: true,
			reset_generation,
		}
	}

	#[cfg(not(target_os = "linux"))]
	#[allow(clippy::too_many_arguments)]
	fn new_for_capture(
		frame: &CapturedFrame,
		settings: Settings,
		bitrate: u32,
		video_settings: VideoSettings,
		adapter: Option<model::VideoAdapter>,
		epoch_base: u64,
		reset_generation: u64,
		stop: Arc<AtomicBool>,
	) -> Result<Self, &'static str> {
		if !settings.valid() || reset_generation == u64::MAX {
			return Err("Invalid screen encoder settings");
		}
		frame.validate()?;
		#[cfg(target_os = "windows")]
		{
			let CapturedFrame::Gpu(frame) = frame;
			let mut config = Self::config(settings, bitrate, video_settings);
			config.adapter = adapter;
			let encoder = crate::video_encode::Encoder::new_on_surface_with_control(
				config,
				frame,
				stop.clone(),
			);
			if let Err(error) = encoder.as_ref()
				&& crate::video_encode::terminal_error(error)
			{
				return Err(error);
			}
			if let Ok(encoder) = encoder {
				let mut screen = Self::with_encoder(
					settings,
					bitrate,
					video_settings,
					adapter,
					epoch_base,
					reset_generation,
					encoder,
					stop,
				);
				screen.surface_input = true;
				return Ok(screen);
			}
			frame.prefer_cpu();
		}
		Self::new_controlled(
			settings,
			bitrate,
			video_settings,
			adapter,
			epoch_base,
			reset_generation,
			stop,
		)
	}

	#[cfg(not(target_os = "linux"))]
	fn encode_capture_at(
		&mut self,
		frame: &CapturedFrame,
		force: bool,
		timestamp: u32,
	) -> Result<crate::video_encode::EncodedPacket, &'static str> {
		#[cfg(target_os = "windows")]
		let captured = frame;
		match frame {
			#[cfg(not(target_os = "windows"))]
			CapturedFrame::Cpu(frame) => self.encode_at(frame, force, timestamp),
			#[cfg(target_os = "windows")]
			CapturedFrame::Gpu(frame) => {
				if self.surface_input
					&& !self
						.encoder
						.as_ref()
						.is_some_and(|encoder| encoder.uses_surface())
				{
					self.surface_input = false;
					frame.prefer_cpu();
				}
				if self.surface_input
					&& !self
						.encoder
						.as_ref()
						.is_some_and(|encoder| encoder.accepts_surface(frame))
				{
					// A restarted capture owns a new pool, even if its dimensions
					// are unchanged. Release the old device/session before opening it.
					let epoch = self
						.epoch()
						.checked_add(1)
						.ok_or("Screen encoder epoch exhausted")?;
					self.encoder = None;
					self.diagnostics.set(None);
					let mut config = Self::config(self.settings, self.bitrate, self.video_settings);
					config.adapter = self.adapter;
					let encoder = crate::video_encode::Encoder::new_on_surface_with_control(
						config,
						frame,
						self.stop.clone(),
					)
					.or_else(|error| {
						if crate::video_encode::terminal_error(error) {
							Err(error)
						} else {
							crate::video_encode::Encoder::new_with_control(
								config,
								self.stop.clone(),
							)
						}
					})?;
					self.surface_input = encoder.uses_surface();
					if !self.surface_input {
						frame.prefer_cpu();
					}
					self.encoder = Some(encoder);
					self.epoch_base = epoch;
					self.awaiting_keyframe = true;
				}
				if self.surface_input {
					if force {
						self.restart()?;
					}
					let encoder = self.encoder.as_mut().ok_or("Screen encoder stopped")?;
					match encoder.encode_surface_at(frame, false, timestamp) {
						Ok(result) => return self.finish_packet(result),
						Err(error) if crate::video_encode::terminal_error(error) => {
							return Err(error);
						}
						Err(_) => {}
					}
					// The host API retires the old surface session and advances its
					// prediction epoch before accepting this downloaded picture.
					self.surface_input = false;
					frame.prefer_cpu();
					self.awaiting_keyframe = true;
				}
				let mut bytes = std::mem::take(&mut self.capture_bytes);
				let result =
					captured.with_cpu(&mut bytes, |raw| self.encode_at(raw, force, timestamp));
				self.capture_bytes = bytes;
				result
			}
		}
	}

	fn config(
		settings: Settings,
		bitrate: u32,
		video: VideoSettings,
	) -> crate::video_encode::Config {
		crate::video_encode::Config {
			width: settings.width,
			height: settings.height,
			fps: settings.fps,
			rates: video
				.bandwidth
				.rates(settings.width, settings.height, settings.fps)
				.limited_to(bitrate),
			preset: video.quality,
			max_bytes: MAX_ENCODED_BYTES,
			profile: crate::video_encode::Profile::Main,
			codec: video.codec,
			adapter: None,
			hdr: None,
		}
	}

	#[cfg(target_os = "linux")]
	pub(super) fn label(&self) -> &'static str {
		self.encoder
			.as_ref()
			.map_or("FFmpeg encoder stopped", |encoder| encoder.label())
	}

	pub(super) fn set_bitrate(&mut self, bitrate: u32) -> Result<bool, &'static str> {
		let bitrate = bitrate.clamp(
			model::voice_settings::VideoBitrate::MINIMUM,
			self.video_settings
				.bandwidth
				.rates(self.settings.width, self.settings.height, self.settings.fps)
				.maximum,
		);
		// Rate changes restart at an IDR. Bound restart frequency with the sender's
		// existing 15% reduction / 25% recovery thresholds.
		if self.awaiting_keyframe || !software_rate_change(self.bitrate, bitrate) {
			return Ok(false);
		}
		self.diagnostics.set(None);
		let mut config = Self::config(self.settings, bitrate, self.video_settings);
		config.adapter = self.adapter;
		let encoder = self.encoder.as_mut().ok_or("Screen encoder stopped")?;
		encoder.reconfigure(config)?;
		self.diagnostics
			.set(Some((encoder.hardware(), encoder.amf_split())));
		self.bitrate = bitrate;
		self.awaiting_keyframe = true;
		Ok(true)
	}

	#[cfg(test)]
	pub(super) fn encode(
		&mut self,
		frame: &RawFrame,
		force: bool,
	) -> Result<(Vec<u8>, bool), &'static str> {
		let packet = self.encode_at(frame, force, 0)?;
		Ok((packet.data, packet.keyframe))
	}

	pub(super) fn pending(&self) -> bool {
		self.encoder
			.as_ref()
			.is_some_and(|encoder| encoder.pending())
	}

	pub(super) fn drain(
		&mut self,
	) -> Result<Option<crate::video_encode::EncodedPacket>, &'static str> {
		if self.stop.load(Ordering::Acquire) {
			return Err(crate::video_encode::ENCODE_CANCELLED);
		}
		let result = self
			.encoder
			.as_mut()
			.ok_or("Screen encoder stopped")?
			.drain_at()?;
		result.map(|packet| self.finish_packet(packet)).transpose()
	}

	pub(super) fn epoch(&self) -> u64 {
		self.epoch_base
			.saturating_add(self.encoder.as_ref().map_or(0, |encoder| encoder.epoch()))
	}

	pub(super) fn restart(&mut self) -> Result<(), &'static str> {
		if !self.awaiting_keyframe {
			self.encoder
				.as_mut()
				.ok_or("Screen encoder stopped")?
				.restart()?;
			self.awaiting_keyframe = true;
		}
		Ok(())
	}

	#[cfg(target_os = "linux")]
	pub(super) fn reset_generation(&self) -> u64 {
		self.reset_generation
	}

	#[cfg(any(test, target_os = "linux"))]
	pub(super) fn reset_for_security(&mut self, generation: u64) -> Result<bool, &'static str> {
		if generation == u64::MAX {
			return Err("Screen security reset generation exhausted");
		}
		if generation == self.reset_generation {
			return Ok(false);
		}
		// Unlike ordinary keyframe feedback, this must discard an already pending IDR.
		self.encoder
			.as_mut()
			.ok_or("Screen encoder stopped")?
			.restart()?;
		self.reset_generation = generation;
		self.awaiting_keyframe = true;
		Ok(true)
	}

	pub(super) fn encode_at(
		&mut self,
		frame: &RawFrame,
		force: bool,
		timestamp: u32,
	) -> Result<crate::video_encode::EncodedPacket, &'static str> {
		if self.stop.load(Ordering::Acquire) {
			return Err(crate::video_encode::ENCODE_CANCELLED);
		}
		if force {
			self.restart()?;
		}
		let (width, height) = (self.settings.width as usize, self.settings.height as usize);
		self.i420.resize(width * height * 3 / 2, 0);
		self.converter
			.convert(frame, width, height, &mut self.i420)?;
		let encoder = self
			.encoder
			.as_mut()
			.ok_or("FFmpeg screen encoder stopped")?;
		let result = encoder.encode_at(&self.i420, false, timestamp)?;
		self.finish_packet(result)
	}

	fn finish_packet(
		&mut self,
		mut result: crate::video_encode::EncodedPacket,
	) -> Result<crate::video_encode::EncodedPacket, &'static str> {
		let encoder = self.encoder.as_ref().ok_or("Screen encoder stopped")?;
		self.diagnostics
			.set(Some((encoder.hardware(), encoder.amf_split())));
		result.epoch = result
			.epoch
			.checked_add(self.epoch_base)
			.ok_or("Screen encoder epoch exhausted")?;
		if !result.data.is_empty() {
			if self.awaiting_keyframe && !result.keyframe {
				return Err("Screen encoder did not restart at an independently decodable picture");
			}
			self.awaiting_keyframe = false;
		}
		Ok(result)
	}
}

/// Significant bitrate moves apply without restarting on every feedback tick.
pub(crate) fn software_rate_change(current: u32, target: u32) -> bool {
	u64::from(target) * 100 <= u64::from(current) * 85
		|| u64::from(target) * 100 >= u64::from(current) * 125
}

fn validate_frame(frame: &RawFrame) -> Result<(usize, usize), &'static str> {
	let row_bytes = (frame.width as usize)
		.checked_mul(frame.format.bytes())
		.ok_or("Screen capture returned an unsupported frame size")?;
	let required = frame
		.stride
		.checked_mul(frame.height as usize)
		.ok_or("Screen capture returned an unsupported frame size")?;
	if !valid_dimensions(frame.width, frame.height)
		|| frame.data.len() > MAX_RAW_BYTES
		|| frame.stride < row_bytes
		|| required > frame.data.len()
		|| matches!(frame.format, RawFormat::P010 { .. })
			&& (!frame.width.is_multiple_of(2)
				|| !frame.height.is_multiple_of(2)
				|| frame.stride != row_bytes
				|| required + required / 2 != frame.data.len())
		|| matches!(frame.format, RawFormat::Nv12)
			&& (!frame.width.is_multiple_of(2)
				|| !frame.height.is_multiple_of(2)
				|| !frame.stride.is_multiple_of(2)
				|| required + required / 2 != frame.data.len())
	{
		return Err("Screen capture returned an unsupported frame size");
	}

	let valid_color = match frame.format {
		RawFormat::Bgra8 | RawFormat::Nv12 => true,
		RawFormat::LinearRgba16 {
			unit_nits,
			white_nits,
			peak_nits,
		} => {
			(1..=1000).contains(&unit_nits)
				&& (80..=1000).contains(&white_nits)
				&& (white_nits..=10000).contains(&peak_nits)
		}
		RawFormat::P010 {
			transfer,
			primaries,
			matrix,
			peak_nits,
			..
		} => {
			matches!(transfer, 16 | 18)
				&& matches!(primaries, 1 | 9 | 12)
				&& matches!(matrix, 1 | 5 | 6 | 9)
				&& (100..=10000).contains(&peak_nits)
		}
	};
	if !valid_color {
		return Err("Screen capture returned unsupported color metadata");
	}

	Ok((row_bytes, required))
}

#[cfg(any(test, not(target_os = "windows")))]
pub(super) fn preview_frame(frame: &RawFrame) -> Result<image::RgbaImage, &'static str> {
	validate_frame(frame)?;
	let scale = (640.0 / f64::from(frame.width))
		.min(360.0 / f64::from(frame.height))
		.min(1.0);
	let width = (f64::from(frame.width) * scale).round().max(1.0) as u32;
	let height = (f64::from(frame.height) * scale).round().max(1.0) as u32;
	// Nearest sampling keeps the ten-fps preview cheap; streaming uses filtered scaling.
	let conversion = frame.conversion();
	Ok(image::RgbaImage::from_fn(width, height, |x, y| {
		let source_x = (x * frame.width / width) as usize;
		let source_y = (y * frame.height / height) as usize;
		let rgb = frame.sdr(source_x, source_y, conversion.as_ref());
		image::Rgba([rgb[0], rgb[1], rgb[2], 255])
	}))
}

impl RawFrame {
	fn conversion(&self) -> Option<platform::video::ffmpeg::SdrConverter> {
		use platform::video::ffmpeg::{HdrTransfer, SdrConverter};
		match self.format {
			RawFormat::Bgra8 | RawFormat::Nv12 => None,
			RawFormat::LinearRgba16 {
				white_nits,
				peak_nits,
				..
			} => Some(SdrConverter::new(
				1,
				peak_nits as f32,
				white_nits as f32,
				None,
			)),
			RawFormat::P010 {
				primaries,
				peak_nits,
				transfer,
				..
			} => Some(SdrConverter::new(
				primaries,
				peak_nits as f32,
				203.0,
				HdrTransfer::from_native(transfer),
			)),
		}
	}
	#[cfg(any(test, not(target_os = "windows")))]
	fn sdr(
		&self,
		x: usize,
		y: usize,
		conversion: Option<&platform::video::ffmpeg::SdrConverter>,
	) -> [u8; 3] {
		let at = y * self.stride + x * self.format.bytes();
		if matches!(self.format, RawFormat::Bgra8) {
			return [self.data[at + 2], self.data[at + 1], self.data[at]];
		}
		if matches!(self.format, RawFormat::Nv12) {
			let luma = i32::from(self.data[at]).saturating_sub(16).max(0);
			let uv = self.stride * self.height as usize + y / 2 * self.stride + (x & !1);
			let u = i32::from(self.data[uv]) - 128;
			let v = i32::from(self.data[uv + 1]) - 128;
			return [
				298 * luma + 409 * v,
				298 * luma - 100 * u - 208 * v,
				298 * luma + 516 * u,
			]
			.map(|value| ((value + 128) >> 8).clamp(0, 255) as u8);
		}
		let conversion = conversion.expect("validated HDR conversion");
		conversion.to_sdr(self.nits(x, y, conversion))
	}
	fn nits(
		&self,
		x: usize,
		y: usize,
		conversion: &platform::video::ffmpeg::SdrConverter,
	) -> [f32; 3] {
		let at = y * self.stride + x * self.format.bytes();
		match self.format {
			RawFormat::LinearRgba16 { unit_nits, .. } => std::array::from_fn(|channel| {
				platform::video::ffmpeg::half([
					self.data[at + channel * 2],
					self.data[at + channel * 2 + 1],
				]) * unit_nits as f32
			}),
			RawFormat::P010 {
				transfer,
				matrix,
				full_range,
				..
			} => {
				let word =
					|at| (u16::from_le_bytes([self.data[at], self.data[at + 1]]) >> 6) as f32;
				let yy = word(at);
				let c = self.stride * self.height as usize + y / 2 * self.stride + x / 2 * 4;
				let uv = [word(c), word(c + 2)];
				let yy = if full_range {
					yy / 1023.0
				} else {
					(yy - 64.0) / 876.0
				};
				let uv = uv.map(|v| (v - 512.0) / if full_range { 1023.0 } else { 896.0 });
				let (kr, kb) = match matrix {
					9 => (0.2627, 0.0593),
					1 => (0.2126, 0.0722),
					_ => (0.299, 0.114),
				};
				let kg = 1.0 - kr - kb;
				let rgb = [
					yy + 2.0 * (1.0 - kr) * uv[1],
					yy - 2.0 * kb * (1.0 - kb) / kg * uv[0] - 2.0 * kr * (1.0 - kr) / kg * uv[1],
					yy + 2.0 * (1.0 - kb) * uv[0],
				];
				conversion.source_nits(
					rgb,
					platform::video::ffmpeg::HdrTransfer::from_native(transfer)
						.expect("validated capture transfer"),
				)
			}
			RawFormat::Bgra8 | RawFormat::Nv12 => [0.0; 3],
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn output_deadline_bounds_dropped_pictures_and_allows_idle_completed_shares() {
		let start = Instant::now();
		let mut deadline = None;
		let mut delivery = CaptureDelivery::default();
		delivery.fresh().unwrap();
		let dropped = crate::video_encode::EncodedPacket {
			data: Vec::new(),
			keyframe: false,
			timestamp: 0,
			epoch: 0,
			input_accepted: false,
		};
		for second in 0..15 {
			delivery.record_packet(second * 90_000, &dropped).unwrap();
			assert!(delivery.inputs.is_empty() && delivery.needs_picture());
			check_output_deadline(
				&mut deadline,
				true,
				false,
				start + Duration::from_secs(u64::from(second)),
			)
			.unwrap();
			assert_eq!(deadline, Some(start + Duration::from_secs(15)));
		}
		assert!(
			check_output_deadline(&mut deadline, true, false, start + Duration::from_secs(15))
				.is_err()
		);
		// Successful output or a security pause retires the previous deadline.
		check_output_deadline(&mut deadline, false, false, start + Duration::from_secs(15))
			.unwrap();
		let resumed = start + Duration::from_secs(3600);
		check_output_deadline(&mut deadline, false, false, resumed).unwrap();
		assert!(deadline.is_none());
		check_output_deadline(&mut deadline, true, false, resumed).unwrap();
		assert_eq!(deadline, Some(resumed + Duration::from_secs(15)));
		check_output_deadline(&mut deadline, true, true, resumed + Duration::from_secs(1)).unwrap();
		assert!(deadline.is_none());
		// Continuous capture can still need a newer picture when an older one is
		// returned. Actual progress must reset the deadline even in that case.
		for second in 1..=60 {
			let now = resumed + Duration::from_secs(second);
			check_output_deadline(&mut deadline, true, false, now).unwrap();
			check_output_deadline(&mut deadline, true, true, now + Duration::from_millis(1))
				.unwrap();
			assert!(deadline.is_none());
		}
		// Once startup succeeded, empty polls/dropped pictures must still time out.
		let stalled = resumed + Duration::from_secs(61);
		for second in 0..15 {
			check_output_deadline(
				&mut deadline,
				true,
				false,
				stalled + Duration::from_secs(second),
			)
			.unwrap();
			assert_eq!(deadline, Some(stalled + Duration::from_secs(15)));
		}
		assert!(
			check_output_deadline(
				&mut deadline,
				true,
				false,
				stalled + Duration::from_secs(15)
			)
			.is_err()
		);
		check_output_deadline(
			&mut deadline,
			false,
			false,
			stalled + Duration::from_secs(3600),
		)
		.unwrap();
		assert!(deadline.is_none());
	}

	#[test]
	fn buffered_static_captures_stop_uploading_after_delivery_and_keep_the_keepalive() {
		use openh264::formats::YUVSource;
		use std::collections::VecDeque;
		let settings = Settings {
			source: SourceId::Display(1),
			width: 320,
			height: 240,
			fps: 60,
			cursor: false,
			audio: false,
		};
		let mut encoder = crate::video_encode::Encoder::software(ScreenEncoder::config(
			settings,
			settings.bit_rate(),
			VideoSettings::default(),
		))
		.unwrap();
		let mut pictures = Vec::new();
		for value in [128, 64] {
			pictures.push(
				encoder
					.encode_at(&vec![value; 320 * 240 * 3 / 2], true, 0)
					.unwrap()
					.data,
			);
		}
		let mut latest = None;
		let mut delivery = CaptureDelivery::default();
		let mut pending = VecDeque::new();
		let mut uploads = 0;
		let mut last = Vec::new();
		// Two captures and two keepalives over 240 scheduling opportunities. Model
		// sixteen-picture lookahead with real encoded pictures and capture accounting.
		for tick in 0..240 {
			let raw = (tick == 0 || tick == 120).then(|| RawFrame {
				format: RawFormat::Bgra8,
				width: 320,
				height: 240,
				stride: 320 * 4,
				data: vec![if tick == 0 { 128 } else { 64 }; 320 * 240 * 4],
			});
			if raw.is_some() {
				delivery.fresh().unwrap();
			}
			let keepalive = tick == 60 || tick == 180;
			if !retain_screen_frame(
				&mut latest,
				raw,
				true,
				keepalive || delivery.needs_picture(),
			)
			.unwrap()
			{
				continue;
			}
			let picture = usize::from(latest.as_ref().unwrap().data[0] == 64);
			pending.push_back((tick * 1500, picture));
			uploads += 1;
			let mut packet = crate::video_encode::EncodedPacket {
				data: Vec::new(),
				keyframe: false,
				timestamp: 0,
				epoch: 0,
				input_accepted: true,
			};
			if pending.len() > 16 {
				let (timestamp, picture) = pending.pop_front().unwrap();
				packet.timestamp = timestamp;
				packet.data = pictures[picture].clone();
				packet.keyframe = true;
				last = packet.data.clone();
			}
			delivery.record_packet(tick * 1500, &packet).unwrap();
		}
		assert_eq!(
			uploads, 36,
			"buffered output must not keep uploading an unchanged capture"
		);
		assert!(!delivery.needs_picture());
		assert_eq!(pending.len(), 16);
		// Reordered, receive-only tail output still drains after the latest
		// capture was delivered, without turning polling into more uploads.
		while let Some((timestamp, picture)) = pending.pop_back() {
			assert!(
				!retain_screen_frame(&mut latest, None, true, delivery.needs_picture()).unwrap()
			);
			let packet = crate::video_encode::EncodedPacket {
				data: pictures[picture].clone(),
				keyframe: true,
				timestamp,
				epoch: 0,
				input_accepted: false,
			};
			delivery.record_packet(0, &packet).unwrap();
			last = packet.data;
		}
		assert!(delivery.inputs.is_empty() && !delivery.needs_picture());
		let mut decoder = openh264::decoder::Decoder::new().unwrap();
		let decoded = decoder.decode(&last).unwrap().unwrap();
		assert_eq!(decoded.dimensions(), (320, 240));
		assert!((60..=68).contains(&decoded.y()[0]));
	}

	#[test]
	fn delayed_capture_output_finishes_the_latest_picture_without_an_idle_encode_loop() {
		let mut delivery = CaptureDelivery::default();
		assert!(!delivery.needs_picture());
		delivery.fresh().unwrap();
		// A 16-picture lookahead releases the original input after bounded priming.
		for timestamp in 0..17 {
			delivery.submitted(timestamp, 3, true).unwrap();
		}
		assert!(delivery.needs_picture());
		delivery.received(0, 3);
		assert!(!delivery.needs_picture());
		// Receiving remaining duplicates never creates another input requirement.
		for timestamp in 1..17 {
			delivery.received(timestamp, 3);
			assert!(!delivery.needs_picture());
		}
		assert!(delivery.inputs.is_empty());
		delivery.fresh().unwrap();
		delivery.submitted(20, 3, true).unwrap();
		assert!(delivery.needs_picture());
		delivery.received(20, 3);
		assert!(!delivery.needs_picture());
	}

	#[test]
	fn capture_delivery_rejects_old_epochs_and_bounds_priming() {
		let mut delivery = CaptureDelivery::default();
		delivery.fresh().unwrap();
		delivery.submitted(5, 0, true).unwrap();
		delivery.retire();
		delivery.submitted(6, 1, true).unwrap();
		delivery.received(5, 0);
		assert!(delivery.needs_picture());
		delivery.received(6, 1);
		assert!(!delivery.needs_picture());
		delivery.fresh().unwrap();
		for timestamp in 0..crate::video_encode::MAX_PENDING_PICTURES as u32 {
			delivery.submitted(timestamp, 1, true).unwrap();
		}
		assert!(delivery.submitted(1000, 1, true).is_err());
		assert_eq!(
			delivery.inputs.len(),
			crate::video_encode::MAX_PENDING_PICTURES
		);
		// A poll without new input must remain legal when every real credit is used.
		delivery.submitted(1000, 1, false).unwrap();
		assert_eq!(
			delivery.inputs.len(),
			crate::video_encode::MAX_PENDING_PICTURES
		);
	}

	#[test]
	fn dropped_and_polled_pictures_do_not_create_capture_inputs() {
		let mut delivery = CaptureDelivery::default();
		let mut packet = crate::video_encode::EncodedPacket {
			data: Vec::new(),
			keyframe: false,
			timestamp: u32::MAX - 50,
			epoch: 4,
			input_accepted: false,
		};
		for timestamp in 0..96 {
			delivery.fresh().unwrap();
			delivery.record_packet(timestamp, &packet).unwrap();
			assert!(delivery.inputs.is_empty() && delivery.needs_picture());
		}
		packet.input_accepted = true;
		delivery.record_packet(packet.timestamp, &packet).unwrap();
		assert_eq!(delivery.inputs.len(), 1);
		// Receive-only output identifies its original input, including wrapping RTP
		// values; the dummy polling timestamp must never enter the ledger.
		packet.input_accepted = false;
		packet.data.push(1);
		delivery.record_packet(0, &packet).unwrap();
		assert!(delivery.inputs.is_empty() && !delivery.needs_picture());

		delivery.fresh().unwrap();
		delivery.submitted(1000, 4, true).unwrap();
		packet.data.clear();
		packet.epoch = 5;
		delivery.record_packet(0, &packet).unwrap();
		delivery.received(1000, 4);
		assert!(delivery.inputs.is_empty() && delivery.needs_picture());
	}

	#[test]
	fn progress_does_not_treat_a_static_lookahead_tail_as_missing_pictures() {
		let progress = Progress::default();
		let mut delivery = CaptureDelivery::default();
		progress.reset(7);
		delivery.fresh().unwrap();
		let mut packet = crate::video_encode::EncodedPacket {
			data: Vec::new(),
			timestamp: 0,
			keyframe: false,
			epoch: 1,
			input_accepted: true,
		};
		for timestamp in 0..16 {
			delivery.record_packet(timestamp, &packet).unwrap();
			progress.packet(7, &packet);
		}
		progress.pending(7, delivery.needs_picture());
		assert!(progress.snapshot().pending_since.is_some());
		packet.input_accepted = false;
		packet.data.push(1);
		delivery.record_packet(0, &packet).unwrap();
		progress.packet(7, &packet);
		progress.pending(7, delivery.needs_picture());
		let snapshot = progress.snapshot();
		assert_eq!((snapshot.accepted, snapshot.produced), (16, 1));
		assert_eq!(delivery.inputs.len(), 15);
		assert!(snapshot.pending_since.is_none());
		assert!(snapshot.last_output.is_some());
		progress.transmitted(7);
		assert_eq!(progress.snapshot().sent, 1);
		progress.reset(8);
		progress.packet(7, &packet);
		progress.pending(7, true);
		progress.transmitted(7);
		let snapshot = progress.snapshot();
		assert_eq!(
			(snapshot.generation, snapshot.accepted, snapshot.produced),
			(8, 0, 0)
		);
		assert!(snapshot.pending_since.is_none() && snapshot.last_output.is_none());
		assert_eq!(snapshot.sent, 0);
		progress.transmitted(8);
		assert_eq!(progress.snapshot().sent, 1);
	}

	#[test]
	fn choosing_source_keeps_its_deadline_and_native_start_gets_a_fresh_one() {
		let progress = Progress::default();
		let outer = progress.operation(NativeOperation::OpeningCapture);
		let choosing = progress.operation(NativeOperation::ChoosingSource);
		let chosen_at = progress.snapshot().operation.unwrap().1;
		assert_eq!(
			progress.native_failure_at(chosen_at + Duration::from_secs(45)),
			None
		);
		assert_eq!(
			progress.native_failure_at(chosen_at + Duration::from_secs(119)),
			None
		);
		assert_eq!(
			progress.native_failure_at(chosen_at + Duration::from_secs(120)),
			Some("Screen selection timed out; choose a source again")
		);
		drop(choosing);
		let opening = progress.operation(NativeOperation::OpeningCapture);
		let started_at = progress.snapshot().operation.unwrap().1;
		drop(outer);
		assert_eq!(
			progress.snapshot().operation,
			Some((NativeOperation::OpeningCapture, started_at))
		);
		assert_eq!(
			progress.native_failure_at(started_at + Duration::from_secs(29)),
			None
		);
		assert_eq!(
			progress.native_failure_at(started_at + Duration::from_secs(30)),
			Some("Screen capture did not finish starting")
		);
		drop(opening);
		assert!(progress.snapshot().operation.is_none());
	}

	#[test]
	fn native_watchdog_runs_without_encoder_progress_and_preserves_first_failure() {
		let progress = Progress::default();
		let operation = progress.operation(NativeOperation::Encoding);
		progress.0.lock().unwrap().snapshot.operation = Some((
			NativeOperation::Encoding,
			Instant::now() - Duration::from_secs(16),
		));
		assert_eq!(
			progress.native_failure(),
			Some("Screen video encoder stopped responding")
		);
		drop(operation);
		assert!(
			progress
				.result::<()>(Err("original encoder failure"))
				.is_err()
		);
		assert!(
			progress
				.result::<()>(Err("later teardown failure"))
				.is_err()
		);
		let _closing = progress.operation(NativeOperation::Retiring);
		assert_eq!(progress.native_failure(), Some("original encoder failure"));
	}

	#[test]
	fn worker_shutdown_cancels_before_native_completion() {
		let stop = Arc::new(AtomicBool::new(false));
		let ready = Arc::new(AtomicBool::new(true));
		let (send, done) = mpsc::sync_channel(1);
		let worker = Worker {
			stop: stop.clone(),
			ready: ready.clone(),
			progress: Arc::new(Progress::default()),
			output: Arc::new(Mutex::new(None)),
			bitrate: Arc::new(AtomicU32::new(1_000_000)),
			preview: Arc::new(Mutex::new(None)),
			done: Some(done),
			#[cfg(target_os = "linux")]
			status: Arc::new(Mutex::new("synthetic")),
			preview_visible: Arc::new(AtomicBool::new(false)),
		};
		let done = worker.shutdown();
		assert!(stop.load(Ordering::Acquire));
		assert!(!ready.load(Ordering::Acquire));
		assert_eq!(done.try_recv(), Err(mpsc::TryRecvError::Empty));
		send.send(Ok(())).unwrap();
		assert_eq!(done.try_recv(), Ok(Ok(())));
	}

	#[test]
	fn hiding_preview_discards_pending_images_without_changing_stream_readiness() {
		let ready = Arc::new(AtomicBool::new(true));
		let worker = Worker {
			stop: Arc::new(AtomicBool::new(false)),
			progress: Arc::new(Progress::default()),
			ready: ready.clone(),
			output: Arc::new(Mutex::new(None)),
			bitrate: Arc::new(AtomicU32::new(1_000_000)),
			preview: Arc::new(Mutex::new(Some(image::RgbaImage::new(2, 2)))),
			done: None,
			#[cfg(target_os = "linux")]
			status: Arc::new(Mutex::new("synthetic")),
			preview_visible: Arc::new(AtomicBool::new(true)),
		};
		worker.set_preview_visible(false);
		assert!(worker.take_preview().is_none());
		assert!(worker.preview.lock().unwrap().is_none());
		assert!(ready.load(Ordering::Acquire));
		worker.set_preview_visible(true);
		*worker.preview.lock().unwrap() = Some(image::RgbaImage::new(2, 2));
		assert!(worker.take_preview().is_some());
		assert!(ready.load(Ordering::Acquire));
	}

	#[test]
	fn source_aspect_ratios_encode_the_resolved_dimensions_and_fresh_keyframes() {
		use openh264::formats::YUVSource;
		for (source, expected) in [
			((1280, 720), (854, 480)),
			((1280, 800), (768, 480)),
			((1280, 960), (640, 480)),
			((1280, 1024), (600, 480)),
			((1500, 1000), (720, 480)),
			((1720, 720), (1146, 480)),
			((2560, 720), (1706, 480)),
			((6144, 2560), (1152, 480)),
			((720, 1280), (480, 854)),
			((720, 720), (480, 480)),
		] {
			let request = RequestSettings {
				source: SourceId::Display(1),
				resolution: client_core::screen::Resolution::Preset(
					model::voice_settings::VideoResolution::P480,
				),
				fps: 30,
				cursor: true,
				audio: false,
			};
			let settings = request.resolve(source).unwrap();
			assert_eq!((settings.width, settings.height), expected);
			let mut encoder = ScreenEncoder::new_on_adapter(
				settings,
				settings.bit_rate(),
				VideoSettings::default(),
				// Keep this one-picture geometry fixture independent of native
				// hardware buffering; explicit encoder tests cover that separately.
				Some(model::VideoAdapter::default()),
				0,
				0,
			)
			.unwrap();
			let frame = RawFrame {
				format: RawFormat::Bgra8,
				width: source.0,
				height: source.1,
				stride: source.0 as usize * 4,
				data: vec![128; source.0 as usize * source.1 as usize * 4],
			};
			let (encoded, keyframe) = encoder.encode(&frame, true).unwrap();
			assert!(keyframe && crate::video_receive::has_parameter_sets(&encoded));
			let mut decoder = openh264::decoder::Decoder::new().unwrap();
			assert_eq!(
				decoder.decode(&encoded).unwrap().unwrap().dimensions(),
				(expected.0 as usize, expected.1 as usize)
			);
			assert!(encoder.reset_for_security(1).unwrap());
			let (encoded, keyframe) = encoder.encode(&frame, false).unwrap();
			assert!(keyframe && crate::video_receive::has_parameter_sets(&encoded));
		}
	}
	#[test]
	fn output_generation_retires_older_geometry_and_bounds_invalid_publication() {
		let output = Arc::new(Mutex::new(None));
		let ready = Arc::new(AtomicBool::new(true));
		let keyframe = Arc::new(AtomicBool::new(false));
		let bitrate = Arc::new(AtomicU32::new(0));
		let settings = Settings {
			source: SourceId::Display(1),
			width: 6144,
			height: 2560,
			fps: 60,
			cursor: true,
			audio: false,
		};
		let first = publish_output(
			&output,
			settings,
			(settings.width, settings.height),
			&ready,
			&keyframe,
			&bitrate,
			Default::default(),
		)
		.unwrap();
		assert_eq!(first.generation, 0);
		assert!(!ready.load(Ordering::Acquire));
		assert!(keyframe.load(Ordering::Acquire));
		assert_eq!(bitrate.load(Ordering::Acquire), first.rates.maximum);
		let (_, frames) = tokio::sync::mpsc::channel(1);
		let video = Video {
			progress: Arc::new(Progress::default()),
			video_settings: VideoSettings::default(),
			output: output.clone(),
			frames,
			ready: ready.clone(),
			keyframe: keyframe.clone(),
			bitrate: bitrate.clone(),
			audio: None,
			audio_epoch: Arc::new(AtomicU64::new(0)),
		};
		assert!(video.enable_output(first));
		assert_eq!(
			publish_output(
				&output,
				settings,
				(settings.width, settings.height),
				&ready,
				&keyframe,
				&bitrate,
				Default::default(),
			)
			.unwrap(),
			first
		);
		assert!(ready.load(Ordering::Acquire));
		let resized = publish_output(
			&output,
			Settings {
				width: 2592,
				height: 1080,
				..settings
			},
			(6144, 2560),
			&ready,
			&keyframe,
			&bitrate,
			Default::default(),
		)
		.unwrap();
		assert_eq!(resized.generation, 1);
		assert!(!video.enable_output(first));
		assert!(!ready.load(Ordering::Acquire));
		assert!(video.enable_output(resized));
		assert!(
			publish_output(
				&output,
				Settings {
					width: 0,
					..settings
				},
				(6144, 2560),
				&ready,
				&keyframe,
				&bitrate,
				Default::default(),
			)
			.is_err()
		);
		assert_eq!(video.output(), Some(resized));
		let same_output = publish_output(
			&output,
			resized.settings,
			(3072, 1280),
			&ready,
			&keyframe,
			&bitrate,
			Default::default(),
		)
		.unwrap();
		assert_eq!(same_output.settings, resized.settings);
		assert_eq!(same_output.generation, 2);
		assert!(!ready.load(Ordering::Acquire));
		assert!(!video.enable_output(resized));
		assert!(video.enable_output(same_output));

		*output.lock().unwrap() = Some(Output {
			generation: u64::MAX,
			..resized
		});
		assert!(
			publish_output(
				&output,
				settings,
				(settings.width, settings.height),
				&ready,
				&keyframe,
				&bitrate,
				Default::default(),
			)
			.is_err()
		);
	}

	#[test]
	fn high_bandwidth_restart_keeps_a_ceiling_above_medium_and_emits_a_fresh_idr() {
		let settings = Settings {
			source: SourceId::Display(1),
			width: 320,
			height: 240,
			fps: 60,
			cursor: false,
			audio: false,
		};
		let video = VideoSettings {
			bandwidth: model::voice_settings::VideoBandwidth::High,
			..Default::default()
		};
		let maximum = video.bandwidth.rates(320, 240, 60).maximum;
		let mut encoder = ScreenEncoder::new_on_adapter(
			settings,
			maximum,
			video,
			Some(model::VideoAdapter::default()),
			0,
			0,
		)
		.unwrap();
		let frame = RawFrame {
			format: RawFormat::Bgra8,
			width: 320,
			height: 240,
			stride: 320 * 4,
			data: vec![128; 320 * 240 * 4],
		};
		let first = encoder.encode_at(&frame, false, 0).unwrap();
		assert!(first.keyframe);
		assert!(encoder.set_bitrate(3_000_000).unwrap());
		assert_eq!(encoder.bitrate, 3_000_000);
		assert!(encoder.bitrate > settings.bit_rates().maximum);
		let next = encoder.encode_at(&frame, false, 1500).unwrap();
		assert!(next.keyframe && next.epoch > first.epoch);
		assert!(crate::video_receive::has_parameter_sets(&next.data));
	}

	#[test]
	fn selected_bandwidth_and_quality_reach_encoder_without_medium_clamping() {
		use model::voice_settings::{EncoderPreset, VideoBandwidth};
		let settings = Settings {
			source: SourceId::Display(1),
			width: 2560,
			height: 1440,
			fps: 60,
			cursor: true,
			audio: false,
		};
		for bandwidth in VideoBandwidth::ALL {
			let selected = VideoSettings {
				bandwidth,
				quality: EncoderPreset::AmfQuality,
				..Default::default()
			};
			let rates = bandwidth.rates(settings.width, settings.height, settings.fps);
			let initial = ScreenEncoder::config(settings, rates.maximum, selected);
			assert_eq!(initial.rates, rates);
			assert_eq!(initial.preset, selected.quality);
			let reduced = ScreenEncoder::config(settings, 400_000, selected);
			assert_eq!(
				(
					reduced.rates.minimum,
					reduced.rates.average,
					reduced.rates.maximum
				),
				(200_000, 400_000, 400_000)
			);
		}
	}

	#[test]
	fn screen_recovery_coalesces_feedback_and_always_honors_security_resets() {
		let settings = Settings {
			source: SourceId::Display(1),
			width: 320,
			height: 240,
			fps: 30,
			cursor: false,
			audio: false,
		};
		let video_settings = VideoSettings::default();
		let software = crate::video_encode::Encoder::software(ScreenEncoder::config(
			settings,
			settings.bit_rate(),
			video_settings,
		))
		.unwrap();
		let mut encoder = ScreenEncoder {
			diagnostics: crate::diagnostics::EncoderRegistration::new(
				true,
				false,
				crate::diagnostics::AmfSplit::Off,
			),
			encoder: Some(software),
			stop: Arc::new(AtomicBool::new(false)),
			video_settings,
			i420: Vec::new(),
			converter: convert::Converter::default(),
			#[cfg(target_os = "windows")]
			capture_bytes: Vec::new(),
			#[cfg(target_os = "windows")]
			surface_input: false,
			settings,
			bitrate: settings.bit_rate(),
			adapter: None,
			epoch_base: 7,
			awaiting_keyframe: true,
			reset_generation: 0,
		};
		let raw = RawFrame {
			format: crate::screen::RawFormat::Bgra8,
			width: 2,
			height: 2,
			stride: 8,
			data: vec![128; 16],
		};
		encoder.restart().unwrap();
		encoder.restart().unwrap();
		assert_eq!(encoder.epoch(), 7);
		assert!(!encoder.set_bitrate(settings.bit_rate() / 2).unwrap());
		let first = encoder.encode_at(&raw, true, 6000).unwrap();
		assert!(first.keyframe);
		assert_eq!((first.epoch, first.timestamp), (7, 6000));
		encoder.restart().unwrap();
		encoder.restart().unwrap();
		assert_eq!(encoder.epoch(), 8);
		let recovered = encoder.encode_at(&raw, false, 12000).unwrap();
		assert!(recovered.keyframe);
		assert_eq!((recovered.epoch, recovered.timestamp), (8, 12000));
		assert!(crate::video_receive::has_parameter_sets(&recovered.data));
		encoder.restart().unwrap();
		assert!(encoder.awaiting_keyframe);
		assert_eq!(encoder.epoch(), 9);
		assert!(encoder.reset_for_security(1).unwrap());
		assert_eq!(encoder.epoch(), 10);
		assert_eq!(encoder.reset_generation, 1);
		assert!(!encoder.reset_for_security(1).unwrap());
		let fresh = encoder.encode_at(&raw, false, 18000).unwrap();
		assert_eq!((fresh.epoch, fresh.timestamp), (10, 18000));
		assert!(fresh.keyframe && crate::video_receive::has_parameter_sets(&fresh.data));
		use openh264::formats::YUVSource;
		let mut decoder = openh264::decoder::Decoder::new().unwrap();
		assert_eq!(
			decoder.decode(&fresh.data).unwrap().unwrap().dimensions(),
			(320, 240)
		);
		assert!(encoder.reset_for_security(u64::MAX).is_err());
	}

	#[test]
	fn idle_screen_keyframe_uses_latest_snapshot_only_when_ready() {
		let frame = |value| RawFrame {
			format: crate::screen::RawFormat::Bgra8,
			width: 2,
			height: 2,
			stride: 8,
			data: vec![value; 16],
		};
		let mut latest = None;
		assert!(!retain_screen_frame(&mut latest, None, true, true).unwrap());
		assert!(retain_screen_frame(&mut latest, Some(frame(1)), true, false).unwrap());
		assert!(!retain_screen_frame(&mut latest, None, true, false).unwrap());
		assert!(retain_screen_frame(&mut latest, None, true, true).unwrap());
		// A security pause keeps tracking the source without encoding. Its newest snapshot
		// can be freshly encoded after readiness, even if no further capture event arrives.
		assert!(!retain_screen_frame(&mut latest, Some(frame(2)), false, true).unwrap());
		assert!(!retain_screen_frame(&mut latest, None, false, true).unwrap());
		assert!(retain_screen_frame(&mut latest, None, true, true).unwrap());
		assert_eq!(latest.as_ref().unwrap().data, vec![2; 16]);
		let mut oversized = frame(3);
		oversized.width = model::voice_settings::VideoResolution::MAX_WIDTH + 1;
		assert!(retain_screen_frame(&mut latest, Some(oversized), true, true).is_err());
		assert_eq!(latest.unwrap().data, vec![2; 16]);
	}

	#[test]
	fn local_preview_is_bounded_and_converts_padded_bgra_without_media_readiness() {
		let preview = preview_frame(&RawFrame {
			format: crate::screen::RawFormat::Bgra8,
			width: 1,
			height: 2,
			stride: 8,
			data: vec![1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 0, 9, 9, 9, 9],
		})
		.unwrap();
		assert_eq!(preview.as_raw(), &[3, 2, 1, 255, 6, 5, 4, 255]);
		let worker = Worker {
			stop: Arc::new(AtomicBool::new(false)),
			progress: Arc::new(Progress::default()),
			ready: Arc::new(AtomicBool::new(false)),
			output: Arc::new(Mutex::new(None)),
			bitrate: Arc::new(AtomicU32::new(model::voice_settings::VideoBitrate::MINIMUM)),
			preview: Arc::new(Mutex::new(Some(preview))),
			done: None,
			#[cfg(target_os = "linux")]
			status: Arc::new(Mutex::new("")),
			preview_visible: Arc::new(AtomicBool::new(true)),
		};
		assert!(worker.take_preview().is_some());
		assert!(worker.take_preview().is_none());
		assert!(!worker.ready.load(Ordering::Acquire));
		for (width, height) in [(3840, 2160), (2, 4320), (7680, 2)] {
			let frame = RawFrame {
				format: crate::screen::RawFormat::Bgra8,
				width,
				height,
				stride: width as usize * 4,
				data: vec![0; width as usize * height as usize * 4],
			};
			let preview = preview_frame(&frame).unwrap();
			assert!(preview.width() > 0 && preview.width() <= 640);
			assert!(preview.height() > 0 && preview.height() <= 360);
			assert!(preview.as_raw().len() <= 640 * 360 * 4);
		}
		assert!(
			preview_frame(&RawFrame {
				format: crate::screen::RawFormat::Bgra8,
				width: 2,
				height: 2,
				stride: 4,
				data: vec![0; 8],
			})
			.is_err()
		);
	}

	#[test]
	fn synthetic_frame_is_bounded_and_encodes_a_keyframe() {
		let frame = RawFrame {
			format: crate::screen::RawFormat::Bgra8,
			width: 2,
			height: 2,
			stride: 12,
			data: vec![255; 24],
		};
		let mut pixels = vec![0; 1280 * 720 * 3 / 2];
		convert::bgra_to_i420(&frame, 1280, 720, &mut pixels).unwrap();
		assert_eq!(pixels.len(), 1280 * 720 * 3 / 2);

		let settings = Settings {
			source: SourceId::Display(1),
			width: 1280,
			height: 720,
			fps: 30,
			cursor: true,
			audio: false,
		};
		let video_settings = VideoSettings::default();
		let encoder = crate::video_encode::Encoder::software(ScreenEncoder::config(
			settings,
			settings.bit_rate(),
			video_settings,
		))
		.unwrap();
		let mut encoder = ScreenEncoder {
			diagnostics: crate::diagnostics::EncoderRegistration::new(
				true,
				false,
				crate::diagnostics::AmfSplit::Off,
			),
			encoder: Some(encoder),
			video_settings,
			i420: Vec::new(),
			stop: Arc::new(AtomicBool::new(false)),
			converter: convert::Converter::default(),
			#[cfg(target_os = "windows")]
			capture_bytes: Vec::new(),
			#[cfg(target_os = "windows")]
			surface_input: false,
			settings,
			bitrate: settings.bit_rate(),
			adapter: None,
			epoch_base: 0,
			awaiting_keyframe: true,
			reset_generation: 0,
		};
		let raw = frame;
		let (encoded, keyframe) = encoder.encode(&raw, true).unwrap();
		assert!(keyframe);
		assert!(!encoded.is_empty() && encoded.len() <= MAX_ENCODED_BYTES);
		assert!(crate::video_receive::is_keyframe(&encoded));
		assert!(crate::video_receive::has_parameter_sets(&encoded));
		use openh264::formats::YUVSource;
		let mut decoder = openh264::decoder::Decoder::new().unwrap();
		assert_eq!(
			decoder.decode(&encoded).unwrap().unwrap().dimensions(),
			(1280, 720)
		);
		assert!(encoder.set_bitrate(settings.bit_rate() / 2).unwrap());
		let (encoded, keyframe) = encoder.encode(&raw, true).unwrap();
		assert!(keyframe && crate::video_receive::has_parameter_sets(&encoded));

		assert!(
			convert::bgra_to_i420(
				&RawFrame {
					format: crate::screen::RawFormat::Bgra8,
					width: 2,
					height: 2,
					stride: 4,
					data: vec![0; 8],
				},
				1280,
				720,
				&mut pixels,
			)
			.is_err()
		);
	}
}
