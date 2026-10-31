//! Bounded webcam denoising on the selected hardware GPU. No CPU filter fallback.

use model::voice_settings::CameraDenoisingAvailability;
use std::sync::Arc;

mod gpu;

/// Initialize and verify the filter against the renderer's actual GPU without
/// opening a camera. The callback requests a UI refresh when the result changes.
pub fn initialize_gpu(device: wgpu::Device, queue: wgpu::Queue, wake: Arc<dyn Fn() + Send + Sync>) {
	gpu::initialize(device, queue, wake);
}

/// Session-only availability, distinct from the saved user preference.
pub fn availability() -> CameraDenoisingAvailability {
	gpu::availability()
}

/// Install an offline software adapter only when explicitly invoked by tests.
#[cfg(feature = "gpu-test")]
pub fn install_test_gpu() -> bool {
	gpu::install_test_gpu()
}

/// The filter rejected its input or could not complete GPU processing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DenoiseError;

/// Bounded GPU filter chain with a nine-frame temporal window.
pub struct CameraDenoiser {
	filter: gpu::Session,
}

impl CameraDenoiser {
	/// Reject unsupported geometry before allocating GPU resources.
	pub fn supports_dimensions(width: usize, height: usize) -> bool {
		width > 0
			&& height >= 12
			&& width <= 7680
			&& height <= 4320
			&& width.is_multiple_of(2)
			&& height.is_multiple_of(2)
			&& width
				.checked_mul(height)
				.is_some_and(|pixels| pixels <= 1920 * 1080)
	}

	pub fn new(width: usize, height: usize) -> Option<Self> {
		Self::supports_dimensions(width, height)
			.then(|| gpu::session(width, height))
			.flatten()
			.map(|filter| Self { filter })
	}

	/// Replaces the I420 picture when a GPU result is ready, retaining its
	/// capture timestamp. `None` represents temporal warmup.
	pub fn process(&mut self, picture: &mut [u8], pts: i64) -> Result<Option<i64>, DenoiseError> {
		self.filter.process(picture, pts).map_err(|_| DenoiseError)
	}
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
	use super::CameraDenoiser;
	use std::{ffi::c_void, ptr::NonNull};

	unsafe extern "C" {
		fn serein_denoise_new(width: i32, height: i32) -> *mut c_void;
		fn serein_denoise_free(filter: *mut c_void);
		fn serein_denoise_process(
			filter: *mut c_void,
			input: *const u8,
			output: *mut u8,
			pts: i64,
			output_pts: *mut i64,
		) -> i32;
	}

	struct Reference(NonNull<c_void>);
	impl Reference {
		fn new(width: usize, height: usize) -> Self {
			// SAFETY: the synthetic dimensions are positive and bounded.
			Self(NonNull::new(unsafe { serein_denoise_new(width as i32, height as i32) }).unwrap())
		}
		fn process(&mut self, picture: &mut [u8], pts: i64) -> Option<i64> {
			let mut output_pts = 0;
			// SAFETY: the oracle owns its graph and the input/output have the same validated size.
			let result = unsafe {
				serein_denoise_process(
					self.0.as_ptr(),
					picture.as_ptr(),
					picture.as_mut_ptr(),
					pts,
					&mut output_pts,
				)
			};
			assert!(result >= 0);
			(result == 1).then_some(output_pts)
		}
	}
	impl Drop for Reference {
		fn drop(&mut self) {
			// SAFETY: the oracle is uniquely owned by this fixture.
			unsafe { serein_denoise_free(self.0.as_ptr()) }
		}
	}

	#[test]
	fn dimensions_and_input_are_bounded() {
		assert!(CameraDenoiser::supports_dimensions(1920, 1080));
		assert!(!CameraDenoiser::supports_dimensions(usize::MAX, usize::MAX));
		assert!(!CameraDenoiser::supports_dimensions(3840, 2160));
		assert!(!CameraDenoiser::supports_dimensions(16, 10));
		assert!(CameraDenoiser::new(0, 10).is_none());
		assert!(CameraDenoiser::new(3, 14).is_none());
		assert!(CameraDenoiser::new(3840, 2160).is_none());
		if !super::gpu::install_test_gpu() {
			return;
		}
		let mut filter = CameraDenoiser::new(16, 16).expect("offline compute filter");
		assert!(filter.process(&mut [0; 3], 0).is_err());
	}

	#[test]
	fn gpu_matches_the_requested_ffmpeg_chain_and_preserves_timestamps() {
		if !super::gpu::install_test_gpu() {
			eprintln!("No offline GPU adapter is available for the shader fixture");
			return;
		}
		for (width, height) in [(16, 16), (18, 14), (64, 48)] {
			let mut filter = CameraDenoiser::new(width, height).expect("offline compute filter");
			let mut reference = Reference::new(width, height);
			let length = width * height * 3 / 2;
			let mut checked = 0;
			for pts in 0..24_i64 {
				let input = (0..length)
					.map(|i| {
						let pattern = ((i * 37 + (i / width) * 13 + usize::from(pts >= 12) * 53)
							% 220) as i32;
						let noise = ((i * 7 + pts as usize * 3) % 9) as i32 - 4;
						(pattern + noise + 12).clamp(0, 255) as u8
					})
					.collect::<Vec<_>>();
				let mut expected = input.clone();
				let mut actual = input;
				let expected_pts = reference.process(&mut expected, pts);
				let actual_pts = filter.process(&mut actual, pts).unwrap();
				assert_eq!(actual_pts, expected_pts);
				if actual_pts.is_some() {
					for (index, (a, b)) in actual.iter().zip(&expected).enumerate() {
						assert!(
							a.abs_diff(*b) <= 1,
							"{width}x{height}, frame {pts}, sample {index}: {a} vs {b}"
						);
					}
					checked += 1;
				}
			}
			assert_eq!(checked, 19);
		}
	}
}
