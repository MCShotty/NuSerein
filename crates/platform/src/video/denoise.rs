//! Bounded, single-owner webcam denoising through the packaged FFmpeg filter.
#![allow(unsafe_code)]

use std::{ffi::c_void, ptr::NonNull};

unsafe extern "C" {
	fn serein_denoise_new(width: i32, height: i32) -> *mut c_void;
	fn serein_denoise_free(filter: *mut c_void);
	fn serein_denoise_process(
		filter: *mut c_void,
		input: *const u8,
		output: *mut u8,
		pts: i64,
		out_pts: *mut i64,
	) -> i32;
}

/// The filter rejected its input or could not complete processing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DenoiseError;

/// Five-frame temporal filter. Geometry is bounded before native allocations.
pub struct CameraDenoiser {
	filter: NonNull<c_void>,
	length: usize,
}

impl CameraDenoiser {
	/// Reject unsupported geometry before allocating the temporal graph.
	pub fn supports_dimensions(width: usize, height: usize) -> bool {
		width > 0
			&& height > 0
			&& width <= 7680
			&& height <= 4320
			&& width % 2 == 0
			&& height % 2 == 0
			&& width
				.checked_mul(height)
				.is_some_and(|pixels| pixels <= 1920 * 1080)
	}

	pub fn new(width: usize, height: usize) -> Option<Self> {
		if !Self::supports_dimensions(width, height) {
			return None;
		}
		let pixels = width.checked_mul(height)?;
		let length = pixels.checked_mul(3)?.checked_div(2)?;
		// SAFETY: dimensions are positive, even and bounded; the returned graph is owned.
		let filter = NonNull::new(unsafe { serein_denoise_new(width as i32, height as i32) })?;
		Some(Self { filter, length })
	}

	/// Mutates an I420 picture when output is ready, retaining its submitted timestamp.
	/// Returns `None` during warmup; errors leave the caller free to bypass the filter.
	pub fn process(&mut self, picture: &mut [u8], pts: i64) -> Result<Option<i64>, DenoiseError> {
		if picture.len() != self.length || pts < 0 {
			return Err(DenoiseError);
		}
		let mut output_pts = 0;
		// SAFETY: the native graph is uniquely owned, and the fixed-size input/output may
		// alias because the bridge copies input into an owned AVFrame before writing output.
		let status = unsafe {
			serein_denoise_process(
				self.filter.as_ptr(),
				picture.as_ptr(),
				picture.as_mut_ptr(),
				pts,
				&mut output_pts,
			)
		};
		match status {
			0 => Ok(None),
			1 if output_pts >= 0 => Ok(Some(output_pts)),
			_ => Err(DenoiseError),
		}
	}
}

impl Drop for CameraDenoiser {
	fn drop(&mut self) {
		// SAFETY: this is the only owner, and all operations require an exclusive borrow.
		unsafe { serein_denoise_free(self.filter.as_ptr()) }
	}
}

#[cfg(test)]
mod tests {
	use super::CameraDenoiser;

	#[test]
	fn dimensions_and_input_are_bounded() {
		assert!(CameraDenoiser::supports_dimensions(1920, 1080));
		assert!(!CameraDenoiser::supports_dimensions(usize::MAX, usize::MAX));
		assert!(!CameraDenoiser::supports_dimensions(3840, 2160));
		assert!(CameraDenoiser::new(0, 10).is_none());
		assert!(CameraDenoiser::new(3, 4).is_none());
		assert!(CameraDenoiser::new(3840, 2160).is_none());
		let mut filter = CameraDenoiser::new(16, 16).expect("packaged FFmpeg filter");
		assert!(filter.process(&mut [0; 3], 0).is_err());
	}

	#[test]
	fn temporal_filter_reduces_small_alternating_luma_noise() {
		let mut filter = CameraDenoiser::new(16, 16).expect("packaged FFmpeg filter");
		let mut changed = false;
		for pts in 0..20 {
			let mut frame = vec![128; 16 * 16 * 3 / 2];
			frame[..16 * 16].fill(if pts % 2 == 0 { 100 } else { 104 });
			if let Some(output_pts) = filter.process(&mut frame, pts).unwrap() {
				let submitted = if output_pts % 2 == 0 { 100 } else { 104 };
				changed |= frame[..16 * 16].iter().any(|value| *value != submitted);
			}
		}
		assert!(changed, "the actual filter must alter small temporal noise");
	}

	#[test]
	fn temporal_output_preserves_original_timestamp() {
		let mut filter = CameraDenoiser::new(16, 16).expect("packaged FFmpeg filter");
		let mut previous = None;
		for pts in 0..12 {
			let mut frame = vec![128; 16 * 16 * 3 / 2];
			if let Some(timestamp) = filter.process(&mut frame, pts).unwrap() {
				assert!(timestamp <= pts);
				if let Some(last) = previous {
					assert!(timestamp > last);
				}
				previous = Some(timestamp);
			}
		}
		assert!(previous.is_some());
	}
}
