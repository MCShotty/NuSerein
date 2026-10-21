//! Decoder-driven attachment ranges. No file cache or eager whole-video download.
use std::{
	collections::VecDeque,
	io::{self, Read, Seek, SeekFrom},
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};
use tokio::runtime::Handle;

const MAX_BYTES: usize = 100 * 1024 * 1024;
const CHUNK: usize = 256 * 1024;
const CACHE_CHUNKS: usize = 8;
const INVALID: &str = "Video download failed or changed; reload the conversation";
const UNSUPPORTED: &str = "Video server does not support buffering; download to play externally";
const WEB_PAGE: &str = "This embed links to a web page; open the original to watch it";
fn html_bytes(bytes: &[u8]) -> bool {
	let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
	bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'<')
}
fn web_page(headers: &reqwest::header::HeaderMap) -> bool {
	headers
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|value| {
			let value = value.split(';').next().unwrap_or(value).trim();
			value.eq_ignore_ascii_case("text/html")
				|| value.eq_ignore_ascii_case("application/xhtml+xml")
		})
}
pub(super) const STARTUP_TIMEOUT: &str = "Video took too long to start; retry or open the original";
pub(super) const STALL_TIMEOUT: &str = "Video buffering stalled; retry or open the original";
#[derive(Clone)]
pub(super) struct Startup {
	deadline: std::time::Instant,
	pub first_picture: Arc<AtomicBool>,
	progress: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}
impl Startup {
	pub fn new(deadline: std::time::Instant) -> Self {
		Self {
			deadline,
			first_picture: Arc::new(AtomicBool::new(false)),
			progress: Default::default(),
		}
	}
	pub fn progressed(&self) {
		if let Ok(mut progress) = self.progress.lock() {
			*progress = Some(std::time::Instant::now());
		}
	}
	pub fn expired(&self) -> bool {
		let now = std::time::Instant::now();
		if !self.first_picture.load(Ordering::Acquire) {
			return now >= self.deadline;
		}
		self.progress
			.lock()
			.map(|last| last.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(15)))
			.unwrap_or(true)
	}
	pub fn timeout_error(&self) -> &'static str {
		if self.first_picture.load(Ordering::Acquire) {
			STALL_TIMEOUT
		} else {
			STARTUP_TIMEOUT
		}
	}
}

async fn interruption(cancelled: &AtomicBool, startup: Option<&Startup>) -> &'static str {
	loop {
		if cancelled.load(Ordering::Acquire) {
			return INVALID;
		}
		if let Some(startup) = startup
			&& startup.expired()
		{
			return startup.timeout_error();
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
}

fn embed_redirect_allowed(url: &url::Url, previous: usize) -> bool {
	previous <= 3
		&& ui::embedded_video_url(&model::EmbedMedia {
			url: Some(url.as_str().into()),
			..Default::default()
		})
		.is_some()
}

pub(super) fn source(
	url: Option<url::Url>,
	expected: usize,
	cancelled: Arc<AtomicBool>,
	runtime: Handle,
) -> Result<Box<dyn platform::video::ReadSeek>, &'static str> {
	source_until(url, expected, cancelled, runtime, None)
}
pub(super) fn source_until(
	url: Option<url::Url>,
	expected: usize,
	cancelled: Arc<AtomicBool>,
	runtime: Handle,
	startup: Option<Startup>,
) -> Result<Box<dyn platform::video::ReadSeek>, &'static str> {
	if let Some(startup) = &startup
		&& startup.expired()
	{
		return Err(startup.timeout_error());
	}
	if expected > MAX_BYTES {
		return Err("Video preview limit: 100 MiB");
	}
	let mut cache = VecDeque::new();
	let (input, len) = if let Some(url) = url {
		// The caller has validated the service URL; never let URL userinfo add credentials.
		if !url.username().is_empty() || url.password().is_some() {
			return Err(INVALID);
		}
		let client = reqwest::Client::builder()
			.user_agent(concat!(
				"Serein/",
				env!("CARGO_PKG_VERSION"),
				" (+https://github.com/ViceVerse-cz/Serein)"
			))
			.no_proxy()
			.no_gzip()
			.no_brotli()
			.no_deflate()
			.no_zstd()
			.redirect(if expected == 0 {
				reqwest::redirect::Policy::custom(|attempt| {
					if embed_redirect_allowed(attempt.url(), attempt.previous().len()) {
						attempt.follow()
					} else {
						attempt.stop()
					}
				})
			} else {
				reqwest::redirect::Policy::none()
			})
			.timeout(Duration::from_secs(15))
			.build()
			.map_err(|_| INVALID)?;
		let len = if expected == 0 {
			let (len, whole) = probe(&client, &url, &cancelled, &runtime, startup.as_ref())?;
			if let Some(bytes) = whole {
				cache.push_back((0, bytes));
			}
			len
		} else {
			expected
		};
		if len == 0 || len > MAX_BYTES {
			return Err("Video preview limit: 100 MiB");
		}
		(
			Input::Http {
				client,
				url,
				runtime,
			},
			len,
		)
	} else {
		#[cfg(not(feature = "demo"))]
		return Err(INVALID);
		#[cfg(feature = "demo")]
		{
			let bytes = include_bytes!("../../tests/fixtures/video.mov");
			if bytes.is_empty() || bytes.len() > MAX_BYTES {
				return Err(INVALID);
			}
			(Input::Demo(bytes), bytes.len())
		}
	};
	Ok(Box::new(Source {
		input,
		cancelled,
		len,
		position: 0,
		cache,
		startup,
	}))
}

/// Probe an unknown length. A small server that ignores Range may return its whole file;
/// retain that one bounded chunk instead of rejecting a valid clip or downloading it twice.
fn probe(
	client: &reqwest::Client,
	url: &url::Url,
	cancelled: &AtomicBool,
	runtime: &Handle,
	startup: Option<&Startup>,
) -> Result<(usize, Option<Vec<u8>>), &'static str> {
	runtime.block_on(async {
		let cancelled = interruption(cancelled, startup);
		let transfer = async {
			let mut response = client
				.get(url.clone())
				.header(reqwest::header::ACCEPT_ENCODING, "identity")
				.header(reqwest::header::RANGE, "bytes=0-0")
				.send()
				.await
				.map_err(|_| INVALID)?;
			if web_page(response.headers()) {
				return Err(WEB_PAGE);
			}
			if response
				.headers()
				.get(reqwest::header::CONTENT_ENCODING)
				.is_some_and(|value| value != "identity")
			{
				return Err(INVALID);
			}
			if response.status() == reqwest::StatusCode::OK {
				if response
					.headers()
					.contains_key(reqwest::header::CONTENT_RANGE)
				{
					return Err(INVALID);
				}
				let expected = response.content_length();
				if expected.is_some_and(|len| len > CHUNK as u64) {
					return Err(UNSUPPORTED);
				}
				let mut bytes = Vec::with_capacity(expected.map_or(CHUNK, |len| len as usize));
				while let Some(chunk) = response.chunk().await.map_err(|_| INVALID)? {
					if chunk.len() > CHUNK - bytes.len() {
						return Err(UNSUPPORTED);
					}
					bytes.extend_from_slice(&chunk);
				}
				if html_bytes(&bytes) {
					return Err(WEB_PAGE);
				}
				if bytes.is_empty() || expected.is_some_and(|len| len != bytes.len() as u64) {
					return Err(INVALID);
				}
				return Ok((bytes.len(), Some(bytes)));
			}
			if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
				return Err(UNSUPPORTED);
			}
			response
				.headers()
				.get(reqwest::header::CONTENT_RANGE)
				.and_then(|value| value.to_str().ok())
				.and_then(|value| value.strip_prefix("bytes 0-0/"))
				.and_then(|total| total.parse::<usize>().ok())
				.map(|len| (len, None))
				.ok_or(INVALID)
		};
		tokio::select! { biased;
			error = cancelled => Err(error),
			result = transfer => result,
		}
	})
}

enum Input {
	Http {
		client: reqwest::Client,
		url: url::Url,
		runtime: Handle,
	},
	#[cfg(feature = "demo")]
	Demo(&'static [u8]),
}
struct Source {
	input: Input,
	cancelled: Arc<AtomicBool>,
	len: usize,
	position: usize,
	cache: VecDeque<(usize, Vec<u8>)>,
	startup: Option<Startup>,
}
fn invalid() -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, INVALID)
}
impl Source {
	fn check_cancelled(&self) -> io::Result<()> {
		if let Some(startup) = &self.startup
			&& startup.expired()
		{
			return Err(io::Error::new(
				io::ErrorKind::TimedOut,
				startup.timeout_error(),
			));
		}
		if self.cancelled.load(Ordering::Acquire) {
			// Read::read_exact retries Interrupted indefinitely.
			Err(io::Error::new(
				io::ErrorKind::ConnectionAborted,
				"Cancelled",
			))
		} else {
			Ok(())
		}
	}
	fn fill(&mut self) -> io::Result<()> {
		// Retain both tracks across demuxer seeks. Evict before transfer so encoded
		// payloads, including the new range, stay within eight chunks / 2 MiB.
		if self.cache.len() == CACHE_CHUNKS {
			self.cache.pop_front();
		}
		let (client, url, runtime) = match &self.input {
			Input::Http {
				client,
				url,
				runtime,
			} => (client, url, runtime),
			#[cfg(feature = "demo")]
			Input::Demo(_) => return Err(invalid()),
		};
		let start = self.position / CHUNK * CHUNK;
		let count = CHUNK.min(self.len - start);
		let end = start + count - 1;
		let bytes = runtime.block_on(async {
			let cancelled = interruption(&self.cancelled, self.startup.as_ref());
			let transfer = async {
				let mut response = client
					.get(url.clone())
					.header(reqwest::header::ACCEPT_ENCODING, "identity")
					.header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
					.send()
					.await
					.map_err(|_| invalid())?;
				if web_page(response.headers()) {
					return Err(io::Error::new(io::ErrorKind::Unsupported, WEB_PAGE));
				}
				if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
					let range = format!("bytes {start}-{end}/{}", self.len);
					if response
						.headers()
						.get(reqwest::header::CONTENT_RANGE)
						.and_then(|value| value.to_str().ok())
						!= Some(range.as_str())
					{
						return Err(invalid());
					}
				} else if !(response.status() == reqwest::StatusCode::OK
					&& start == 0 && count == self.len)
				{
					return Err(io::Error::new(io::ErrorKind::Unsupported, UNSUPPORTED));
				}
				if response
					.headers()
					.get(reqwest::header::CONTENT_ENCODING)
					.is_some_and(|value| value != "identity")
					|| response
						.content_length()
						.is_some_and(|len| len != count as u64)
				{
					return Err(invalid());
				}
				let mut bytes = Vec::with_capacity(count);
				while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
					if chunk.len() > count - bytes.len() {
						return Err(invalid());
					}
					bytes.extend_from_slice(&chunk);
				}
				if bytes.len() != count {
					return Err(invalid());
				}
				Ok(bytes)
			};
			tokio::select! { biased;
				error = cancelled => Err(io::Error::new(io::ErrorKind::ConnectionAborted, error)),
				result = transfer => result,
			}
		})?;
		self.check_cancelled()?;
		if start == 0 && html_bytes(&bytes) {
			return Err(io::Error::new(io::ErrorKind::Unsupported, WEB_PAGE));
		}
		self.cache.push_back((start, bytes));
		Ok(())
	}
}
impl Read for Source {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		self.check_cancelled()?;
		let mut count = output.len().min(CHUNK).min(self.len - self.position);
		if count == 0 {
			return Ok(0);
		}
		#[cfg(feature = "demo")]
		if let Input::Demo(bytes) = self.input {
			output[..count].copy_from_slice(&bytes[self.position..self.position + count]);
			self.position += count;
			return Ok(count);
		}
		if let Some(index) = self.cache.iter().position(|(start, bytes)| {
			self.position >= *start && self.position < start + bytes.len()
		}) {
			let range = self.cache.remove(index).ok_or_else(invalid)?;
			self.cache.push_back(range);
		} else {
			self.fill()?;
		}
		let (start, bytes) = self.cache.back().ok_or_else(invalid)?;
		let offset = self.position - start;
		count = count.min(bytes.len() - offset);
		output[..count].copy_from_slice(&bytes[offset..offset + count]);
		self.position += count;
		Ok(count)
	}
}
impl Seek for Source {
	fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
		self.check_cancelled()?;
		let position = match position {
			SeekFrom::Start(position) => Some(position),
			SeekFrom::Current(offset) => (self.position as u64).checked_add_signed(offset),
			SeekFrom::End(offset) => (self.len as u64).checked_add_signed(offset),
		}
		.filter(|position| *position <= self.len as u64)
		.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid video seek"))?;
		self.position = position as usize;
		Ok(position)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{io::Write, net::TcpListener, sync::atomic::AtomicUsize};

	#[test]
	fn presentation_progress_and_recovery_share_the_stall_deadline() {
		let runtime = tokio::runtime::Runtime::new().unwrap();
		let startup = Startup::new(std::time::Instant::now() + Duration::from_secs(30));
		startup.first_picture.store(true, Ordering::Release);
		*startup.progress.lock().unwrap() =
			Some(std::time::Instant::now() - Duration::from_secs(16));
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		listener.set_nonblocking(true).unwrap();
		let url =
			url::Url::parse(&format!("http://{}/clip", listener.local_addr().unwrap())).unwrap();
		let open = |limits| {
			source_until(
				Some(url.clone()),
				0,
				Arc::new(AtomicBool::new(false)),
				runtime.handle().clone(),
				Some(limits),
			)
		};
		assert_eq!(open(startup.clone()).err(), Some(STALL_TIMEOUT));
		assert!(
			listener.accept().is_err(),
			"a retry must not reset the last presentation deadline"
		);
		startup.progressed();
		assert!(!startup.expired());
	}

	#[test]
	fn startup_deadline_interrupts_a_body_and_html_never_reaches_the_decoder() {
		for html in [false, true] {
			let runtime = tokio::runtime::Runtime::new().unwrap();
			let listener = TcpListener::bind("127.0.0.1:0").unwrap();
			listener.set_nonblocking(true).unwrap();
			let url = url::Url::parse(&format!("http://{}/clip", listener.local_addr().unwrap()))
				.unwrap();
			let finished = Arc::new(AtomicBool::new(false));
			let done = finished.clone();
			let server = std::thread::spawn(move || {
				let deadline = std::time::Instant::now() + Duration::from_secs(5);
				let mut socket = loop {
					match listener.accept() {
						Ok((s, _)) => break s,
						Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
							if done.load(Ordering::Acquire) {
								return;
							}
							assert!(std::time::Instant::now() < deadline);
							std::thread::sleep(Duration::from_millis(5));
						}
						Err(e) => panic!("{e}"),
					}
				};
				socket.set_nonblocking(false).unwrap();
				socket
					.set_read_timeout(Some(Duration::from_secs(5)))
					.unwrap();
				let mut headers = Vec::new();
				while !headers.ends_with(b"\r\n\r\n") {
					let mut b = [0];
					socket.read_exact(&mut b).unwrap();
					headers.push(b[0]);
					assert!(headers.len() < 8192);
				}
				write!(
					socket,
					"HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: 128\r\nConnection: close\r\n\r\n",
					if html { "text/html" } else { "video/mp4" }
				)
				.unwrap();
				// Hold the declared body open until timeout or MIME rejection closes the reader.
				let _ = socket.read(&mut [0]);
			});
			let result = source_until(
				Some(url),
				0,
				Arc::new(AtomicBool::new(false)),
				runtime.handle().clone(),
				Some(Startup::new(
					std::time::Instant::now()
						+ if html {
							Duration::from_secs(5)
						} else {
							Duration::from_millis(200)
						},
				)),
			);
			finished.store(true, Ordering::Release);
			server.join().unwrap();
			assert_eq!(
				result.err(),
				Some(if html { WEB_PAGE } else { STARTUP_TIMEOUT })
			);
		}
	}

	#[test]
	fn embed_probe_requires_a_bounded_range_length() {
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		for (status, range, valid) in [
			("206 Partial Content", "bytes 0-0/8".to_owned(), true),
			("206 Partial Content", "bytes 0-0/0".to_owned(), false),
			(
				"206 Partial Content",
				format!("bytes 0-0/{}", MAX_BYTES + 1),
				false,
			),
			("206 Partial Content", "bytes 1-1/8".to_owned(), false),
			("206 Partial Content", "bytes 0-0/*".to_owned(), false),
			("200 OK", "bytes 0-0/8".to_owned(), false),
		] {
			let listener = TcpListener::bind("127.0.0.1:0").unwrap();
			listener.set_nonblocking(true).unwrap();
			let url = url::Url::parse(&format!(
				"http://{}/embedded.mp4",
				listener.local_addr().unwrap()
			))
			.unwrap();
			let server = std::thread::spawn(move || {
				// Covers client initialization plus the bounded 15-second request under parallel load.
				let deadline = std::time::Instant::now() + Duration::from_secs(20);
				let mut socket = loop {
					match listener.accept() {
						Ok((socket, _)) => break socket,
						Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
							assert!(
								std::time::Instant::now() < deadline,
								"probe did not connect"
							);
							std::thread::sleep(Duration::from_millis(5));
						}
						Err(error) => panic!("{error}"),
					}
				};
				// Winsock inherits the listener's nonblocking mode; headers use blocking reads.
				socket.set_nonblocking(false).unwrap();
				socket
					.set_read_timeout(Some(Duration::from_secs(20)))
					.unwrap();
				let mut header = Vec::new();
				while !header.ends_with(b"\r\n\r\n") {
					assert!(header.len() < 8192);
					let mut byte = [0];
					socket.read_exact(&mut byte).unwrap();
					header.push(byte[0]);
				}
				let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
				assert!(header.contains("range: bytes=0-0\r\n"));
				assert!(!header.contains("authorization:") && !header.contains("cookie:"));
				write!(socket, "HTTP/1.1 {status}\r\nContent-Range: {range}\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx").unwrap();
			});
			let result = source(
				Some(url),
				0,
				Arc::new(AtomicBool::new(false)),
				runtime.handle().clone(),
			);
			assert_eq!(result.is_ok(), valid);
			if let Ok(mut source) = result {
				assert_eq!(source.seek(SeekFrom::End(0)).unwrap(), 8);
			}
			server.join().unwrap();
		}
	}

	#[test]
	fn unknown_size_embeds_decode_with_ranges_or_one_bounded_whole_response() {
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		for (ranges, length_header) in [(true, true), (false, true), (false, false)] {
			let listener = TcpListener::bind("127.0.0.1:0").unwrap();
			listener.set_nonblocking(true).unwrap();
			let url = url::Url::parse(&format!(
				"http://{}/offload/synthetic/1?sig=synthetic",
				listener.local_addr().unwrap()
			))
			.unwrap();
			let server = std::thread::spawn(move || {
				let movie = include_bytes!("../../tests/fixtures/video.mov");
				assert!(movie.len() < CHUNK);
				let deadline = std::time::Instant::now() + Duration::from_secs(10);
				for request in 0..if ranges { 2 } else { 1 } {
					let mut socket = loop {
						match listener.accept() {
							Ok((socket, _)) => break socket,
							Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
								assert!(
									std::time::Instant::now() < deadline,
									"missing video request"
								);
								std::thread::sleep(Duration::from_millis(5));
							}
							Err(error) => panic!("{error}"),
						}
					};
					socket.set_nonblocking(false).unwrap();
					socket
						.set_read_timeout(Some(Duration::from_secs(5)))
						.unwrap();
					let mut header = Vec::new();
					while !header.ends_with(b"\r\n\r\n") {
						assert!(header.len() < 8192);
						let mut byte = [0];
						socket.read_exact(&mut byte).unwrap();
						header.push(byte[0]);
					}
					let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
					assert!(header.starts_with("get /offload/synthetic/1?sig=synthetic "));
					assert!(!header.contains("authorization:") && !header.contains("cookie:"));
					let end = if ranges && request == 0 {
						0
					} else {
						movie.len() - 1
					};
					if ranges {
						assert!(header.contains(&format!("range: bytes=0-{end}\r\n")));
						write!(
							socket,
							"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{end}/{}\r\n",
							movie.len()
						)
						.unwrap();
					} else {
						assert!(header.contains("range: bytes=0-0\r\n"));
						write!(socket, "HTTP/1.1 200 OK\r\n").unwrap();
					}
					if length_header {
						write!(socket, "Content-Length: {}\r\n", end + 1).unwrap();
					}
					write!(
						socket,
						"Content-Type: video/mp4\r\nConnection: close\r\n\r\n"
					)
					.unwrap();
					socket.write_all(&movie[..=end]).unwrap();
				}
			});
			let mut input = source(
				Some(url),
				0,
				Arc::new(AtomicBool::new(false)),
				runtime.handle().clone(),
			)
			.unwrap();
			assert_eq!(
				input.seek(SeekFrom::End(0)).unwrap(),
				include_bytes!("../../tests/fixtures/video.mov").len() as u64
			);
			input.seek(SeekFrom::Start(0)).unwrap();
			let mut decoder =
				platform::video::ffmpeg::AttachmentDecoder::open(input, None).unwrap();
			let deadline = std::time::Instant::now() + Duration::from_secs(10);
			loop {
				if matches!(
					decoder.poll_video().unwrap(),
					std::task::Poll::Ready(Some(_))
				) {
					break;
				}
				let _ = decoder.poll_audio().unwrap();
				assert!(std::time::Instant::now() < deadline, "no decoded picture");
				std::thread::sleep(Duration::from_millis(2));
			}
			server.join().unwrap();
		}
	}

	#[test]
	fn whole_embed_probe_rejects_excess_bytes_and_content_encoding() {
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		for (headers, body) in [
			(format!("Content-Length: {}\r\n", CHUNK + 1), Vec::new()),
			(
				"Content-Encoding: gzip\r\nContent-Length: 1\r\n".into(),
				vec![0],
			),
			("Content-Length: 0\r\n".into(), Vec::new()),
			("".into(), vec![0; CHUNK + 1]),
		] {
			let listener = TcpListener::bind("127.0.0.1:0").unwrap();
			let url = url::Url::parse(&format!("http://{}/video", listener.local_addr().unwrap()))
				.unwrap();
			let server = std::thread::spawn(move || {
				let (mut socket, _) = listener.accept().unwrap();
				socket
					.set_read_timeout(Some(Duration::from_secs(5)))
					.unwrap();
				let mut header = Vec::new();
				while !header.ends_with(b"\r\n\r\n") {
					assert!(header.len() < 8192);
					let mut byte = [0];
					socket.read_exact(&mut byte).unwrap();
					header.push(byte[0]);
				}
				write!(
					socket,
					"HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
				)
				.unwrap();
				let _ = socket.write_all(&body); // A rejected response can close before its body.
			});
			assert!(
				source(
					Some(url),
					0,
					Arc::new(AtomicBool::new(false)),
					runtime.handle().clone()
				)
				.is_err()
			);
			server.join().unwrap();
		}
	}

	#[test]
	fn alternating_tracks_reuse_buffered_ranges() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let address = listener.local_addr().unwrap();
		let url = url::Url::parse(&format!("http://{address}/tracks.mov")).unwrap();
		let expected = 8 * 1024 * 1024;
		let requests = Arc::new(AtomicUsize::new(0));
		let observed = requests.clone();
		let stopped = Arc::new(AtomicBool::new(false));
		let stop = stopped.clone();
		let server = std::thread::spawn(move || {
			loop {
				let (mut socket, _) = listener.accept().unwrap();
				if stop.load(Ordering::Acquire) {
					break;
				}
				socket
					.set_read_timeout(Some(Duration::from_secs(2)))
					.unwrap();
				let mut header = Vec::new();
				while !header.ends_with(b"\r\n\r\n") {
					assert!(header.len() < 8192);
					let mut byte = [0];
					socket.read_exact(&mut byte).unwrap();
					header.push(byte[0]);
				}
				let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
				let range = header
					.lines()
					.find_map(|line| line.strip_prefix("range: bytes="))
					.unwrap();
				let (start, end) = range.split_once('-').unwrap();
				let start = start.parse::<usize>().unwrap();
				let end = end.parse::<usize>().unwrap();
				let count = end - start + 1;
				assert!(count <= 256 * 1024 && end < expected);
				observed.fetch_add(1, Ordering::Release);
				std::thread::sleep(Duration::from_millis(10));
				write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{expected}\r\nContent-Length: {count}\r\nConnection: close\r\n\r\n").unwrap();
				socket
					.write_all(&vec![(start / (4 * 1024 * 1024)) as u8; count])
					.unwrap();
			}
		});
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		let mut input = source(
			Some(url),
			expected,
			Arc::new(AtomicBool::new(false)),
			runtime.handle().clone(),
		)
		.unwrap();
		assert_eq!(requests.load(Ordering::Acquire), 0);
		let started = std::time::Instant::now();
		for sample in 0..32 {
			for track in 0..2 {
				input
					.seek(SeekFrom::Start(
						(track * 4 * 1024 * 1024 + sample * 4096) as u64,
					))
					.unwrap();
				let mut bytes = [0; 4096];
				input.read_exact(&mut bytes).unwrap();
				assert!(bytes.iter().all(|byte| *byte == track as u8));
			}
		}
		let elapsed = started.elapsed();
		let count = requests.load(Ordering::Acquire);
		stopped.store(true, Ordering::Release);
		std::net::TcpStream::connect(address).unwrap();
		server.join().unwrap();
		eprintln!(
			"alternating tracks: {count} requests, {elapsed:?}, 64 reads, 10 ms response delay"
		);
		assert_eq!(count, 2);
	}

	#[test]
	fn cancellation_interrupts_a_stalled_range_body() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let url = url::Url::parse(&format!(
			"http://{}/stalled.mov",
			listener.local_addr().unwrap()
		))
		.unwrap();
		let cancelled = Arc::new(AtomicBool::new(false));
		let stop = cancelled.clone();
		let server = std::thread::spawn(move || {
			let (mut socket, _) = listener.accept().unwrap();
			socket
				.set_read_timeout(Some(Duration::from_secs(2)))
				.unwrap();
			let mut header = Vec::new();
			while !header.ends_with(b"\r\n\r\n") {
				assert!(header.len() < 8192);
				let mut byte = [0];
				socket.read_exact(&mut byte).unwrap();
				header.push(byte[0]);
			}
			write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{CHUNK}\r\nContent-Length: {CHUNK}\r\n\r\n", CHUNK - 1).unwrap();
			std::thread::sleep(Duration::from_millis(40));
			stop.store(true, Ordering::Release);
			// Keep the response open until cancellation drops the connection.
			match socket.read(&mut [0]) {
				Ok(0) => {}
				Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
				other => panic!("cancelled range connection stayed open: {other:?}"),
			}
		});
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		let mut input = source(Some(url), CHUNK, cancelled, runtime.handle().clone()).unwrap();
		assert_eq!(
			input.read(&mut [0]).unwrap_err().kind(),
			io::ErrorKind::ConnectionAborted
		);
		server.join().unwrap();
	}
	#[test]
	fn cancellation_interrupts_a_stalled_whole_embed_probe() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let url = url::Url::parse(&format!(
			"http://{}/stalled",
			listener.local_addr().unwrap()
		))
		.unwrap();
		let cancelled = Arc::new(AtomicBool::new(false));
		let stop = cancelled.clone();
		let server = std::thread::spawn(move || {
			let (mut socket, _) = listener.accept().unwrap();
			socket
				.set_read_timeout(Some(Duration::from_secs(5)))
				.unwrap();
			let mut header = Vec::new();
			while !header.ends_with(b"\r\n\r\n") {
				assert!(header.len() < 8192);
				let mut byte = [0];
				socket.read_exact(&mut byte).unwrap();
				header.push(byte[0]);
			}
			write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {CHUNK}\r\n\r\n").unwrap();
			std::thread::sleep(Duration::from_millis(40));
			stop.store(true, Ordering::Release);
			match socket.read(&mut [0]) {
				Ok(0) => {}
				Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
				other => panic!("probe connection stayed open: {other:?}"),
			}
		});
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		assert!(source(Some(url), 0, cancelled, runtime.handle().clone()).is_err());
		server.join().unwrap();
	}

	#[test]
	fn ranges_follow_reads_and_seeks_and_reject_changed_responses() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let url = url::Url::parse(&format!(
			"http://{}/synthetic.mov",
			listener.local_addr().unwrap()
		))
		.unwrap();
		let expected = CHUNK * (CACHE_CHUNKS + 1);
		let requests = Arc::new(AtomicUsize::new(0));
		let observed = requests.clone();
		let server = std::thread::spawn(move || {
			let starts = [0, CHUNK * CACHE_CHUNKS]
				.into_iter()
				.chain((1..CACHE_CHUNKS).map(|chunk| chunk * CHUNK))
				.chain([CHUNK * CACHE_CHUNKS]);
			for (index, start) in starts.enumerate() {
				let (mut socket, _) = listener.accept().unwrap();
				socket
					.set_read_timeout(Some(Duration::from_secs(2)))
					.unwrap();
				let mut header = Vec::new();
				while !header.ends_with(b"\r\n\r\n") {
					assert!(header.len() < 8192);
					let mut byte = [0];
					socket.read_exact(&mut byte).unwrap();
					header.push(byte[0]);
				}
				let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
				assert!(
					header.contains(&format!("range: bytes={start}-{}\r\n", start + CHUNK - 1))
				);
				assert!(!header.contains("authorization:"));
				assert!(!header.contains("cookie:"));
				observed.fetch_add(1, Ordering::Release);
				let changed = index == CACHE_CHUNKS + 1;
				let total = expected + usize::from(changed);
				write!(socket, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{total}\r\nContent-Length: {CHUNK}\r\nConnection: close\r\n\r\n", start + CHUNK - 1).unwrap();
				let result = socket.write_all(&vec![(start / CHUNK) as u8; CHUNK]);
				if !changed {
					result.unwrap();
				}
			}
		});
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		let cancelled = Arc::new(AtomicBool::new(false));
		let mut input = source(
			Some(url.clone()),
			expected,
			cancelled.clone(),
			runtime.handle().clone(),
		)
		.unwrap();
		assert_eq!(requests.load(Ordering::Acquire), 0);
		let mut bytes = [1; 32];
		input.read_exact(&mut bytes).unwrap();
		assert_eq!(bytes, [0; 32]);
		assert_eq!(requests.load(Ordering::Acquire), 1);
		input.seek(SeekFrom::Start(8)).unwrap();
		input.read_exact(&mut bytes).unwrap();
		assert_eq!(requests.load(Ordering::Acquire), 1);
		input.seek(SeekFrom::End(-32)).unwrap();
		input.read_exact(&mut bytes).unwrap();
		assert_eq!(bytes, [CACHE_CHUNKS as u8; 32]);
		assert_eq!(requests.load(Ordering::Acquire), 2);
		assert_eq!(input.read(&mut bytes).unwrap(), 0);
		assert!(input.seek(SeekFrom::End(1)).is_err());
		assert!(input.seek(SeekFrom::Start(u64::MAX)).is_err());
		assert!(input.seek(SeekFrom::Current(i64::MIN)).is_err());
		for chunk in 1..CACHE_CHUNKS {
			// Keep the first range hot while loading enough others to evict the tail.
			input.seek(SeekFrom::Start(0)).unwrap();
			input.read_exact(&mut bytes).unwrap();
			assert_eq!(bytes, [0; 32]);
			input.seek(SeekFrom::Start((chunk * CHUNK) as u64)).unwrap();
			input.read_exact(&mut bytes).unwrap();
			assert_eq!(bytes, [chunk as u8; 32]);
		}
		assert_eq!(requests.load(Ordering::Acquire), CACHE_CHUNKS + 1);
		input.seek(SeekFrom::Start(0)).unwrap();
		input.read_exact(&mut bytes).unwrap();
		assert_eq!(bytes, [0; 32]);
		input.seek(SeekFrom::End(-32)).unwrap();
		assert_eq!(
			input.read(&mut bytes).unwrap_err().kind(),
			io::ErrorKind::InvalidData
		);
		assert_eq!(requests.load(Ordering::Acquire), CACHE_CHUNKS + 2);
		server.join().unwrap();
		cancelled.store(true, Ordering::Release);
		assert_eq!(
			input.read(&mut bytes).unwrap_err().kind(),
			io::ErrorKind::ConnectionAborted
		);
		assert!(
			source(
				Some(url),
				MAX_BYTES + 1,
				cancelled,
				runtime.handle().clone()
			)
			.is_err()
		);
	}
	#[test]
	fn embed_redirects_are_bounded_and_cannot_reach_arbitrary_origins() {
		let approved =
			url::Url::parse("https://video.xx.fbcdn.net/video/clip.mp4?sig=synthetic").unwrap();
		assert!(embed_redirect_allowed(&approved, 1));
		assert!(embed_redirect_allowed(&approved, 3));
		assert!(!embed_redirect_allowed(&approved, 4));
		for raw in [
			"https://example.org/clip.mp4",
			"https://127.0.0.1/clip.mp4",
			"https://video.xx.fbcdn.net.evil.test/clip.mp4",
			"https://user@video.xx.fbcdn.net/video/clip.mp4",
			"http://video.xx.fbcdn.net/video/clip.mp4",
		] {
			assert!(!embed_redirect_allowed(&url::Url::parse(raw).unwrap(), 1));
		}
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.enable_all()
			.build()
			.unwrap();
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let destination = TcpListener::bind("127.0.0.1:0").unwrap();
		destination.set_nonblocking(true).unwrap();
		let url =
			url::Url::parse(&format!("http://{}/clip", listener.local_addr().unwrap())).unwrap();
		let location = format!("http://{}/private", destination.local_addr().unwrap());
		let server = std::thread::spawn(move || {
			let (mut socket, _) = listener.accept().unwrap();
			socket
				.set_read_timeout(Some(Duration::from_secs(20)))
				.unwrap();
			let mut header = Vec::new();
			while !header.ends_with(b"\r\n\r\n") {
				assert!(header.len() < 8192);
				let mut byte = [0];
				socket.read_exact(&mut byte).unwrap();
				header.push(byte[0]);
			}
			write!(
				socket,
				"HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
			)
			.unwrap();
		});
		assert!(
			source(
				Some(url),
				0,
				Arc::new(AtomicBool::new(false)),
				runtime.handle().clone()
			)
			.is_err()
		);
		server.join().unwrap();
		assert_eq!(
			destination.accept().unwrap_err().kind(),
			io::ErrorKind::WouldBlock
		);
	}
}
