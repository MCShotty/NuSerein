//! Worker-owned live video decoder and admission.
use super::{
	Adapter, Header, MAX_UNIT, MemoryLease, NonNull, PhantomData, Picture, Rc, VideoAdapter,
	c_void, decode_result, decoder_bytes, serein_decode_close, serein_decode_hardware,
	serein_decode_header, serein_decode_open, serein_decode_receive, serein_decode_send,
};

/// Owns one native codec exclusively on the media worker that opened it.
pub struct LiveDecoder {
	native: NonNull<c_void>,
	codec: u32,
	lease: MemoryLease,
	_thread: PhantomData<Rc<()>>,
}

impl Drop for LiveDecoder {
	fn drop(&mut self) {
		// SAFETY: This sole owner closes the context on its owning worker.
		unsafe { serein_decode_close(self.native.as_ptr()) };
	}
}

impl LiveDecoder {
	pub fn new(
		codec: u32,
		adapter: Option<VideoAdapter>,
		hardware: bool,
	) -> Result<Self, &'static str> {
		if codec > 2 || hardware && adapter.is_none() {
			return Err(crate::video::UNSUPPORTED);
		}
		let adapter = adapter.map(Adapter::from);
		// SAFETY: The adapter is read synchronously and never retained by C.
		let native = unsafe {
			serein_decode_open(
				codec as i32,
				adapter
					.as_ref()
					.map_or(std::ptr::null(), std::ptr::from_ref),
				i32::from(hardware),
			)
		};
		Ok(Self {
			native: NonNull::new(native).ok_or(crate::video::UNSUPPORTED)?,
			codec,
			lease: MemoryLease::default(),
			_thread: PhantomData,
		})
	}
	pub fn codec(&self) -> u32 {
		self.codec
	}
	pub fn inspect(&mut self, data: &[u8]) -> Result<Option<Header>, &'static str> {
		if data.is_empty() || data.len() > MAX_UNIT {
			return Err(crate::video::INVALID);
		}
		let mut header = Header::default();
		// SAFETY: C parses a copied, padded access unit and writes this fixed ABI record.
		let parsed = decode_result(unsafe {
			serein_decode_header(self.native.as_ptr(), data.as_ptr(), data.len(), &mut header)
		})?;
		Ok(parsed.then_some(header))
	}
	pub fn hardware(&self) -> bool {
		// SAFETY: Context remains exclusively owned and live.
		unsafe { serein_decode_hardware(self.native.as_ptr()) != 0 }
	}
	pub fn reserve(
		&mut self,
		width: usize,
		height: usize,
		depth: u32,
		chroma: u32,
		references: u32,
	) -> Result<(), &'static str> {
		let bytes = decoder_bytes(width, height, depth, chroma, references)?;
		self.lease.resize(bytes.max(self.lease.0))
	}
	pub fn send(&mut self, data: &[u8], pts: i64) -> Result<bool, &'static str> {
		if data.is_empty() || data.len() > MAX_UNIT {
			return Err(crate::video::INVALID);
		}
		// SAFETY: C copies the bounded access unit before returning.
		decode_result(unsafe {
			serein_decode_send(self.native.as_ptr(), data.as_ptr(), data.len(), pts)
		})
	}
	pub fn receive(&mut self, output: &mut Vec<u8>) -> Result<Option<Picture>, &'static str> {
		let mut picture = Picture::default();
		// SAFETY: Inspection retains the native frame and writes only this ABI record.
		if !decode_result(unsafe {
			serein_decode_receive(self.native.as_ptr(), &mut picture, std::ptr::null_mut(), 0)
		})? {
			return Ok(None);
		}
		picture.validate()?;
		if self.lease.0 == 0 {
			// Callers with parsed sequence headers reserve before send. This guard
			// also covers anonymous file streams whose dimensions are container supplied.
			return Err("Video decoding requires a surface reservation");
		}
		if output.len() < picture.bytes {
			output
				.try_reserve_exact(picture.bytes - output.len())
				.map_err(|_| crate::video::INVALID)?;
			output.resize(picture.bytes, 0);
		}
		// SAFETY: The validated length exactly matches the writable output slice.
		let bytes = picture.bytes;
		decode_result(unsafe {
			serein_decode_receive(
				self.native.as_ptr(),
				&mut picture,
				output.as_mut_ptr(),
				bytes,
			)
		})?;
		Ok(Some(picture))
	}
}
