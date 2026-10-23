//! Owned Windows capture surfaces. GPU work is bounded and completed before a
//! frame reaches encoding; CPU downloads happen only on the media worker.
#![allow(unsafe_code)]

use super::ffmpeg::{Adapter, CaptureLuminance, SdrConverter, half};
use std::{ffi::c_void, ptr::NonNull};

const FAILED: &str = "Screen capture processing failed";
const MAX_BYTES: usize = 7680 * 4320 * 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
	Bgra8,
	LinearRgba16 {
		unit_nits: u32,
		white_nits: u32,
		peak_nits: u32,
	},
	Nv12,
}

#[derive(Clone, Copy, Debug)]
pub struct Description {
	pub width: u32,
	pub height: u32,
	pub stride: usize,
	pub format: PixelFormat,
	pub bytes: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeDescription {
	width: u32,
	height: u32,
	stride: u32,
	format: u32,
	source_width: u32,
	source_height: u32,
	unit_nits: u32,
	white_nits: u32,
	peak_nits: u32,
	bytes: usize,
}

impl NativeDescription {
	fn validated(self) -> Result<Description, &'static str> {
		let (format, row_bytes, rows) = match self.format {
			0 => (
				PixelFormat::Bgra8,
				self.width as usize * 4,
				self.height as usize,
			),
			1 if (1..=1000).contains(&self.unit_nits)
				&& (80..=1000).contains(&self.white_nits)
				&& (self.white_nits..=10000).contains(&self.peak_nits) =>
			{
				(
					PixelFormat::LinearRgba16 {
						unit_nits: self.unit_nits,
						white_nits: self.white_nits,
						peak_nits: self.peak_nits,
					},
					self.width as usize * 8,
					self.height as usize,
				)
			}
			2 if self.width.is_multiple_of(2) && self.height.is_multiple_of(2) => (
				PixelFormat::Nv12,
				self.width as usize,
				self.height as usize * 3 / 2,
			),
			_ => return Err(FAILED),
		};
		if !client_core::screen::valid_dimensions(self.width, self.height)
			|| !client_core::screen::valid_dimensions(self.source_width, self.source_height)
			|| self.width > self.source_width
			|| self.height > self.source_height
			|| self.stride as usize != row_bytes
			|| self.bytes > MAX_BYTES
			|| row_bytes.checked_mul(rows) != Some(self.bytes)
		{
			return Err(FAILED);
		}
		Ok(Description {
			width: self.width,
			height: self.height,
			stride: self.stride as usize,
			format,
			bytes: self.bytes,
		})
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
	pub submitted: u64,
	pub completed: u64,
	pub skipped: u64,
	pub resources_created: u64,
	pub readback_bytes: u64,
	pub readback_ns: u64,
	pub gpu_frames: u64,
}

unsafe extern "C" {
	fn serein_capture_open(
		device: *mut c_void,
		context: *mut c_void,
		adapter: *const Adapter,
		gpu: i32,
	) -> *mut c_void;
	fn serein_capture_close(capture: *mut c_void);
	fn serein_capture_submit(
		capture: *mut c_void,
		texture: *mut c_void,
		width: u32,
		height: u32,
		target_width: u32,
		target_height: u32,
		format: u32,
		luminance: *const CaptureLuminance,
	) -> i32;
	fn serein_capture_take(capture: *mut c_void, frame: *mut *mut c_void) -> i32;
	fn serein_capture_stats(capture: *mut c_void, stats: *mut Stats);
	fn serein_capture_wants_cpu(capture: *mut c_void) -> i32;
	fn serein_capture_cpu_replacement(capture: *mut c_void) -> *mut c_void;
	fn serein_capture_replacement(capture: *mut c_void, gpu: i32) -> *mut c_void;
	fn serein_capture_frame_prefer_cpu(frame: *mut c_void);
	fn serein_capture_frame_close(frame: *mut c_void);
	fn serein_capture_frame_description(
		frame: *mut c_void,
		description: *mut NativeDescription,
	) -> i32;
	fn serein_capture_frame_avframe(frame: *const c_void) -> *const c_void;
	fn serein_capture_frame_download(frame: *mut c_void, output: *mut u8, capacity: usize) -> i32;
	fn serein_capture_frame_preview(
		frame: *mut c_void,
		max_width: u32,
		max_height: u32,
		output: *mut u8,
		capacity: usize,
		description: *mut NativeDescription,
	) -> i32;
}

/// Created/submitted on the capture thread and polled by its completion worker
/// through one serialized owner. Frame leases retain their originating pool.
pub struct Processor {
	native: NonNull<c_void>,
}

// SAFETY: Native owners serialize all generations sharing an immediate context.
// Export to FFmpeg additionally requires D3D multithread protection. A Mutex
// owns this moved handle; it contains no apartment-bound WinRT capture objects.
unsafe impl Send for Processor {}

impl Processor {
	/// # Safety
	/// `device` and `context` must be live, matching ID3D11 interfaces from WGC.
	/// The native owner takes its own COM references before this returns.
	pub unsafe fn new(
		device: *mut c_void,
		context: *mut c_void,
		adapter: Option<model::VideoAdapter>,
		gpu: bool,
	) -> Result<Self, &'static str> {
		let adapter = adapter.map(Adapter::from);
		// SAFETY: The caller supplies the two matching live COM interfaces.
		let native = unsafe {
			serein_capture_open(
				device,
				context,
				adapter
					.as_ref()
					.map_or(std::ptr::null(), std::ptr::from_ref),
				i32::from(gpu),
			)
		};
		Ok(Self {
			native: NonNull::new(native).ok_or(FAILED)?,
		})
	}

	/// Queue one picture without mapping or waiting. False is bounded backpressure.
	/// # Safety
	/// `texture` must be a live capture texture belonging to the constructor device.
	pub unsafe fn submit(
		&mut self,
		texture: *mut c_void,
		source: (u32, u32),
		target: (u32, u32),
		format: PixelFormat,
	) -> Result<bool, &'static str> {
		let (native_format, luminance) = match format {
			PixelFormat::Bgra8 => (0, None),
			PixelFormat::LinearRgba16 {
				unit_nits,
				white_nits,
				peak_nits,
			} => (
				1,
				Some(CaptureLuminance {
					unit_nits,
					white_nits,
					peak_nits,
				}),
			),
			PixelFormat::Nv12 => return Err(FAILED),
		};
		// SAFETY: The texture and owner are live; C validates geometry before work.
		match unsafe {
			serein_capture_submit(
				self.native.as_ptr(),
				texture,
				source.0,
				source.1,
				target.0,
				target.1,
				native_format,
				luminance
					.as_ref()
					.map_or(std::ptr::null(), std::ptr::from_ref),
			)
		} {
			0 => Ok(false),
			1 => Ok(true),
			_ => Err(FAILED),
		}
	}

	pub fn take(&mut self) -> Result<Option<Frame>, &'static str> {
		let mut native = std::ptr::null_mut();
		// SAFETY: This owner is exclusive; the out pointer lives through the call.
		match unsafe { serein_capture_take(self.native.as_ptr(), &mut native) } {
			0 => Ok(None),
			1 => {
				let native = NonNull::new(native).ok_or(FAILED)?;
				let mut description = NativeDescription::default();
				// SAFETY: C has returned one owned frame lease.
				if unsafe { serein_capture_frame_description(native.as_ptr(), &mut description) }
					!= 1
				{
					unsafe { serein_capture_frame_close(native.as_ptr()) };
					return Err(FAILED);
				}
				if description.validated().is_err() {
					unsafe { serein_capture_frame_close(native.as_ptr()) };
					return Err(FAILED);
				}
				Ok(Some(Frame {
					native,
					description,
				}))
			}
			_ => Err(FAILED),
		}
	}

	pub fn stats(&self) -> Stats {
		let mut stats = Stats::default();
		// SAFETY: The owner is live; C serializes access to this fixed-size record.
		unsafe { serein_capture_stats(self.native.as_ptr(), &mut stats) };
		stats
	}
	pub fn wants_cpu(&self) -> bool {
		// SAFETY: The owner is live; this reads one interlocked preference flag.
		unsafe { serein_capture_wants_cpu(self.native.as_ptr()) != 0 }
	}
	pub fn cpu_replacement(&self) -> Result<Self, &'static str> {
		// SAFETY: C clones the live device/context references into a fresh pool.
		let native = unsafe { serein_capture_cpu_replacement(self.native.as_ptr()) };
		Ok(Self {
			native: NonNull::new(native).ok_or(FAILED)?,
		})
	}
	pub fn replacement(&self, gpu: bool) -> Result<Self, &'static str> {
		// SAFETY: C clones the live interfaces and preserves the selected adapter.
		let native = unsafe { serein_capture_replacement(self.native.as_ptr(), i32::from(gpu)) };
		Ok(Self {
			native: NonNull::new(native).ok_or(FAILED)?,
		})
	}
}

impl Drop for Processor {
	fn drop(&mut self) {
		// SAFETY: This releases the constructor reference, not existing frame leases.
		unsafe { serein_capture_close(self.native.as_ptr()) };
	}
}

pub struct Frame {
	native: NonNull<c_void>,
	description: NativeDescription,
}

// SAFETY: A frame is an immutable completed GPU/staging lease. Native calls use
// a critical section and a multithread-protected D3D immediate context. It has
// one Rust owner and is moved from capture to media processing, never aliased.
unsafe impl Send for Frame {}

impl Frame {
	pub fn dimensions(&self) -> (u32, u32) {
		(self.description.width, self.description.height)
	}
	pub fn source_dimensions(&self) -> (u32, u32) {
		(
			self.description.source_width,
			self.description.source_height,
		)
	}
	pub fn description(&self) -> Description {
		self.description
			.validated()
			.expect("validated capture description")
	}
	pub fn as_encoder_frame(&self) -> *const c_void {
		// SAFETY: The returned borrow lives only as long as this frame lease.
		unsafe { serein_capture_frame_avframe(self.native.as_ptr()) }
	}
	pub fn prefer_cpu(&self) {
		// SAFETY: Only the interlocked pool preference changes, not this picture.
		unsafe { serein_capture_frame_prefer_cpu(self.native.as_ptr()) };
	}
	pub fn download_into(&self, bytes: &mut Vec<u8>) -> Result<Description, &'static str> {
		let description = self.description();
		if bytes.len() < description.bytes {
			bytes
				.try_reserve_exact(description.bytes - bytes.len())
				.map_err(|_| FAILED)?;
		}
		bytes.resize(description.bytes, 0);
		// SAFETY: The output is exactly the previously validated bounded picture size.
		if unsafe {
			serein_capture_frame_download(self.native.as_ptr(), bytes.as_mut_ptr(), bytes.len())
		} != 1
		{
			return Err(FAILED);
		}
		Ok(description)
	}
	pub fn preview(
		&self,
		max_width: u32,
		max_height: u32,
	) -> Result<Option<image::RgbaImage>, &'static str> {
		if max_width == 0 || max_height == 0 || max_width > 640 || max_height > 360 {
			return Err(FAILED);
		}
		// The temporary is thumbnail-sized even for half-float HDR input.
		let mut pixels = vec![0; max_width as usize * max_height as usize * 8];
		let mut description = NativeDescription::default();
		// SAFETY: Native code never writes beyond this bounded output allocation.
		match unsafe {
			serein_capture_frame_preview(
				self.native.as_ptr(),
				max_width,
				max_height,
				pixels.as_mut_ptr(),
				pixels.len(),
				&mut description,
			)
		} {
			0 => return Ok(None),
			1 => {}
			_ => return Err(FAILED),
		}
		let metadata = description.validated()?;
		let mut image = image::RgbaImage::new(metadata.width, metadata.height);
		match metadata.format {
			PixelFormat::Bgra8 => {
				for (source, destination) in pixels[..metadata.bytes]
					.as_chunks::<4>()
					.0
					.iter()
					.zip(image.pixels_mut())
				{
					*destination = image::Rgba([source[2], source[1], source[0], 255]);
				}
			}
			PixelFormat::LinearRgba16 {
				unit_nits,
				white_nits,
				peak_nits,
			} => {
				let converter = SdrConverter::new(1, peak_nits as f32, white_nits as f32, None);
				for (source, destination) in pixels[..metadata.bytes]
					.as_chunks::<8>()
					.0
					.iter()
					.zip(image.pixels_mut())
				{
					let rgb = std::array::from_fn(|channel| {
						half([source[channel * 2], source[channel * 2 + 1]]) * unit_nits as f32
					});
					let [r, g, b] = converter.to_sdr(rgb);
					*destination = image::Rgba([r, g, b, 255]);
				}
			}
			PixelFormat::Nv12 => return Err(FAILED),
		}
		Ok(Some(image))
	}
}

impl Drop for Frame {
	fn drop(&mut self) {
		// SAFETY: This is the sole owner of the opaque frame lease.
		unsafe { serein_capture_frame_close(self.native.as_ptr()) };
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	unsafe extern "C" {
		fn serein_capture_synthetic_test(hardware: i32, hdr: i32) -> i32;
		fn serein_capture_synthetic_device_loss() -> i32;
	}
	#[test]
	fn synthetic_warp_device_removal_retires_pending_and_retained_pools() {
		// SAFETY: Only a newly created private D3D12 WARP device is removed.
		// Its D3D11On12 capture surfaces are synthetic; no hardware GPU,
		// desktop capture session, display reset or system-wide TDR is used.
		let result = unsafe { serein_capture_synthetic_device_loss() };
		let unavailable = match result {
			0 => Some("optional system D3D12/D3D11On12 entry points are unavailable"),
			2 => Some("optional IDXGIFactory4 or ID3D12Device5 interface is unavailable"),
			3 => Some("D3D12 WARP is unavailable"),
			4 => Some("D3D11On12 cannot layer over the private WARP device"),
			_ => None,
		};
		if let Some(reason) = unavailable {
			eprintln!(
				"Device-removal regression unavailable: {reason}; ordinary WARP pool tests remain mandatory"
			);
			return;
		}
		assert_eq!(
			result, 1,
			"Private WARP device-removal regression failed ({result}); -2 means removal was not observed, -3 means processing failed to stop within its bounded poll"
		);
	}
	#[test]
	fn synthetic_warp_pool_readback_and_retirement() {
		// SAFETY: The fixture creates its own textures; it never starts desktop capture.
		let result = unsafe { serein_capture_synthetic_test(0, 0) };
		assert_eq!(result, 1, "WARP capture-surface fixture failed ({result})");
	}
	#[test]
	fn synthetic_warp_half_float_preserves_hdr_and_source_geometry() {
		// SAFETY: Offline owned half-float textures only; no monitor or picker is used.
		let result = unsafe { serein_capture_synthetic_test(0, 1) };
		assert_eq!(
			result, 1,
			"WARP HDR capture-surface fixture failed ({result})"
		);
	}
	#[test]
	fn synthetic_hardware_nv12_matches_sdr_color_when_available() {
		// SAFETY: Hardware device/owned test textures only; no desktop pixels are captured.
		let result = unsafe { serein_capture_synthetic_test(1, 0) };
		if matches!(result, 0 | 2) {
			eprintln!(
				"Hardware NV12 video processing unavailable; WARP fallback fixtures remain required"
			);
			return;
		}
		assert_eq!(
			result, 1,
			"Hardware NV12 capture-surface fixture failed ({result})"
		);
	}
	#[test]
	fn capture_description_rejects_stride_size_and_geometry_mismatch() {
		let valid = NativeDescription {
			width: 128,
			height: 72,
			stride: 512,
			format: 0,
			source_width: 256,
			source_height: 144,
			bytes: 128 * 72 * 4,
			..Default::default()
		};
		assert!(valid.validated().is_ok());
		assert!(
			NativeDescription {
				stride: 511,
				..valid
			}
			.validated()
			.is_err()
		);
		assert!(
			NativeDescription {
				bytes: valid.bytes + 1,
				..valid
			}
			.validated()
			.is_err()
		);
		assert!(
			NativeDescription {
				source_width: 64,
				..valid
			}
			.validated()
			.is_err()
		);
		assert!(
			NativeDescription { format: 1, ..valid }
				.validated()
				.is_err()
		);
	}
}
