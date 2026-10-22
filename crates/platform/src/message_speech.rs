//! Explicit local message reading. One active engine and one replaceable pending utterance.
use std::{
	sync::{Arc, Condvar, Mutex},
	thread::JoinHandle,
	time::Duration,
};

type Result<T> = std::result::Result<T, &'static str>;
const UNAVAILABLE: &str =
	"Message speech is unavailable. Install or enable a system speech engine.";
const FAILED: &str = "The system speech engine could not read this message.";
trait Engine: Send {
	fn finished(&mut self) -> Result<bool>;
}
struct Pending {
	generation: u64,
	text: Option<String>,
}
#[derive(Default)]
struct Shared {
	generation: u64,
	pending: Option<Pending>,
	result: Option<Result<()>>,
	shutdown: bool,
}
type Signal = Arc<(Mutex<Shared>, Condvar)>;

pub struct Speaker {
	shared: Signal,
	worker: Option<JoinHandle<()>>,
}
impl Speaker {
	pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Result<Self> {
		Self::with_engine(wake, start)
	}
	fn with_engine(
		wake: impl Fn() + Send + Sync + 'static,
		start: impl Fn(String) -> Result<Box<dyn Engine>> + Send + 'static,
	) -> Result<Self> {
		let shared: Signal = Arc::default();
		let signal = shared.clone();
		let worker = std::thread::Builder::new()
			.name("message-speech".into())
			.spawn(move || {
				let mut active: Option<(u64, Box<dyn Engine>)> = None;
				loop {
					let (lock, notify) = &*signal;
					let mut state = lock
						.lock()
						.unwrap_or_else(std::sync::PoisonError::into_inner);
					if state.shutdown {
						break;
					}
					let pending = state.pending.take();
					if pending.is_none() && active.is_none() {
						drop(
							notify
								.wait(state)
								.unwrap_or_else(std::sync::PoisonError::into_inner),
						);
						continue;
					}
					drop(state);
					if let Some(pending) = pending {
						active = None; // Drop cancels and retires the previous engine first.
						if let Some(text) = pending.text {
							match start(text) {
								Ok(engine) => active = Some((pending.generation, engine)),
								Err(error) => {
									publish(&signal, pending.generation, Err(error), &wake)
								}
							}
						} else {
							publish(&signal, pending.generation, Ok(()), &wake);
						}
					}
					if let Some((generation, engine)) = &mut active {
						let generation = *generation;
						let current = signal
							.0
							.lock()
							.unwrap_or_else(std::sync::PoisonError::into_inner)
							.generation;
						if generation != current {
							active = None;
							continue;
						}
						match engine.finished() {
							Ok(false) => {}
							result => {
								active = None;
								publish(&signal, generation, result.map(|_| ()), &wake);
							}
						}
					}
					let state = lock
						.lock()
						.unwrap_or_else(std::sync::PoisonError::into_inner);
					if !state.shutdown && state.pending.is_none() {
						drop(
							notify
								.wait_timeout(state, Duration::from_millis(40))
								.unwrap_or_else(std::sync::PoisonError::into_inner),
						);
					}
				}
			})
			.map_err(|_| UNAVAILABLE)?;
		Ok(Self {
			shared,
			worker: Some(worker),
		})
	}
	pub fn replace(&self, text: Option<String>) -> Result<()> {
		if text.as_ref().is_some_and(|text| {
			text.len() > 64 * 1024 || text.trim().is_empty() || text.contains('\0')
		}) {
			return Err(FAILED);
		}
		let mut state = self
			.shared
			.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		state.generation = state.generation.wrapping_add(1);
		state.result = None;
		state.pending = Some(Pending {
			generation: state.generation,
			text,
		});
		self.shared.1.notify_one();
		Ok(())
	}
	pub fn poll(&self) -> Option<Result<()>> {
		self.shared
			.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.result
			.take()
	}
}
fn publish(signal: &Signal, generation: u64, result: Result<()>, wake: &impl Fn()) {
	let mut state = signal
		.0
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner);
	if !state.shutdown && generation == state.generation {
		state.result = Some(result);
		drop(state);
		wake();
	}
}
impl Drop for Speaker {
	fn drop(&mut self) {
		{
			let mut state = self
				.shared
				.0
				.lock()
				.unwrap_or_else(std::sync::PoisonError::into_inner);
			state.shutdown = true;
			state.pending = None;
			self.shared.1.notify_one();
		}
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct Process {
	child: std::process::Child,
	input: Option<JoinHandle<std::io::Result<()>>>,
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Engine for Process {
	fn finished(&mut self) -> Result<bool> {
		let Some(status) = self.child.try_wait().map_err(|_| FAILED)? else {
			return Ok(false);
		};
		let written = self
			.input
			.take()
			.is_none_or(|thread| thread.join().is_ok_and(|result| result.is_ok()));
		if status.success() && written {
			Ok(true)
		} else {
			Err(FAILED)
		}
	}
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
		if let Some(input) = self.input.take() {
			let _ = input.join();
		}
	}
}
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn start(text: String) -> Result<Box<dyn Engine>> {
	use std::{
		io::Write,
		process::{Command, Stdio},
	};
	#[cfg(target_os = "windows")]
	let mut command = {
		use std::os::windows::process::CommandExt;
		let mut command = Command::new("powershell.exe");
		// Fixed script: the message is stdin data and can never become PowerShell code.
		command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); $text = [Console]::In.ReadToEnd(); $voice = New-Object -ComObject SAPI.SpVoice; [void]$voice.Speak($text, 16)"]).creation_flags(0x08000000);
		command
	};
	#[cfg(target_os = "macos")]
	let mut command = Command::new("/usr/bin/say");
	let mut child = command
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.map_err(|_| UNAVAILABLE)?;
	let mut stdin = child.stdin.take().ok_or(FAILED)?;
	#[cfg(target_os = "macos")]
	let text = text.replace("[[", "[ [");
	let input = std::thread::Builder::new()
		.name("message-speech-input".into())
		.spawn(move || stdin.write_all(text.as_bytes()));
	match input {
		Ok(input) => Ok(Box::new(Process {
			child,
			input: Some(input),
		})),
		Err(_) => {
			let _ = child.kill();
			let _ = child.wait();
			Err(FAILED)
		}
	}
}

#[cfg(target_os = "linux")]
mod linux {
	use super::*;
	use std::{
		io::{Read, Write},
		os::unix::net::UnixStream,
		path::PathBuf,
		time::Instant,
	};
	pub(super) struct Speech {
		stream: UnixStream,
		buffer: Vec<u8>,
		done: bool,
		lines: usize,
	}
	pub(super) fn connect_socket(path: &std::path::Path, deadline: Instant) -> Result<UnixStream> {
		// A local daemon with a full accept queue can block a synchronous connect forever.
		// Keep connection setup inside the same deadline as the SSIP handshake.
		let runtime = tokio::runtime::Builder::new_current_thread()
			.enable_io()
			.enable_time()
			.build()
			.map_err(|_| UNAVAILABLE)?;
		let stream = runtime.block_on(async {
			tokio::time::timeout_at(
				tokio::time::Instant::from_std(deadline),
				tokio::net::UnixStream::connect(path),
			)
			.await
			.map_err(|_| UNAVAILABLE)?
			.map_err(|_| UNAVAILABLE)
		})?;
		let stream = stream.into_std().map_err(|_| FAILED)?;
		stream.set_nonblocking(false).map_err(|_| FAILED)?;
		Ok(stream)
	}
	impl Speech {
		pub fn connect(path: &std::path::Path, text: &str) -> Result<Self> {
			let deadline = Instant::now() + Duration::from_secs(2);
			let stream = connect_socket(path, deadline)?;
			stream
				.set_read_timeout(Some(Duration::from_millis(40)))
				.map_err(|_| FAILED)?;
			stream
				.set_write_timeout(Some(Duration::from_millis(250)))
				.map_err(|_| FAILED)?;
			let mut speech = Self {
				stream,
				buffer: Vec::new(),
				done: false,
				lines: 0,
			};
			for command in [
				"SET SELF CLIENT_NAME serein:message:reader\r\n",
				"SET SELF SSML_MODE off\r\n",
				"SET SELF NOTIFICATION END on\r\n",
				"SET SELF NOTIFICATION CANCEL on\r\n",
				"SPEAK\r\n",
			] {
				speech
					.stream
					.write_all(command.as_bytes())
					.map_err(|_| FAILED)?;
				speech.reply(deadline)?;
			}
			let mut payload = String::with_capacity(text.len() + 1024);
			for line in text.replace('\r', "").split('\n') {
				let line = line.trim_end_matches('\r');
				if line.starts_with('.') {
					payload.push('.');
				}
				payload.push_str(line);
				payload.push_str("\r\n");
			}
			payload.push_str(".\r\n");
			speech
				.stream
				.write_all(payload.as_bytes())
				.map_err(|_| FAILED)?;
			speech.reply(deadline)?;
			Ok(speech)
		}
		fn code(&mut self) -> Result<Option<u16>> {
			loop {
				if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
					if !(4..=8192).contains(&end) || self.lines >= 64 {
						return Err(FAILED);
					}
					self.lines += 1;
					let code = std::str::from_utf8(&self.buffer[..3])
						.ok()
						.and_then(|s| s.parse::<u16>().ok())
						.filter(|code| (200..800).contains(code))
						.ok_or(FAILED)?;
					let separator = self.buffer[3];
					self.buffer.drain(..=end);
					if separator == b' ' {
						self.lines = 0;
						return Ok(Some(code));
					}
					if separator != b'-' {
						return Err(FAILED);
					}
					continue;
				}
				let mut buffer = [0; 4096];
				match self.stream.read(&mut buffer) {
					Ok(0) => return Err(FAILED),
					Ok(count) if self.buffer.len() + count <= 8192 => {
						self.buffer.extend_from_slice(&buffer[..count])
					}
					Ok(_) => return Err(FAILED),
					Err(error)
						if matches!(
							error.kind(),
							std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
						) =>
					{
						return Ok(None);
					}
					Err(_) => return Err(FAILED),
				}
			}
		}
		fn reply(&mut self, deadline: Instant) -> Result<()> {
			while Instant::now() < deadline {
				match self.code()? {
					Some(200..=299) => return Ok(()),
					Some(702 | 703) => self.done = true,
					Some(_) => return Err(FAILED),
					None => {}
				}
			}
			Err(FAILED)
		}
	}
	impl Engine for Speech {
		fn finished(&mut self) -> Result<bool> {
			if self.done {
				return Ok(true);
			}
			match self.code()? {
				Some(702 | 703) => self.done = true,
				Some(_) => return Err(FAILED),
				None => {}
			}
			Ok(self.done)
		}
	}
	impl Drop for Speech {
		fn drop(&mut self) {
			let _ = self.stream.write_all(b"CANCEL SELF\r\nQUIT\r\n");
		}
	}
	pub(super) fn start(text: String) -> Result<Box<dyn Engine>> {
		let path = if let Some(address) = std::env::var_os("SPEECHD_ADDRESS") {
			let address = address.to_str().ok_or(UNAVAILABLE)?;
			PathBuf::from(address.strip_prefix("unix_socket:").ok_or(UNAVAILABLE)?)
		} else if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
			PathBuf::from(runtime).join("speech-dispatcher/speechd.sock")
		} else {
			return Err(UNAVAILABLE);
		};
		Ok(Box::new(Speech::connect(&path, &text)?))
	}
}
#[cfg(target_os = "linux")]
use linux::start;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn start(_: String) -> Result<Box<dyn Engine>> {
	Err(UNAVAILABLE)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};
	struct Held(Arc<AtomicUsize>);
	impl Engine for Held {
		fn finished(&mut self) -> Result<bool> {
			Ok(false)
		}
	}
	impl Drop for Held {
		fn drop(&mut self) {
			self.0.fetch_sub(1, Ordering::SeqCst);
		}
	}
	#[test]
	fn replacement_stop_and_drop_retire_the_single_active_reader() {
		let alive = Arc::new(AtomicUsize::new(0));
		let observed = alive.clone();
		let (started, reads) = std::sync::mpsc::sync_channel(8);
		let speaker = Speaker::with_engine(
			|| {},
			move |text| {
				assert_eq!(
					observed.fetch_add(1, Ordering::SeqCst),
					0,
					"previous engine retired first"
				);
				started.send(text).unwrap();
				Ok(Box::new(Held(observed.clone())))
			},
		)
		.unwrap();
		speaker.replace(Some("first".into())).unwrap();
		assert_eq!(reads.recv_timeout(Duration::from_secs(2)).unwrap(), "first");
		speaker.replace(Some("second".into())).unwrap();
		assert_eq!(
			reads.recv_timeout(Duration::from_secs(2)).unwrap(),
			"second"
		);
		speaker.replace(None).unwrap();
		let deadline = std::time::Instant::now() + Duration::from_secs(2);
		while alive.load(Ordering::SeqCst) != 0 && std::time::Instant::now() < deadline {
			std::thread::yield_now();
		}
		assert_eq!(alive.load(Ordering::SeqCst), 0);
		speaker.replace(Some("third".into())).unwrap();
		reads.recv_timeout(Duration::from_secs(2)).unwrap();
		drop(speaker);
		assert_eq!(alive.load(Ordering::SeqCst), 0);
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn speech_daemon_full_accept_queue_cannot_block_reader_teardown() {
		use std::time::Instant;
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("speech.sock");
		let _listener = {
			let runtime = tokio::runtime::Builder::new_current_thread()
				.enable_io()
				.build()
				.unwrap();
			let _entered = runtime.enter();
			let socket = tokio::net::UnixSocket::new_stream().unwrap();
			socket.bind(&path).unwrap();
			socket.listen(1).unwrap().into_std().unwrap()
		};
		let mut connections = Vec::new();
		let mut saturated = false;
		for _ in 0..8 {
			match linux::connect_socket(&path, Instant::now() + Duration::from_millis(25)) {
				Ok(stream) => connections.push(stream),
				Err(_) => {
					saturated = true;
					break;
				}
			}
		}
		assert!(
			saturated,
			"exercise a local daemon that stops accepting connections"
		);
		let start = Instant::now();
		assert!(linux::Speech::connect(&path, "synthetic text").is_err());
		assert!(start.elapsed() < Duration::from_secs(3));
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn speech_dispatcher_escapes_data_and_cancels_only_its_own_connection() {
		use std::{
			io::{BufRead, BufReader, Write},
			os::unix::net::UnixListener,
		};
		for completed in [true, false] {
			let directory = tempfile::tempdir().unwrap();
			let path = directory.path().join("speech.sock");
			let listener = UnixListener::bind(&path).unwrap();
			let server = std::thread::spawn(move || {
				let (mut socket, _) = listener.accept().unwrap();
				socket
					.set_read_timeout(Some(Duration::from_secs(3)))
					.unwrap();
				let mut reader = BufReader::new(socket.try_clone().unwrap());
				for expected in [
					"SET SELF CLIENT_NAME serein:message:reader",
					"SET SELF SSML_MODE off",
					"SET SELF NOTIFICATION END on",
					"SET SELF NOTIFICATION CANCEL on",
					"SPEAK",
				] {
					let mut line = String::new();
					reader.read_line(&mut line).unwrap();
					assert_eq!(line.trim_end(), expected);
					socket.write_all(b"200 OK\r\n").unwrap();
				}
				let mut lines = Vec::new();
				loop {
					let mut line = String::new();
					reader.read_line(&mut line).unwrap();
					if line == ".\r\n" {
						break;
					}
					lines.push(line);
					assert!(lines.len() < 10);
				}
				assert_eq!(
					lines,
					["..\r\n", "CANCEL ALL\r\n", "visible\r\n"],
					"message text remains data"
				);
				socket
					.write_all(b"225-42\r\n225 OK MESSAGE QUEUED\r\n")
					.unwrap();
				if completed {
					socket.write_all(b"702-42\r\n702-1\r\n702 END\r\n").unwrap();
				}
				let mut line = String::new();
				reader.read_line(&mut line).unwrap();
				assert_eq!(line, "CANCEL SELF\r\n");
				line.clear();
				reader.read_line(&mut line).unwrap();
				assert_eq!(line, "QUIT\r\n");
			});
			let mut engine = linux::Speech::connect(&path, ".\nCANCEL ALL\nvisible").unwrap();
			assert_eq!(engine.finished().unwrap(), completed);
			drop(engine);
			server.join().unwrap();
		}
	}
}
