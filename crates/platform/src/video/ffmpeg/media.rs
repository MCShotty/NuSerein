//! Worker-owned bounded attachment demux/decode lifecycle.
use super::{
	Adapter, Frame, MediaInfo, MemoryLease, NonNull, PhantomData, Picture, Rc, Reader, Sample,
	VideoAdapter, c_void, decode_result, decoder_bytes, media_read, media_seek, serein_media_audio,
	serein_media_close, serein_media_open, serein_media_poll, serein_media_seek,
	serein_media_video,
};

/// MOV/Matroska H264/HEVC/AV1 decoding from the application's anonymous reader.
/// No native URL, filename, protocol or HTTP client is exposed to FFmpeg.
pub struct File {
	native: NonNull<c_void>,
	_reader: Box<Reader>,
	_memory: MemoryLease,
	info: crate::video::Info,
	restarted: bool,
	_thread: PhantomData<Rc<()>>,
}

impl Drop for File {
	fn drop(&mut self) {
		// SAFETY: Close all AVIO/codec references before dropping the reader Box.
		unsafe { serein_media_close(self.native.as_ptr()) };
	}
}

impl File {
	pub fn open(
		mut source: Box<dyn crate::video::ReadSeek>,
		adapter: Option<VideoAdapter>,
	) -> Result<Self, (Box<dyn crate::video::ReadSeek>, &'static str)> {
		use std::io::SeekFrom;
		let length = match source.seek(SeekFrom::End(0)).and_then(|length| {
			source.seek(SeekFrom::Start(0))?;
			Ok(length)
		}) {
			Ok(length) if (1..=100 * 1024 * 1024).contains(&length) => length,
			_ => return Err((source, crate::video::INVALID)),
		};
		let mut reader = Box::new(Reader { source, length });
		let mut info = MediaInfo::default();
		let adapter = adapter.map(Adapter::from);
		// SAFETY: Reader Box has a stable address and remains owned until native close.
		let native = unsafe {
			serein_media_open(
				(&mut *reader as *mut Reader).cast(),
				media_read,
				media_seek,
				adapter
					.as_ref()
					.map_or(std::ptr::null(), std::ptr::from_ref),
				&mut info,
			)
		};
		let Some(native) = NonNull::new(native) else {
			return Err((reader.source, crate::video::UNSUPPORTED));
		};
		let mut memory = MemoryLease::default();
		let result = Picture::sdr(info.width, info.height)
			.validate()
			.and_then(|()| {
				if !info.duration.is_finite()
					|| !(0.0..=crate::video::MAX_SECONDS).contains(&info.duration)
					|| info.depth > 10
					|| info.references > 16
				{
					return Err(crate::video::INVALID);
				}
				let pictures = decoder_bytes(
					info.width as usize,
					info.height as usize,
					info.depth,
					1,
					info.references,
				)?;
				// Both compressed track queues share 8 MiB; include their fixed bookkeeping.
				memory.resize(pictures + 8 * 1024 * 1024 + 2 * 512 * 2 * size_of::<usize>())
			});
		if let Err(error) = result {
			// SAFETY: open returned an owned live context; no sample has been read.
			unsafe { serein_media_close(native.as_ptr()) };
			return Err((reader.source, error));
		}
		let (width, height) = if matches!(info.rotation, 90 | 270) {
			(info.height, info.width)
		} else {
			(info.width, info.height)
		};
		Ok(Self {
			native,
			_reader: reader,
			_memory: memory,
			restarted: false,
			info: crate::video::Info {
				width,
				height,
				duration: info.duration,
				sample_rate: info.sample_rate,
				channels: if info.sample_rate > 0 { 2 } else { 0 },
			},
			_thread: PhantomData,
		})
	}
	pub fn info(&self) -> crate::video::Info {
		self.info
	}
	/// Retire presentation/audio buffers after a first-frame hardware fallback.
	pub fn take_restart(&mut self) -> bool {
		std::mem::take(&mut self.restarted)
	}
	pub fn seek(&mut self, seconds: f64) -> Result<(), &'static str> {
		if !seconds.is_finite() || seconds < 0.0 || seconds > self.info.duration {
			return Err(crate::video::INVALID);
		}
		// SAFETY: Seek flushes this exclusively owned decoder and both bounded tracks.
		decode_result(unsafe { serein_media_seek(self.native.as_ptr(), seconds) }).map(|_| ())
	}
	fn poll(&mut self, audio: bool) -> Result<std::task::Poll<bool>, &'static str> {
		// SAFETY: Track polling is confined to this File's worker.
		match unsafe { serein_media_poll(self.native.as_ptr(), i32::from(audio)) } {
			0 => Ok(std::task::Poll::Pending),
			1 => Ok(std::task::Poll::Ready(true)),
			2 => Ok(std::task::Poll::Ready(false)),
			3 => {
				self.restarted = true;
				Ok(std::task::Poll::Pending)
			}
			code => {
				decode_result(code)?;
				Err(crate::video::INVALID)
			}
		}
	}
	pub fn poll_video(&mut self) -> Result<std::task::Poll<Option<Sample>>, &'static str> {
		use std::task::Poll;
		match self.poll(false)? {
			Poll::Pending => return Ok(Poll::Pending),
			Poll::Ready(false) => return Ok(Poll::Ready(None)),
			Poll::Ready(true) => {}
		}
		let mut picture = Picture::default();
		let mut pts = 0.0;
		// SAFETY: Inspect one retained bounded native picture before allocating output.
		decode_result(unsafe {
			serein_media_video(
				self.native.as_ptr(),
				&mut picture,
				std::ptr::null_mut(),
				0,
				&mut pts,
			)
		})?;
		picture.validate()?;
		let mut memory = MemoryLease::default();
		memory.resize(picture.bytes)?;
		let mut bytes = Vec::new();
		bytes
			.try_reserve_exact(picture.bytes)
			.map_err(|_| crate::video::INVALID)?;
		bytes.resize(picture.bytes, 0);
		let capacity = bytes.len();
		// SAFETY: The output allocation matches the inspected picture exactly.
		decode_result(unsafe {
			serein_media_video(
				self.native.as_ptr(),
				&mut picture,
				bytes.as_mut_ptr(),
				capacity,
				&mut pts,
			)
		})?;
		Ok(Poll::Ready(Some(Sample::Picture {
			pts,
			frame: Frame {
				picture,
				data: bytes,
				_memory: memory,
			},
		})))
	}
	pub fn poll_audio(&mut self) -> Result<std::task::Poll<Option<Sample>>, &'static str> {
		use std::task::Poll;
		match self.poll(true)? {
			Poll::Pending => return Ok(Poll::Pending),
			Poll::Ready(false) => return Ok(Poll::Ready(None)),
			Poll::Ready(true) => {}
		}
		let mut frames = vec![[0.0f32; 2]; 65536];
		let mut written = 0;
		let mut pts = 0.0;
		// SAFETY: C writes at most this fixed stereo PCM allocation and reports its length.
		decode_result(unsafe {
			serein_media_audio(
				self.native.as_ptr(),
				frames.as_mut_ptr().cast(),
				frames.len(),
				&mut written,
				&mut pts,
			)
		})?;
		if written > frames.len() || !pts.is_finite() {
			return Err(crate::video::INVALID);
		}
		frames.truncate(written);
		Ok(Poll::Ready(Some(Sample::Audio { pts, frames })))
	}
}
