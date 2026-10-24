//! Worker-owned bounded packet audio decoder.
use super::{
	MemoryLease, NonNull, PhantomData, Rc, c_void, serein_audio_close, serein_audio_decode,
	serein_audio_open,
};

/// Bounded mono/stereo decoding of packets from the application's audio demuxer.
/// Absence of a compiled codec permits fallback; corrupt data does not.
pub struct AudioDecoder {
	native: NonNull<c_void>,
	buffer: Vec<f32>,
	_memory: MemoryLease,
	_thread: PhantomData<Rc<()>>,
}

impl Drop for AudioDecoder {
	fn drop(&mut self) {
		// SAFETY: This worker owns the native decoder exclusively.
		unsafe { serein_audio_close(self.native.as_ptr()) };
	}
}

impl AudioDecoder {
	pub fn open(
		name: &std::ffi::CStr,
		rate: u32,
		channels: usize,
		extra: &[u8],
	) -> Result<Option<Self>, &'static str> {
		if !(8000..=96000).contains(&rate)
			|| !(1..=2).contains(&channels)
			|| extra.len() > 65536 + 64
		{
			return Err(crate::video::INVALID);
		}
		let mut native = std::ptr::null_mut();
		let mut memory = MemoryLease::default();
		memory.resize(2 * 1024 * 1024)?;
		// SAFETY: Native open copies the bounded header and retains no Rust pointers.
		match unsafe {
			serein_audio_open(
				name.as_ptr(),
				rate,
				channels as u32,
				extra.as_ptr(),
				extra.len(),
				&mut native,
			)
		} {
			0 => Ok(None),
			1 => Ok(Some(Self {
				native: NonNull::new(native).ok_or(crate::video::INVALID)?,
				buffer: vec![0.0; 65536 * channels],
				_memory: memory,
				_thread: PhantomData,
			})),
			_ => Err(crate::video::INVALID),
		}
	}
	/// Drain each packet completely before accepting another. None flushes at EOF.
	pub fn decode(&mut self, packet: Option<&[u8]>) -> Result<&[f32], &'static str> {
		if packet.is_some_and(|bytes| bytes.is_empty() || bytes.len() > 1024 * 1024) {
			return Err(crate::video::INVALID);
		}
		let mut written = 0;
		// SAFETY: C copies the packet and writes at most this fixed PCM capacity.
		let result = unsafe {
			serein_audio_decode(
				self.native.as_ptr(),
				packet.map_or(std::ptr::null(), |bytes| bytes.as_ptr()),
				packet.map_or(0, |bytes| bytes.len()),
				self.buffer.as_mut_ptr(),
				self.buffer.len(),
				&mut written,
			)
		};
		if result != 1 || written > self.buffer.len() {
			return Err(crate::video::INVALID);
		}
		Ok(&self.buffer[..written])
	}
}
