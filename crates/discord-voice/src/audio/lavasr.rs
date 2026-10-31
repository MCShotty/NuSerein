//! Bounded local speech-helper protocol and capture-generation retirement.
//! The supervisor owns process I/O and deadlines; the audio worker only polls bounded queues.
use crate::{CapturedFrame, Frame};
use platform::speech::{Assets, ENHANCER_WINDOW, Enhancer, HOP, LOOKAHEAD, OVERLAP, WINDOW};
use std::{
	collections::VecDeque,
	io::{self, Read, Write},
	process::{Child, Command, Stdio},
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
		mpsc::{self, Receiver, SyncSender},
	},
	thread,
	time::{Duration, Instant},
};

const READY: u8 = 0;
const PCM: u8 = 1;
const RESET: u8 = 2;
const ERROR: u8 = 3;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const PROCESS_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_INPUT_AGE: Duration = Duration::from_millis(320);
const INPUT_BLOCKS: usize = 3;
const OUTPUT_BLOCKS: usize = 2;
const MAX_OUTPUT_FRAMES: usize = 12;
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
	Loading = 1,
	Warming = 2,
	Active = 3,
	Fallback = 4,
	Unavailable = 5,
	Overloaded = 6,
}
impl Status {
	pub(crate) fn from_code(code: u8) -> Self {
		match code {
			1 => Self::Loading,
			2 => Self::Warming,
			3 => Self::Active,
			5 => Self::Unavailable,
			6 => Self::Overloaded,
			_ => Self::Fallback,
		}
	}
	pub(crate) fn is_fallback(self) -> bool {
		matches!(self, Self::Fallback | Self::Unavailable | Self::Overloaded)
	}
}
struct Shared {
	cancel: AtomicBool,
	epoch: AtomicU64,
	ready_epoch: AtomicU64,
	status: AtomicU8,
}
impl Shared {
	fn status(&self) -> Status {
		Status::from_code(self.status.load(Ordering::Acquire))
	}
	fn set(&self, value: Status) {
		self.status.store(value as u8, Ordering::Release);
	}
}
struct Lease;
impl Drop for Lease {
	fn drop(&mut self) {
		ACTIVE.store(false, Ordering::Release);
	}
}
struct Process(Child);
impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}
struct Packet {
	kind: u8,
	epoch: u64,
	sequence: u64,
	pcm: Vec<f32>,
}
struct Job {
	packet: Packet,
	captured: Instant,
}

fn read_packet(input: &mut impl Read) -> io::Result<Packet> {
	let mut header = [0u8; 32];
	input.read_exact(&mut header)?;
	if &header[..4] != b"LSR2"
		|| header[4] != 1
		|| header[6..8] != [0, 0]
		|| header[28..] != [0, 0, 0, 0]
	{
		return Err(io::ErrorKind::InvalidData.into());
	}
	let kind = header[5];
	let epoch = u64::from_le_bytes(header[8..16].try_into().unwrap());
	let sequence = u64::from_le_bytes(header[16..24].try_into().unwrap());
	let samples = u32::from_le_bytes(header[24..28].try_into().unwrap()) as usize;
	if !matches!(kind, READY | RESET | PCM | ERROR) || samples != if kind == PCM { HOP } else { 0 }
	{
		return Err(io::ErrorKind::InvalidData.into());
	}
	let mut bytes = vec![0u8; samples * 4];
	input.read_exact(&mut bytes)?;
	let pcm: Vec<_> = bytes
		.as_chunks::<4>()
		.0
		.iter()
		.map(|value| f32::from_le_bytes(*value))
		.collect();
	if pcm.iter().any(|x| !x.is_finite() || x.abs() > 1.0) {
		return Err(io::ErrorKind::InvalidData.into());
	}
	Ok(Packet {
		kind,
		epoch,
		sequence,
		pcm,
	})
}
fn write_packet(output: &mut impl Write, packet: &Packet) -> io::Result<()> {
	if packet.pcm.len() != if packet.kind == PCM { HOP } else { 0 } {
		return Err(io::ErrorKind::InvalidData.into());
	}
	let mut bytes = Vec::with_capacity(32 + packet.pcm.len() * 4);
	bytes.extend_from_slice(b"LSR2");
	bytes.push(1);
	bytes.push(packet.kind);
	bytes.extend_from_slice(&[0, 0]);
	bytes.extend_from_slice(&packet.epoch.to_le_bytes());
	bytes.extend_from_slice(&packet.sequence.to_le_bytes());
	bytes.extend_from_slice(&(packet.pcm.len() as u32).to_le_bytes());
	bytes.extend_from_slice(&[0; 4]);
	for value in &packet.pcm {
		bytes.extend_from_slice(&value.to_le_bytes());
	}
	output.write_all(&bytes)?;
	output.flush()
}

pub struct Controller {
	shared: Arc<Shared>,
	input: SyncSender<Job>,
	output: Receiver<Packet>,
	thread: thread::Thread,
	epoch: u64,
	generation: u64,
	sequence: u64,
	partial: Vec<f32>,
	frames: VecDeque<CapturedFrame>,
	next_frame: Instant,
}
impl Controller {
	pub fn new() -> Result<Self, &'static str> {
		let shared = Arc::new(Shared {
			cancel: AtomicBool::new(false),
			epoch: AtomicU64::new(1),
			ready_epoch: AtomicU64::new(0),
			status: AtomicU8::new(Status::Loading as u8),
		});
		let (input, receive) = mpsc::sync_channel(INPUT_BLOCKS);
		let (output, results) = mpsc::sync_channel(OUTPUT_BLOCKS);
		let service = shared.clone();
		let job = thread::Builder::new()
			.name("voice-speech-supervisor".into())
			.spawn(move || {
				// Preview-to-call handoff must wait for the retiring helper without
				// blocking audio or permanently choosing fallback during a brief race.
				let deadline = Instant::now() + Duration::from_secs(5);
				while !service.cancel.load(Ordering::Acquire) {
					if ACTIVE
						.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
						.is_ok()
					{
						let _lease = Lease;
						let _ = supervise(&service, receive, output);
						break;
					}
					if Instant::now() >= deadline {
						break;
					}
					thread::park_timeout(Duration::from_millis(5));
				}
				if !service.status().is_fallback() {
					service.set(Status::Fallback);
				}
			})
			.map_err(|_| "Speech worker could not start")?;
		Ok(Self {
			shared,
			input,
			output: results,
			thread: job.thread().clone(),
			epoch: 1,
			generation: 0,
			sequence: 0,
			partial: Vec::with_capacity(HOP),
			frames: VecDeque::with_capacity(MAX_OUTPUT_FRAMES),
			next_frame: Instant::now(),
		})
	}
	pub fn status(&self) -> Status {
		self.shared.status()
	}
	pub fn generation(&self) -> u64 {
		self.generation
	}
	pub fn reset(&mut self, generation: u64) {
		self.generation = generation;
		self.epoch = self.epoch.wrapping_add(1).max(1);
		self.shared.epoch.store(self.epoch, Ordering::Release);
		if matches!(self.status(), Status::Active | Status::Warming) {
			self.shared.set(Status::Warming);
		}
		self.partial.fill(0.0);
		self.partial.clear();
		self.frames.clear();
		while self.output.try_recv().is_ok() {}
		self.next_frame = Instant::now();
		self.thread.unpark();
	}
	pub fn push(&mut self, frame: &CapturedFrame) {
		if frame.generation != self.generation {
			self.reset(frame.generation);
		}
		if self.shared.ready_epoch.load(Ordering::Acquire) != self.epoch
			|| !matches!(self.status(), Status::Warming | Status::Active)
		{
			self.partial.clear();
			return;
		}
		self.partial.extend(frame.pcm.iter().map(|x| {
			if x.is_finite() {
				x.clamp(-1.0, 1.0)
			} else {
				0.0
			}
		}));
		if self.partial.len() == HOP {
			self.sequence = self.sequence.wrapping_add(1).max(1);
			let pcm = std::mem::replace(&mut self.partial, Vec::with_capacity(HOP));
			if self
				.input
				.try_send(Job {
					packet: Packet {
						kind: PCM,
						epoch: self.epoch,
						sequence: self.sequence,
						pcm,
					},
					captured: Instant::now(),
				})
				.is_err()
			{
				// A sustained real-time overrun must not silently omit microphone speech.
				self.shared.set(Status::Overloaded);
				self.shared.cancel.store(true, Ordering::Release);
			}
			self.thread.unpark();
		}
	}
	pub fn poll(&mut self) -> Option<CapturedFrame> {
		while let Ok(packet) = self.output.try_recv() {
			if packet.epoch != self.epoch || !matches!(self.status(), Status::Active) {
				continue;
			}
			if self.frames.len() + HOP / 960 > MAX_OUTPUT_FRAMES {
				// Do not silently discard enhanced speech if playback falls behind.
				self.shared.set(Status::Overloaded);
				self.shared.cancel.store(true, Ordering::Release);
				self.thread.unpark();
				break;
			}
			if self.frames.is_empty() {
				self.next_frame = self.next_frame.max(Instant::now());
			}
			for chunk in packet.pcm.as_chunks::<960>().0 {
				self.frames.push_back(CapturedFrame {
					generation: self.generation,
					pcm: *chunk,
				});
			}
		}
		if self.status().is_fallback() {
			self.frames.clear();
		}
		if Instant::now() < self.next_frame {
			return None;
		}
		let frame = self.frames.pop_front()?;
		self.next_frame += Duration::from_millis(20);
		Some(frame)
	}
}
impl Drop for Controller {
	fn drop(&mut self) {
		self.shared.cancel.store(true, Ordering::Release);
		self.thread.unpark();
	}
}

fn supervise(
	shared: &Shared,
	receive: Receiver<Job>,
	output: SyncSender<Packet>,
) -> Result<(), ()> {
	let mut command = Command::new(std::env::current_exe().map_err(|_| ())?);
	command
		.arg("--lavasr-worker")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null());
	#[cfg(target_os = "windows")]
	{
		use std::os::windows::process::CommandExt;
		command.creation_flags(0x0800_0000);
	}
	let mut process = Process(command.spawn().map_err(|_| ())?);
	let mut stdout = process.0.stdout.take().ok_or(())?;
	let mut stdin = process.0.stdin.take().ok_or(())?;
	let (read_send, read_receive) = mpsc::sync_channel(2);
	let (write_send, write_receive) = mpsc::sync_channel::<Packet>(1);
	let supervisor = thread::current();
	let reader = thread::Builder::new()
		.name("voice-speech-reader".into())
		.spawn(move || {
			loop {
				let result = read_packet(&mut stdout);
				let failed = result.is_err();
				let sent = read_send.try_send(result);
				supervisor.unpark();
				if sent.is_err() || failed {
					break;
				}
			}
		})
		.map_err(|_| ())?;
	let writer = thread::Builder::new()
		.name("voice-speech-writer".into())
		.spawn(move || {
			while let Ok(packet) = write_receive.recv() {
				if write_packet(&mut stdin, &packet).is_err() {
					break;
				}
			}
		})
		.map_err(|_| ())?;
	let started = Instant::now();
	let result = (|| {
		let mut initialized = false;
		let mut epoch = 0;
		let mut pending: Option<(u8, u64, u64, Instant)> = None;
		while !shared.cancel.load(Ordering::Acquire) {
			if let Ok(packet) = read_receive.try_recv() {
				let packet = packet.map_err(|_| ())?;
				if packet.kind == ERROR {
					shared.set(Status::Unavailable);
					return Err(());
				}
				if !initialized {
					if packet.kind != READY || packet.epoch != 0 || packet.sequence != 0 {
						return Err(());
					}
					initialized = true;
				} else {
					let (kind, wanted_epoch, sequence, _) = pending.take().ok_or(())?;
					if packet.epoch != wanted_epoch
						|| packet.sequence != sequence
						|| packet.kind != if kind == RESET { READY } else { PCM }
					{
						return Err(());
					}
					if packet.epoch == shared.epoch.load(Ordering::Acquire) {
						if kind == RESET {
							// New-generation input is gated until this acknowledgement;
							// retire old queued blocks before publishing readiness.
							while receive.try_recv().is_ok() {}
							epoch = packet.epoch;
							shared.ready_epoch.store(epoch, Ordering::Release);
							shared.set(Status::Warming);
						} else {
							shared.set(Status::Active);
							if output.try_send(packet).is_err() {
								shared.set(Status::Overloaded);
								return Err(());
							}
						}
					}
				}
			}
			if !initialized && started.elapsed() >= INITIALIZE_TIMEOUT {
				shared.set(Status::Unavailable);
				return Err(());
			}
			if pending.is_some_and(|(_, _, _, at)| at.elapsed() >= PROCESS_TIMEOUT) {
				shared.set(Status::Overloaded);
				return Err(());
			}
			if initialized && pending.is_none() {
				let latest = shared.epoch.load(Ordering::Acquire);
				if epoch != latest {
					write_send
						.try_send(Packet {
							kind: RESET,
							epoch: latest,
							sequence: 0,
							pcm: Vec::new(),
						})
						.map_err(|_| ())?;
					pending = Some((RESET, latest, 0, Instant::now()));
				} else if let Ok(job) = receive.try_recv()
					&& job.packet.epoch == epoch
				{
					if job.captured.elapsed() > MAX_INPUT_AGE {
						shared.set(Status::Overloaded);
						return Err(());
					}
					pending = Some((PCM, epoch, job.packet.sequence, Instant::now()));
					write_send.try_send(job.packet).map_err(|_| ())?;
				}
			}
			if process.0.try_wait().map_err(|_| ())?.is_some() {
				// The reader can be scheduled after the process exits. Give its fixed
				// failure packet a bounded chance to arrive before reporting fallback.
				if let Ok(Ok(packet)) = read_receive.recv_timeout(Duration::from_millis(50))
					&& packet.kind == ERROR
				{
					shared.set(Status::Unavailable);
				}
				return Err(());
			}
			thread::park_timeout(Duration::from_millis(5));
		}
		Ok(())
	})();
	// Killing the child releases both pipe operations even if native inference stopped responding.
	let _ = process.0.kill();
	let _ = process.0.wait();
	drop(write_send);
	let _ = writer.join();
	let _ = reader.join();
	result
}

fn worker() -> Result<(), &'static str> {
	let assets = Assets::packaged()?;
	let mut model = Enhancer::new(&assets)?;
	let mut history = vec![0.0; WINDOW];
	let mut overlap = vec![0.0; OVERLAP];
	// Allocate lazy operator workspaces before accepting microphone PCM. The first
	// inference can exceed a real-time block even when steady-state processing fits.
	let warmup: Vec<_> = (0..WINDOW)
		.map(|i| (i as f32 * 0.13).sin() * 0.05)
		.collect();
	let _ = model.enhance(&warmup)?;
	drop(warmup);
	let mut epoch = 0;
	let mut input = io::stdin().lock();
	let mut output = io::stdout().lock();
	write_packet(
		&mut output,
		&Packet {
			kind: READY,
			epoch,
			sequence: 0,
			pcm: Vec::new(),
		},
	)
	.map_err(|_| "Speech helper output closed")?;
	loop {
		let packet = match read_packet(&mut input) {
			Ok(packet) => packet,
			Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
			Err(_) => return Err("Invalid speech helper request"),
		};
		if packet.kind == RESET {
			epoch = packet.epoch;
			history.fill(0.0);
			overlap.fill(0.0);
			write_packet(
				&mut output,
				&Packet {
					kind: READY,
					epoch,
					sequence: packet.sequence,
					pcm: Vec::new(),
				},
			)
			.map_err(|_| "Speech helper output closed")?;
			continue;
		}
		if packet.kind != PCM || packet.epoch != epoch || packet.sequence == 0 {
			return Err("Invalid speech helper generation");
		}
		history.rotate_left(HOP);
		history[WINDOW - HOP..].copy_from_slice(&packet.pcm);
		let enhanced = if history.iter().all(|x| *x == 0.0) {
			vec![0.0; ENHANCER_WINDOW]
		} else {
			model.enhance(&history)?
		};
		let start = ENHANCER_WINDOW - LOOKAHEAD - HOP - OVERLAP;
		let mut pcm = enhanced[start..start + HOP].to_vec();
		for i in 0..OVERLAP {
			let t = (i + 1) as f32 / (OVERLAP + 1) as f32;
			pcm[i] = overlap[i] * (1.0 - t) + pcm[i] * t;
		}
		overlap.copy_from_slice(&enhanced[start + HOP..start + HOP + OVERLAP]);
		write_packet(
			&mut output,
			&Packet {
				kind: PCM,
				epoch,
				sequence: packet.sequence,
				pcm,
			},
		)
		.map_err(|_| "Speech helper output closed")?;
	}
}

/// Early command modes do not initialize authentication, devices, capture or UI.
pub fn command() -> Option<i32> {
	let args: Vec<_> = std::env::args_os().take(16).collect();
	let helper = args.iter().any(|arg| arg == "--lavasr-worker");
	let test = args.iter().any(|arg| arg == "--self-test-lavasr");
	let realtime = args.iter().any(|arg| arg == "--self-test-lavasr-realtime");
	if !helper && !test && !realtime {
		return None;
	}
	let result = if helper && !test && !realtime {
		let result = worker();
		if result.is_err() {
			// Send a fixed failure label, never model contents or environment data.
			let _ = write_packet(
				&mut io::stdout().lock(),
				&Packet {
					kind: ERROR,
					epoch: 0,
					sequence: 0,
					pcm: Vec::new(),
				},
			);
		}
		result
	} else if (test ^ realtime) && !helper {
		let result = self_test(realtime);
		// A failing command must still let the supervisor kill and reap its child
		// before main exits; Controller::drop itself never blocks the audio worker.
		let deadline = Instant::now() + Duration::from_secs(2);
		while ACTIVE.load(Ordering::Acquire) && Instant::now() < deadline {
			thread::sleep(Duration::from_millis(5));
		}
		result
	} else {
		Err("Choose one speech command")
	};
	if let Err(error) = &result {
		eprintln!("{error}");
	}
	Some(if result.is_ok() { 0 } else { 1 })
}
fn self_test(realtime: bool) -> Result<(), &'static str> {
	let mut controller = Controller::new()?;
	controller.reset(1);
	let deadline = Instant::now() + INITIALIZE_TIMEOUT + Duration::from_secs(2);
	while controller.status() == Status::Loading && Instant::now() < deadline {
		thread::sleep(Duration::from_millis(5));
	}
	if controller.status().is_fallback() || Instant::now() >= deadline {
		return Err("LavaSRv2 model or runtime could not initialize");
	}
	let mut count = 0;
	let mut generation = 1;
	let frames = if realtime { 300 } else { 80 };
	let mut nonzero = false;
	for frame in 0..frames {
		if frame == frames / 2 {
			generation = 2;
			controller.reset(generation);
		}
		// Package validation checks model initialization and IPC/lifecycle, without
		// imposing a real-time inference speed requirement on shared CI machines.
		// The optional real-time probe instead exercises the full model on a tone.
		let pcm: Frame = if realtime {
			std::array::from_fn(|i| ((frame * 960 + i) as f32 * 0.13).sin() * 0.05)
		} else {
			[0.0; 960]
		};
		controller.push(&CapturedFrame { generation, pcm });
		let until = Instant::now() + Duration::from_millis(20);
		while Instant::now() < until {
			if let Some(value) = controller.poll() {
				if value.generation != generation
					|| value
						.pcm
						.iter()
						.any(|x| !x.is_finite() || x.abs() > 1.0 || (!realtime && *x != 0.0))
				{
					return Err("Stale speech helper output");
				}
				nonzero |= value.pcm.iter().any(|x| *x != 0.0);
				count += 1;
			}
			if controller.status().is_fallback() {
				return Err(if controller.status() == Status::Overloaded {
					"LavaSRv2 could not keep up with real-time audio"
				} else {
					"LavaSRv2 packaged processing failed"
				});
			}
			thread::sleep(Duration::from_millis(2));
		}
	}
	drop(controller);
	let deadline = Instant::now() + Duration::from_secs(2);
	while ACTIVE.load(Ordering::Acquire) && Instant::now() < deadline {
		thread::sleep(Duration::from_millis(5));
	}
	let minimum_frames = if realtime { frames - 24 } else { 20 };
	if ACTIVE.load(Ordering::Acquire) || count < minimum_frames || (realtime && !nonzero) {
		return Err("LavaSRv2 packaged lifecycle failed");
	}
	println!(
		"LavaSRv2 packaged initialization, processing, reset and teardown passed; denoising enabled; {count}/{frames} synthetic {} frames; no devices opened",
		if realtime { "nonzero tone" } else { "silence" },
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn output_pacing_does_not_accumulate_scheduler_lateness() {
		let (mut controller, _input, output) = controller();
		let due = Instant::now() - Duration::from_millis(5);
		controller.next_frame = due;
		controller.frames.push_back(CapturedFrame {
			generation: 1,
			pcm: [0.1; 960],
		});
		assert!(controller.poll().is_some());
		assert_eq!(controller.next_frame, due + Duration::from_millis(20));
		controller.next_frame = Instant::now() - Duration::from_secs(1);
		output
			.send(Packet {
				kind: PCM,
				epoch: 7,
				sequence: 1,
				pcm: vec![0.1; HOP],
			})
			.unwrap();
		let before = Instant::now();
		assert!(controller.poll().is_some());
		assert!(controller.next_frame >= before + Duration::from_millis(20));
	}
	#[test]
	fn canceling_a_waiting_handoff_does_not_release_the_retiring_helpers_lease() {
		assert!(
			ACTIVE
				.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
				.is_ok()
		);
		let _lease = Lease;
		let controller = Controller::new().unwrap();
		let shared = controller.shared.clone();
		assert_eq!(controller.status(), Status::Loading);
		drop(controller);
		let deadline = Instant::now() + Duration::from_secs(1);
		while shared.status() != Status::Fallback && Instant::now() < deadline {
			thread::sleep(Duration::from_millis(2));
		}
		assert_eq!(shared.status(), Status::Fallback);
		assert!(
			ACTIVE.load(Ordering::Acquire),
			"a canceled waiter cannot unlock another helper"
		);
	}
	fn controller() -> (Controller, Receiver<Job>, SyncSender<Packet>) {
		let (input, receive) = mpsc::sync_channel(INPUT_BLOCKS);
		let (output, results) = mpsc::sync_channel(1);
		let controller = Controller {
			shared: Arc::new(Shared {
				cancel: AtomicBool::new(false),
				epoch: AtomicU64::new(7),
				ready_epoch: AtomicU64::new(7),
				status: AtomicU8::new(Status::Active as u8),
			}),
			input,
			output: results,
			thread: thread::current(),
			epoch: 7,
			generation: 1,
			sequence: 0,
			partial: Vec::with_capacity(HOP),
			frames: VecDeque::new(),
			next_frame: Instant::now(),
		};
		(controller, receive, output)
	}
	#[test]
	fn reset_waits_for_acknowledgement_before_queuing_fresh_audio() {
		let (mut controller, input, _output) = controller();
		for _ in 0..INPUT_BLOCKS * HOP / 960 {
			controller.push(&CapturedFrame {
				generation: 1,
				pcm: [0.25; 960],
			});
		}
		controller.reset(2);
		// A late status publication must not let fresh PCM compete with old jobs.
		controller.shared.set(Status::Active);
		for _ in 0..6 {
			controller.push(&CapturedFrame {
				generation: 2,
				pcm: [0.125; 960],
			});
		}
		assert_eq!(controller.status(), Status::Active);
		let retired: Vec<_> = input.try_iter().collect();
		assert_eq!(retired.len(), INPUT_BLOCKS);
		assert!(retired.iter().all(|job| job.packet.epoch == 7));
		assert!(controller.partial.is_empty());
		controller.shared.ready_epoch.store(8, Ordering::Release);
		for _ in 0..HOP / 960 {
			controller.push(&CapturedFrame {
				generation: 2,
				pcm: [0.125; 960],
			});
		}
		let fresh = input.try_recv().unwrap();
		assert_eq!(fresh.packet.epoch, 8);
		assert_eq!(fresh.packet.pcm, vec![0.125; HOP]);
	}
	#[test]
	fn capture_reset_retires_partial_queued_and_late_helper_audio() {
		let (mut controller, _input, output) = controller();
		controller.push(&CapturedFrame {
			generation: 1,
			pcm: [0.25; 960],
		});
		output
			.send(Packet {
				kind: PCM,
				epoch: 7,
				sequence: 1,
				pcm: vec![0.25; HOP],
			})
			.unwrap();
		assert!(controller.poll().is_some());
		assert!(!controller.frames.is_empty());
		controller.reset(2);
		assert!(controller.frames.is_empty() && controller.partial.is_empty());
		output
			.send(Packet {
				kind: PCM,
				epoch: 7,
				sequence: 2,
				pcm: vec![0.25; HOP],
			})
			.unwrap();
		controller.shared.set(Status::Active);
		assert!(
			controller.poll().is_none(),
			"in-flight old response must not regain the new generation"
		);
		output
			.send(Packet {
				kind: PCM,
				epoch: 8,
				sequence: 3,
				pcm: vec![0.125; HOP],
			})
			.unwrap();
		let fresh = controller.poll().unwrap();
		assert_eq!(fresh.generation, 2);
		assert_eq!(fresh.pcm, [0.125; 960]);
	}
	#[test]
	fn a_full_helper_queue_selects_fallback_instead_of_discarding_speech() {
		let (mut controller, input, _output) = controller();
		for _ in 0..(INPUT_BLOCKS + 1) * HOP / 960 {
			controller.push(&CapturedFrame {
				generation: 1,
				pcm: [0.1; 960],
			});
		}
		assert_eq!(controller.status(), Status::Overloaded);
		assert!(controller.shared.cancel.load(Ordering::Acquire));
		assert_eq!(input.try_iter().count(), INPUT_BLOCKS);
		assert!(controller.poll().is_none());
	}
	#[test]
	fn eighty_millisecond_blocks_survive_short_input_and_output_bursts_in_order() {
		let (mut controller, input, output) = controller();
		for i in 0..INPUT_BLOCKS * HOP / 960 {
			controller.push(&CapturedFrame {
				generation: 1,
				pcm: [i as f32 / 20.0; 960],
			});
		}
		assert_eq!(controller.status(), Status::Active);
		let jobs: Vec<_> = input.try_iter().collect();
		assert_eq!(jobs.len(), INPUT_BLOCKS);
		for (block, job) in jobs.iter().enumerate() {
			for (frame, chunk) in job.packet.pcm.as_chunks::<960>().0.iter().enumerate() {
				assert_eq!(*chunk, [(block * HOP / 960 + frame) as f32 / 20.0; 960]);
			}
		}
		for (block, job) in jobs.into_iter().enumerate() {
			output.send(job.packet).unwrap();
			for chunk in 0..HOP / 960 {
				controller.next_frame = Instant::now();
				let frame = controller.poll().unwrap();
				assert_eq!(frame.pcm, [(block * HOP / 960 + chunk) as f32 / 20.0; 960]);
			}
		}
		assert_eq!(controller.status(), Status::Active);
	}
	#[test]
	fn falling_behind_output_capacity_is_explicit() {
		let (mut controller, _input, output) = controller();
		for _ in 0..MAX_OUTPUT_FRAMES {
			controller.frames.push_back(CapturedFrame {
				generation: 1,
				pcm: [0.25; 960],
			});
		}
		output
			.send(Packet {
				kind: PCM,
				epoch: 7,
				sequence: 1,
				pcm: vec![0.1; HOP],
			})
			.unwrap();
		assert!(controller.poll().is_none());
		assert_eq!(controller.status(), Status::Overloaded);
		assert!(controller.shared.cancel.load(Ordering::Acquire));
	}
	#[test]
	fn speech_protocol_rejects_oversized_truncated_and_nonfinite_pcm() {
		let value = Packet {
			kind: PCM,
			epoch: 7,
			sequence: 9,
			pcm: vec![0.125; HOP],
		};
		let mut bytes = Vec::new();
		write_packet(&mut bytes, &value).unwrap();
		let read = read_packet(&mut &bytes[..]).unwrap();
		assert_eq!((read.epoch, read.sequence, read.pcm.len()), (7, 9, HOP));
		assert_eq!(read.pcm, value.pcm);
		assert!(read_packet(&mut &bytes[..bytes.len() - 1]).is_err());
		let mut oversized = bytes.clone();
		oversized[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
		assert!(read_packet(&mut &oversized[..]).is_err());
		bytes[32..36].copy_from_slice(&f32::NAN.to_le_bytes());
		assert!(read_packet(&mut &bytes[..]).is_err());
		let mut failure = Vec::new();
		write_packet(
			&mut failure,
			&Packet {
				kind: ERROR,
				epoch: 0,
				sequence: 0,
				pcm: Vec::new(),
			},
		)
		.unwrap();
		assert_eq!(read_packet(&mut &failure[..]).unwrap().kind, ERROR);
	}
}
