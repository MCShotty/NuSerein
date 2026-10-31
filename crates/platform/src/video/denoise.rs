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

/// Bounded webcam filter chain with a nine-frame temporal window.
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
			&& width.is_multiple_of(2)
			&& height.is_multiple_of(2)
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
	fn camera_filter_matches_the_requested_ffmpeg_chain() {
		let mut filter = CameraDenoiser::new(16, 16).expect("packaged FFmpeg filters");
		let mut compared = false;
		for input_pts in 0..24_usize {
			let mut frame = vec![0; 16 * 16 * 3 / 2];
			for y in 0..16 {
				for x in 0..16 {
					frame[y * 16 + x] = (50
						+ if x >= 8 { 75 } else { 0 }
						+ y * 2 + (input_pts * 3 + x * 7 + y * 11) % 7) as u8;
				}
			}
			for plane in 0..2 {
				for y in 0..8 {
					for x in 0..8 {
						frame[256 + plane * 64 + y * 8 + x] = ((if plane == 0 { 90 } else { 150 })
							+ (x * 3 + y * 2 + input_pts) % 13)
							as u8;
					}
				}
			}
			let output_pts = filter.process(&mut frame, input_pts as i64).unwrap();
			if input_pts < 5 {
				assert_eq!(
					output_pts, None,
					"the default nine-frame filter must warm up"
				);
				continue;
			}
			assert_eq!(output_pts, Some((input_pts - 5) as i64));
			if output_pts == Some(8) {
				// FFmpeg 7.1.5 reference output for atadenoise,hqdn3d=4,unsharp=7:7:0.5.
				// Samples include both sides of the edge and chroma affected by hqdn3d.
				for (index, expected) in [
					(69, 59),
					(71, 47),
					(72, 148),
					(74, 135),
					(101, 63),
					(103, 51),
					(104, 152),
					(106, 139),
					(181, 73),
					(183, 61),
					(184, 162),
					(186, 149),
					(213, 77),
					(215, 65),
					(216, 166),
					(218, 153),
					(267, 93),
					(268, 96),
					(269, 99),
					(271, 92),
				] {
					assert!(frame[index].abs_diff(expected) <= 1, "pixel {index}");
				}
				compared = true;
			}
		}
		assert!(
			compared,
			"the camera filter must produce the reference frame"
		);
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
