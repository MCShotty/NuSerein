#[cfg(target_os = "windows")]
use crate::screen::AudioStatus;
use crate::screen::{AudioChunk, MAX_AUDIO_SAMPLES};
#[cfg(target_os = "windows")]
use std::sync::Mutex;
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::mpsc::Sender;

#[cfg(target_os = "windows")]
pub(super) struct Audio {
	thread: Option<std::thread::JoinHandle<()>>,
	stop: Arc<AtomicBool>,
}

#[cfg(target_os = "windows")]
pub(super) use native::Target;

#[cfg(target_os = "windows")]
impl Audio {
	#[allow(clippy::too_many_arguments)] // One selected target and its bounded audio/control outputs.
	pub(super) fn start(
		target: Target,
		send: Sender<AudioChunk>,
		stop: Arc<AtomicBool>,
		cancel: Arc<AtomicBool>,
		ready: Arc<AtomicBool>,
		audio_epoch: Arc<AtomicU64>,
		status: Arc<Mutex<AudioStatus>>,
	) -> Result<Self, &'static str> {
		let audio_stop = Arc::new(AtomicBool::new(false));
		let failed = Arc::new(AtomicBool::new(false));
		let input = Samples {
			send,
			stop: audio_stop.clone(),
			capture_stop: stop,
			cancel,
			ready,
			epoch: audio_epoch,
			failed: failed.clone(),
		};
		let (started, result) = std::sync::mpsc::sync_channel(1);
		let thread = std::thread::Builder::new()
			.name("screen-audio".into())
			.spawn(move || {
				let result = native::run(&input, &target, started, &status);
				if (input.failed.load(Ordering::Acquire) || (result.is_err() && !input.stopped()))
					&& !input.cancel.load(Ordering::Acquire)
					&& !input.capture_stop.load(Ordering::Acquire)
				{
					input.failed.store(true, Ordering::Release);
					input.stop.store(true, Ordering::Release);
					crate::screen::set_audio_status(
						&status,
						AudioStatus::Unavailable(target.unavailable()),
					);
				}
			})
			.map_err(|_| "System audio worker could not start")?;
		let audio = Self {
			thread: Some(thread),
			stop: audio_stop,
		};
		result
			.recv_timeout(std::time::Duration::from_secs(4))
			.map_err(
				|_| "Echo-free system audio could not start; Windows build 20348 or newer is required",
			)?;
		Ok(audio)
	}
}

#[cfg(target_os = "windows")]
impl Drop for Audio {
	fn drop(&mut self) {
		self.stop.store(true, Ordering::Release);
		if let Some(thread) = self.thread.take() {
			let _ = thread.join();
		}
	}
}

// WASAPI owns each packet until ReleaseBuffer. COM and capture objects stay on this worker.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod native {
	use super::*;
	use crate::diagnostics::{Metrics, Scope, Stage};
	use std::{
		mem::{ManuallyDrop, size_of},
		os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
		sync::mpsc::{SyncSender, sync_channel},
		time::{Duration, Instant},
	};
	use windows::{
		Win32::{
			Foundation::{E_FAIL, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
			Media::Audio::*,
			System::{
				Com::{
					BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
					StructuredStorage::{
						PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
					},
				},
				Threading::{
					CreateEventW, GetProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
					PROCESS_SYNCHRONIZE, WaitForSingleObject,
				},
				Variant::VT_BLOB,
			},
		},
		core::{HRESULT, Interface, Ref, Result, implement},
	};

	pub(crate) enum Target {
		System,
		Application {
			pid: u32,
			window: windows_capture::window::Window,
			// Keep the exact process object alive across activation and every security reset.
			process: OwnedHandle,
		},
	}
	impl Target {
		pub(crate) fn application(
			window: windows_capture::window::Window,
		) -> std::result::Result<Self, &'static str> {
			let pid = window
				.process_id()
				.map_err(|_| "Application audio is unavailable; sharing video only")?;
			if pid == 0 || pid == std::process::id() {
				return Err("This application's audio cannot be shared; sharing video only");
			}
			let process = unsafe {
				OpenProcess(
					PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
					false,
					pid,
				)
			}
			.map_err(
				|_| "The selected application's audio cannot be isolated; sharing video only",
			)?;
			let target = Self::Application {
				pid,
				window,
				process: unsafe { OwnedHandle::from_raw_handle(process.0) },
			};
			if !target.valid() {
				return Err("The selected application changed or closed; sharing video only");
			}
			Ok(target)
		}
		fn valid(&self) -> bool {
			match self {
				Self::System => true,
				Self::Application {
					pid,
					window,
					process,
				} => {
					let handle = HANDLE(process.as_raw_handle());
					owner_matches(
						*pid,
						window.process_id().ok(),
						unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT,
					) && unsafe { GetProcessId(handle) } == *pid
				}
			}
		}
		fn activation(&self) -> AUDIOCLIENT_ACTIVATION_PARAMS {
			match self {
				Self::System => activation_params(std::process::id(), false),
				Self::Application { pid, .. } => activation_params(*pid, true),
			}
		}
		fn status(&self) -> AudioStatus {
			match self {
				Self::System => AudioStatus::System,
				Self::Application { .. } => AudioStatus::Application,
			}
		}
		pub(super) fn unavailable(&self) -> &'static str {
			match self {
				Self::System => "System audio is unavailable; sharing video only",
				Self::Application { .. } => {
					"The selected application's audio stopped or is unavailable; sharing video only"
				}
			}
		}
	}

	fn owner_matches(expected: u32, observed: Option<u32>, alive: bool) -> bool {
		expected != 0 && alive && observed == Some(expected)
	}

	#[implement(IActivateAudioInterfaceCompletionHandler)]
	struct Completion {
		send: SyncSender<()>,
		// Keep the activation blob alive even if cancellation wins the completion race.
		_params: Arc<AUDIOCLIENT_ACTIVATION_PARAMS>,
	}

	impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
		fn ActivateCompleted(
			&self,
			_: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
		) -> Result<()> {
			let _ = self.send.try_send(());
			Ok(())
		}
	}

	fn activation_params(pid: u32, include: bool) -> AUDIOCLIENT_ACTIVATION_PARAMS {
		AUDIOCLIENT_ACTIVATION_PARAMS {
			ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
			Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
				ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
					TargetProcessId: pid,
					ProcessLoopbackMode: if include {
						PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
					} else {
						PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
					},
				},
			},
		}
	}

	pub(super) fn run(
		input: &Samples,
		target: &Target,
		started: SyncSender<()>,
		status: &Mutex<AudioStatus>,
	) -> Result<()> {
		// This dedicated thread initializes and releases all COM objects in one MTA.
		unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
		let result = (|| {
			let mut started = Some(started);
			while !input.stopped() && !input.send.is_closed() {
				capture(input, target, &mut started, status)?;
			}
			Ok(())
		})();
		unsafe { CoUninitialize() };
		result
	}

	fn capture(
		input: &Samples,
		target: &Target,
		started: &mut Option<SyncSender<()>>,
		status: &Mutex<AudioStatus>,
	) -> Result<()> {
		if input.stopped() || !target.valid() {
			return Err(E_FAIL.into());
		}
		let params = Arc::new(target.activation());
		let (send, done) = sync_channel(1);
		let completion: IActivateAudioInterfaceCompletionHandler = Completion {
			send,
			_params: params.clone(),
		}
		.into();
		// The blob points into `params`; PROPVARIANT's Rust Drop would call
		// PropVariantClear and try to free that Arc allocation with CoTaskMemFree.
		// The completion handler owns `params` for the whole async activation.
		let variant = ManuallyDrop::new(PROPVARIANT {
			Anonymous: PROPVARIANT_0 {
				Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
					vt: VT_BLOB,
					Anonymous: PROPVARIANT_0_0_0 {
						blob: BLOB {
							cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
							pBlobData: Arc::as_ptr(&params).cast_mut().cast(),
						},
					},
					..Default::default()
				}),
			},
		});
		// The native operation retains the agile completion handler and its immutable blob.
		let operation = unsafe {
			ActivateAudioInterfaceAsync(
				VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
				&IAudioClient::IID,
				Some(&*variant),
				&completion,
			)?
		};
		let deadline = Instant::now() + Duration::from_secs(3);
		loop {
			if input.stopped() || !target.valid() || Instant::now() >= deadline {
				return Err(E_FAIL.into());
			}
			if done.recv_timeout(Duration::from_millis(50)).is_ok() {
				break;
			}
		}
		let mut activation_status = HRESULT(0);
		let mut activated = None;
		// Completion has fired; GetActivateResult returns an owned COM reference.
		unsafe { operation.GetActivateResult(&mut activation_status, &mut activated)? };
		drop(operation);
		activation_status.ok()?;
		if input.stopped() || input.send.is_closed() || !target.valid() {
			return Err(E_FAIL.into());
		}
		let event = unsafe { CreateEventW(None, false, false, None)? };
		// Created before the clients so it stays open until their COM references are released.
		let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
		let client: IAudioClient = activated.ok_or(E_FAIL)?.cast()?;
		let format = WAVEFORMATEX {
			wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
			nChannels: 2,
			nSamplesPerSec: 48_000,
			nAvgBytesPerSec: 48_000 * 8,
			nBlockAlign: 8,
			wBitsPerSample: 32,
			cbSize: 0,
		};
		// Native conversion gives stereo 48 kHz independently of physical output formats.
		unsafe {
			client.Initialize(
				AUDCLNT_SHAREMODE_SHARED,
				AUDCLNT_STREAMFLAGS_LOOPBACK
					| AUDCLNT_STREAMFLAGS_EVENTCALLBACK
					| AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
				0,
				0,
				&format,
				None,
			)?;
		}
		if unsafe { client.GetBufferSize()? } as usize > MAX_AUDIO_SAMPLES / 2 {
			return Err(E_FAIL.into());
		}
		let capture: IAudioCaptureClient = unsafe { client.GetService()? };
		unsafe { client.SetEventHandle(HANDLE(event.as_raw_handle()))? };
		if input.stopped() || input.send.is_closed() || !target.valid() {
			return Err(E_FAIL.into());
		}
		let epoch = input.epoch.load(Ordering::Acquire);
		let mut metrics = Metrics::new(Scope::ScreenAudio);
		metrics.checkpoint();
		let start = metrics.start();
		unsafe { client.Start()? };
		if input.stopped() || !target.valid() {
			let _ = unsafe { client.Stop() };
			return Err(E_FAIL.into());
		}
		crate::screen::set_audio_status(status, target.status());
		metrics.finish(Stage::CaptureRestart, start);
		metrics.checkpoint();
		if let Some(started) = started.take() {
			let _ = started.try_send(());
		}
		let result = packets(input, target, &capture, &event, epoch, &mut metrics);
		// Always stop before releasing the capture client and event, including packet errors.
		let stopped = unsafe { client.Stop() };
		result.and(stopped)
	}

	fn packets(
		input: &Samples,
		target: &Target,
		capture: &IAudioCaptureClient,
		event: &OwnedHandle,
		epoch: u64,
		metrics: &mut Metrics,
	) -> Result<()> {
		while !input.stopped() && !input.send.is_closed() {
			if !target.valid() {
				return Err(E_FAIL.into());
			}
			if input.epoch.load(Ordering::Acquire) != epoch {
				// Stop/Reset/Start can leave process loopback permanently empty on Windows.
				// Recreate it instead; the old client's immutable epoch prevents buffered
				// audio from being relabeled across the encryption transition.
				metrics.poll(true, 0, false, 0);
				metrics.checkpoint();
				return Ok(());
			}
			match unsafe { WaitForSingleObject(HANDLE(event.as_raw_handle()), 50) } {
				WAIT_TIMEOUT => {
					metrics.poll(false, 0, true, 0);
					continue;
				}
				WAIT_OBJECT_0 => {}
				_ => return Err(E_FAIL.into()),
			}
			let mut dropped = 0;
			// Bound work per wakeup as well as the bytes in each packet.
			for _ in 0..32 {
				if input.stopped()
					|| !target.valid()
					|| unsafe { capture.GetNextPacketSize()? } == 0
				{
					break;
				}
				let start = metrics.start();
				let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
				unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)? };
				metrics.finish(Stage::CaptureRead, start);
				let count = (frames as usize).saturating_mul(2);
				let valid = count <= MAX_AUDIO_SAMPLES
					&& (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0
						|| (!data.is_null() && data.align_offset(align_of::<f32>()) == 0));
				if valid && count != 0 && target.valid() {
					let start = metrics.start();
					let queued = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
						input.push_at(&[0.0; MAX_AUDIO_SAMPLES][..count], epoch)
					} else {
						// WASAPI guarantees frames of the initialized format until ReleaseBuffer.
						input.push_at(
							unsafe { std::slice::from_raw_parts(data.cast::<f32>(), count) },
							epoch,
						)
					};
					if queued {
						metrics.finish(Stage::CaptureQueue, start);
					} else {
						dropped += 1;
					}
				}
				unsafe { capture.ReleaseBuffer(frames)? };
				if !valid {
					return Err(E_FAIL.into());
				}
			}
			metrics.poll(false, dropped, false, 0);
		}
		Ok(())
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		#[test]
		fn activation_excludes_our_entire_process_tree() {
			let params = activation_params(42, false);
			assert_eq!(
				params.ActivationType,
				AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
			);
			let process = unsafe { params.Anonymous.ProcessLoopbackParams };
			assert_eq!(process.TargetProcessId, 42);
			assert_eq!(
				process.ProcessLoopbackMode,
				PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
			);
		}
		#[test]
		fn activation_includes_only_the_selected_application_tree() {
			let params = activation_params(1234, true);
			let process = unsafe { params.Anonymous.ProcessLoopbackParams };
			assert_eq!(process.TargetProcessId, 1234);
			assert_eq!(
				process.ProcessLoopbackMode,
				PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
			);
			assert_eq!(Target::System.status(), AudioStatus::System);
		}
		#[test]
		fn closed_or_reassigned_windows_never_retarget_audio() {
			assert!(owner_matches(42, Some(42), true));
			assert!(!owner_matches(42, Some(43), true));
			assert!(!owner_matches(42, Some(42), false));
			assert!(!owner_matches(42, None, true));
			assert!(!owner_matches(0, Some(0), true));
		}
	}
}

struct Samples {
	send: Sender<AudioChunk>,
	/// This stop belongs only to audio; malformed PCM must not stop video.
	stop: Arc<AtomicBool>,
	capture_stop: Arc<AtomicBool>,
	cancel: Arc<AtomicBool>,
	ready: Arc<AtomicBool>,
	epoch: Arc<AtomicU64>,
	failed: Arc<AtomicBool>,
}

impl Samples {
	fn stopped(&self) -> bool {
		self.stop.load(Ordering::Acquire)
			|| self.capture_stop.load(Ordering::Acquire)
			|| self.cancel.load(Ordering::Acquire)
	}
	#[cfg(test)]
	fn push(&self, data: &[f32]) {
		let epoch = self.epoch.load(Ordering::Acquire);
		self.push_at(data, epoch);
	}

	fn push_at(&self, data: &[f32], epoch: u64) -> bool {
		if self.stopped()
			|| !self.ready.load(Ordering::Acquire)
			|| epoch != self.epoch.load(Ordering::Acquire)
		{
			return false;
		}
		if data.len() > MAX_AUDIO_SAMPLES || !data.len().is_multiple_of(2) {
			self.failed.store(true, Ordering::Release);
			self.stop.store(true, Ordering::Release);
			return false;
		}
		if data.is_empty() {
			return false;
		}
		// Reserve first: a stalled transport must not allocate for dropped packets.
		if let Ok(permit) = self.send.try_reserve() {
			let samples = data
				.iter()
				.map(|&sample| {
					if sample.is_finite() {
						sample.clamp(-1.0, 1.0)
					} else {
						0.0
					}
				})
				.collect();
			if !self.stopped()
				&& self.ready.load(Ordering::Acquire)
				&& epoch == self.epoch.load(Ordering::Acquire)
			{
				permit.send(AudioChunk { samples, epoch });
				return true;
			}
		}
		false
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn loopback_samples_are_gated_sanitized_and_bounded() {
		let (send, mut receive) = tokio::sync::mpsc::channel(1);
		let samples = Samples {
			send,
			stop: Arc::new(AtomicBool::new(false)),
			capture_stop: Arc::new(AtomicBool::new(false)),
			cancel: Arc::new(AtomicBool::new(false)),
			ready: Arc::new(AtomicBool::new(false)),
			epoch: Arc::new(AtomicU64::new(7)),
			failed: Arc::new(AtomicBool::new(false)),
		};
		assert!(!samples.push_at(&[0.25, -0.25], 7));
		assert!(receive.try_recv().is_err());
		samples.ready.store(true, Ordering::Release);
		assert!(samples.push_at(&[f32::NAN, f32::INFINITY, -2.0, 2.0], 7));
		assert!(!samples.push_at(&[0.5, -0.5], 7)); // Full queue drops the new chunk.
		samples.epoch.store(8, Ordering::Release);
		let chunk = receive.try_recv().unwrap();
		assert_eq!(chunk.samples, vec![0.0, 0.0, -1.0, 1.0]);
		assert_eq!(chunk.epoch, 7); // Queued samples retain their capture generation.
		assert!(receive.try_recv().is_err());
		assert!(!samples.push_at(&[0.25, -0.25], 7)); // A native wakeup across rekey is stale.
		assert!(receive.try_recv().is_err());
		samples.push(&vec![0.0; MAX_AUDIO_SAMPLES]);
		let chunk = receive.try_recv().unwrap();
		assert_eq!(chunk.samples.len(), MAX_AUDIO_SAMPLES);
		assert_eq!(chunk.epoch, 8);
		samples.stop.store(true, Ordering::Release);
		samples.push(&[0.25, -0.25]);
		assert!(receive.try_recv().is_err());
		samples.stop.store(false, Ordering::Release);
		samples.capture_stop.store(true, Ordering::Release);
		samples.push(&[0.25, -0.25]);
		assert!(receive.try_recv().is_err());
		assert!(samples.stopped());
		samples.capture_stop.store(false, Ordering::Release);
		// User cancellation reaches loopback immediately, independently of
		// an encoder worker which has not finished unwinding native resources.
		samples.cancel.store(true, Ordering::Release);
		assert!(samples.stopped());
		samples.push(&[0.25, -0.25]);
		assert!(receive.try_recv().is_err());
		assert!(!samples.stop.load(Ordering::Acquire));
		assert!(!samples.failed.load(Ordering::Acquire));
		samples.cancel.store(false, Ordering::Release);
		samples.push(&vec![0.0; MAX_AUDIO_SAMPLES + 2]);
		assert!(samples.failed.load(Ordering::Acquire));
		assert!(samples.stop.load(Ordering::Acquire));
		assert!(
			!samples.capture_stop.load(Ordering::Acquire),
			"audio failure cannot stop video"
		);
		assert!(receive.try_recv().is_err());
	}
}
