//! One-pass letterboxed BGRA scaling and BT.601 limited-range 4:2:0 conversion.
//!
//! Screen sources rarely match the stream preset (a 1440p monitor streamed at 1080p), so
//! scaling, color conversion and chroma packing happen together in integer arithmetic,
//! split into row bands across a few threads, and written straight into the encoder's
//! input buffer. Coefficients match openh264's converter, so both encoders look the same.
use super::{RawFrame, validate_frame};

/// Bilinear source tap: byte offsets of the two neighbours and the 8-bit blend weight.
#[derive(Clone, Copy)]
struct Tap {
	near: usize,
	far: usize,
	weight: u32,
}

/// Destination axis mapping into a source axis, with letterbox bounds.
#[derive(Default)]
struct Axis {
	taps: Vec<Tap>,
	start: usize,
	end: usize,
}

impl Axis {
	#[cfg(test)]
	fn new(source: usize, scaled: usize, destination: usize, step: usize) -> Self {
		let mut axis = Self::default();
		axis.reset(source, scaled, destination, step);
		axis
	}

	fn reset(&mut self, source: usize, scaled: usize, destination: usize, step: usize) {
		self.start = (destination - scaled) / 2;
		self.end = self.start + scaled;
		self.taps.clear();
		self.taps.extend((0..scaled).map(|index| {
			let position = ((index as f64 + 0.5) * source as f64 / scaled as f64 - 0.5)
				.clamp(0.0, (source - 1) as f64);
			let near = position as usize;
			Tap {
				near: near * step,
				far: (near + 1).min(source - 1) * step,
				weight: ((position - near as f64) * 256.0) as u32,
			}
		}));
	}

	fn tap(&self, index: usize) -> Option<Tap> {
		(self.start..self.end)
			.contains(&index)
			.then(|| self.taps[index - self.start])
	}
}

/// A worker-owned conversion workspace. Geometry, row scratch and a small thread pool
/// survive between pictures; full input pictures remain owned by the caller.
pub(crate) struct Converter {
	geometry: Option<Geometry>,
	columns: Axis,
	rows: Axis,
	scratch: Vec<Scratch>,
	pool: Option<rayon::ThreadPool>,
	band_limit: usize,
}
impl Default for Converter {
	fn default() -> Self {
		Self {
			geometry: None,
			columns: Axis::default(),
			rows: Axis::default(),
			scratch: Vec::new(),
			pool: None,
			band_limit: std::thread::available_parallelism()
				.map_or(1, |count| count.get())
				.clamp(1, 4),
		}
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Geometry {
	source: (u32, u32, usize),
	output: (usize, usize),
	hdr: bool,
}

#[derive(Default)]
struct Scratch {
	top: Vec<[u32; 3]>,
	bottom: Vec<[u32; 3]>,
}
impl Scratch {
	fn resize(&mut self, width: usize) {
		self.top.resize(width, [0; 3]);
		self.bottom.resize(width, [0; 3]);
	}
}

impl Converter {
	/// Fit a validated source into limited-range BT.601 I420 without stretching it.
	pub(crate) fn convert(
		&mut self,
		frame: &RawFrame,
		width: usize,
		height: usize,
		output: &mut [u8],
	) -> Result<(), &'static str> {
		validate_frame(frame)?;
		if !super::valid_dimensions(
			u32::try_from(width).unwrap_or(0),
			u32::try_from(height).unwrap_or(0),
		) || !width.is_multiple_of(2)
			|| !height.is_multiple_of(2)
			|| output.len() != width * height * 3 / 2
		{
			return Err("Invalid screen encoder picture size");
		}
		if matches!(frame.format, super::RawFormat::Nv12) {
			return nv12_to_i420(frame, width, height, output);
		}
		let hdr = !matches!(frame.format, super::RawFormat::Bgra8);
		let geometry = Geometry {
			source: (frame.width, frame.height, frame.stride),
			output: (width, height),
			hdr,
		};
		let bands = self.band_limit.min(height / 64).max(1);
		if self.geometry != Some(geometry) {
			let scale =
				(width as f64 / frame.width as f64).min(height as f64 / frame.height as f64);
			let scaled_width = (frame.width as f64 * scale)
				.round()
				.clamp(1.0, width as f64) as usize;
			let scaled_height = (frame.height as f64 * scale)
				.round()
				.clamp(1.0, height as f64) as usize;
			self.columns.reset(
				frame.width as usize,
				scaled_width,
				width,
				if hdr { 1 } else { 4 },
			);
			self.rows.reset(
				frame.height as usize,
				scaled_height,
				height,
				if hdr { 1 } else { frame.stride },
			);
			self.geometry = Some(geometry);
		}
		self.scratch.resize_with(bands, Scratch::default);
		for scratch in &mut self.scratch {
			scratch.resize(width);
		}
		if bands > 1 && self.pool.is_none() {
			self.pool = Some(
				rayon::ThreadPoolBuilder::new()
					.num_threads(self.band_limit - 1)
					.thread_name(|index| format!("screen-convert-{index}"))
					.build()
					.map_err(|_| "Could not start screen conversion workers")?,
			);
		}
		let conversion = if hdr {
			Some(frame.conversion().ok_or("Missing capture color metadata")?)
		} else {
			None
		};
		let columns = &self.columns;
		let rows = &self.rows;
		let band_rows = height.div_ceil(bands).next_multiple_of(2);
		let (luma, uv) = output.split_at_mut(width * height);
		let (u, v) = uv.split_at_mut(width * height / 4);
		let conversion = conversion.as_ref();
		let jobs = luma
			.chunks_mut(band_rows * width)
			.zip(u.chunks_mut(band_rows * width / 4))
			.zip(v.chunks_mut(band_rows * width / 4))
			.zip(&mut self.scratch)
			.enumerate()
			.map(|(index, (((luma, u), v), scratch))| Job {
				frame,
				columns,
				rows,
				conversion,
				width,
				first: index * band_rows,
				luma,
				u,
				v,
				scratch,
			});
		if let Some(pool) = &self.pool {
			// Keep the caller productive as the final band, matching the previous
			// four-way schedule without allocating fresh OS threads per picture.
			pool.in_place_scope(|scope| {
				let mut jobs = jobs.peekable();
				while let Some(job) = jobs.next() {
					if jobs.peek().is_some() {
						scope.spawn(move |_| job.run());
					} else {
						job.run();
					}
				}
			});
		} else {
			for job in jobs {
				job.run();
			}
		}
		Ok(())
	}
}

struct Job<'a> {
	frame: &'a RawFrame,
	columns: &'a Axis,
	rows: &'a Axis,
	conversion: Option<&'a platform::video::ffmpeg::SdrConverter>,
	width: usize,
	first: usize,
	luma: &'a mut [u8],
	u: &'a mut [u8],
	v: &'a mut [u8],
	scratch: &'a mut Scratch,
}
impl Job<'_> {
	fn run(self) {
		if let Some(conversion) = self.conversion {
			HdrBand {
				frame: self.frame,
				columns: self.columns,
				rows: self.rows,
				conversion,
				width: self.width,
				first: self.first,
				luma: self.luma,
				u: self.u,
				v: self.v,
			}
			.run(&mut self.scratch.top, &mut self.scratch.bottom);
		} else {
			Band {
				source: &self.frame.data,
				columns: self.columns,
				rows: self.rows,
				width: self.width,
				first: self.first,
				luma: self.luma,
				u: self.u,
				v: self.v,
			}
			.run(&mut self.scratch.top, &mut self.scratch.bottom);
		}
	}
}

/// One-shot helper for fixtures. Production workers retain a `Converter`.
#[cfg(test)]
pub(crate) fn bgra_to_i420(
	frame: &RawFrame,
	width: usize,
	height: usize,
	output: &mut [u8],
) -> Result<(), &'static str> {
	Converter::default().convert(frame, width, height, output)
}

/// GPU capture already scales NV12 to the resolved output. Preserve its limited-range
/// samples exactly while unpacking the chroma planes for a software/native fallback.
fn nv12_to_i420(
	frame: &RawFrame,
	width: usize,
	height: usize,
	output: &mut [u8],
) -> Result<(), &'static str> {
	if (frame.width as usize, frame.height as usize) != (width, height) {
		return Err("NV12 capture dimensions do not match the resolved screen size");
	}
	let required = frame
		.stride
		.checked_mul(height + height / 2)
		.ok_or("Invalid NV12 capture size")?;
	if frame.stride < width || required > frame.data.len() {
		return Err("Invalid NV12 capture planes");
	}
	let (luma, uv) = output.split_at_mut(width * height);
	let (u, v) = uv.split_at_mut(width * height / 4);
	for (source, destination) in frame.data[..frame.stride * height]
		.chunks_exact(frame.stride)
		.zip(luma.chunks_exact_mut(width))
	{
		destination.copy_from_slice(&source[..width]);
	}
	for ((source, u), v) in frame.data[frame.stride * height..required]
		.chunks_exact(frame.stride)
		.zip(u.chunks_exact_mut(width / 2))
		.zip(v.chunks_exact_mut(width / 2))
	{
		for ((pixel, u), v) in source[..width].as_chunks::<2>().0.iter().zip(u).zip(v) {
			*u = pixel[0];
			*v = pixel[1];
		}
	}
	Ok(())
}

struct Band<'a> {
	source: &'a [u8],
	columns: &'a Axis,
	rows: &'a Axis,
	width: usize,
	first: usize,
	luma: &'a mut [u8],
	u: &'a mut [u8],
	v: &'a mut [u8],
}

impl Band<'_> {
	fn run(self, top: &mut [[u32; 3]], bottom: &mut [[u32; 3]]) {
		let width = self.width;
		for pair in 0..self.luma.len() / width / 2 {
			let row = self.first + pair * 2;
			self.sample(row, top);
			self.sample(row + 1, bottom);
			let offset = pair * 2 * width;
			for (x, rgb) in top.iter().enumerate() {
				self.luma[offset + x] = luma(*rgb);
			}
			for (x, rgb) in bottom.iter().enumerate() {
				self.luma[offset + width + x] = luma(*rgb);
			}
			let chroma_row = pair * width / 2;
			for x in 0..width / 2 {
				let mut sum = [0u32; 3];
				for rgb in [top[2 * x], top[2 * x + 1], bottom[2 * x], bottom[2 * x + 1]] {
					for (total, channel) in sum.iter_mut().zip(rgb) {
						*total += channel;
					}
				}
				let (u, v) = chroma(sum.map(|total| (total + 2) / 4));
				self.u[chroma_row + x] = u;
				self.v[chroma_row + x] = v;
			}
		}
	}

	/// One destination row as RGB, black outside the letterboxed picture.
	fn sample(&self, row: usize, out: &mut [[u32; 3]]) {
		let Some(y) = self.rows.tap(row) else {
			out.fill([0; 3]);
			return;
		};
		let (near, far) = (&self.source[y.near..], &self.source[y.far..]);
		for (column, rgb) in out.iter_mut().enumerate() {
			let Some(x) = self.columns.tap(column) else {
				*rgb = [0; 3];
				continue;
			};
			for (channel, value) in rgb.iter_mut().enumerate() {
				// BGRA: red is byte 2, green 1, blue 0.
				let byte = 2 - channel;
				let lerp = |line: &[u8]| {
					let a = u32::from(line[x.near + byte]);
					if x.weight == 0 {
						return a << 8;
					}
					a * (256 - x.weight) + u32::from(line[x.far + byte]) * x.weight
				};
				let upper = lerp(near);
				*value = if y.weight == 0 {
					(upper + 128) >> 8
				} else {
					(upper * (256 - y.weight) + lerp(far) * y.weight + 32_768) >> 16
				};
			}
		}
	}
}

fn luma([r, g, b]: [u32; 3]) -> u8 {
	(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

fn chroma([r, g, b]: [u32; 3]) -> (u8, u8) {
	let (r, g, b) = (r as i32, g as i32, b as i32);
	let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
	let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
	(u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

struct HdrBand<'a> {
	frame: &'a RawFrame,
	columns: &'a Axis,
	rows: &'a Axis,
	conversion: &'a platform::video::ffmpeg::SdrConverter,
	width: usize,
	first: usize,
	luma: &'a mut [u8],
	u: &'a mut [u8],
	v: &'a mut [u8],
}
impl HdrBand<'_> {
	fn run(self, top: &mut [[u32; 3]], bottom: &mut [[u32; 3]]) {
		for pair in 0..self.luma.len() / self.width / 2 {
			self.sample(self.first + pair * 2, top);
			self.sample(self.first + pair * 2 + 1, bottom);
			let at = pair * 2 * self.width;
			for (x, rgb) in top.iter().enumerate() {
				self.luma[at + x] = luma(*rgb);
			}
			for (x, rgb) in bottom.iter().enumerate() {
				self.luma[at + self.width + x] = luma(*rgb);
			}
			for x in 0..self.width / 2 {
				let sum = std::array::from_fn(|c| {
					(top[2 * x][c]
						+ top[2 * x + 1][c]
						+ bottom[2 * x][c] + bottom[2 * x + 1][c]
						+ 2) / 4
				});
				let (u, v) = chroma(sum);
				self.u[pair * self.width / 2 + x] = u;
				self.v[pair * self.width / 2 + x] = v;
			}
		}
	}
	fn sample(&self, row: usize, out: &mut [[u32; 3]]) {
		let Some(y) = self.rows.tap(row) else {
			out.fill([0; 3]);
			return;
		};
		let mix = |a: [f32; 3], b: [f32; 3], weight: u32| {
			std::array::from_fn(|i| a[i] + (b[i] - a[i]) * weight as f32 / 256.0)
		};
		for (column, rgb) in out.iter_mut().enumerate() {
			let Some(x) = self.columns.tap(column) else {
				*rgb = [0; 3];
				continue;
			};
			let line = |row| {
				let a = self.frame.nits(x.near, row, self.conversion);
				if x.weight == 0 {
					a
				} else {
					mix(a, self.frame.nits(x.far, row, self.conversion), x.weight)
				}
			};
			let upper = line(y.near);
			let nits = if y.weight == 0 {
				upper
			} else {
				mix(upper, line(y.far), y.weight)
			};
			*rgb = self.conversion.to_sdr(nits).map(u32::from);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn solid(width: u32, height: u32, stride: usize, bgra: [u8; 4]) -> RawFrame {
		let mut data = vec![0xee; stride * height as usize];
		for row in data.chunks_exact_mut(stride) {
			for pixel in row[..width as usize * 4].as_chunks_mut::<4>().0 {
				pixel.copy_from_slice(&bgra);
			}
		}
		RawFrame {
			format: crate::screen::RawFormat::Bgra8,
			width,
			height,
			stride,
			data,
		}
	}

	#[test]
	fn workspace_reuses_geometry_without_retaining_old_pixels_or_stride() {
		let mut converter = Converter::default();
		for (source, target, padding, seed) in [
			((333, 250), (320, 180), 12, 31),
			((333, 250), (320, 180), 12, 79),
			((333, 250), (320, 180), 28, 97),
			((250, 333), (96, 144), 4, 19),
			((128, 128), (128, 128), 0, 47),
			((614, 256), (344, 144), 8, 73),
		] {
			let mut frame = solid(source.0, source.1, source.0 as usize * 4 + padding, [0; 4]);
			for (index, byte) in frame.data.iter_mut().enumerate() {
				*byte = (index * seed % 251) as u8;
			}
			let mut reused = vec![0xee; target.0 * target.1 * 3 / 2];
			let mut fresh = vec![0; reused.len()];
			converter
				.convert(&frame, target.0, target.1, &mut reused)
				.unwrap();
			bgra_to_i420(&frame, target.0, target.1, &mut fresh).unwrap();
			assert_eq!(
				reused, fresh,
				"source={source:?} target={target:?} padding={padding}"
			);
		}
	}

	#[test]
	fn nv12_fallback_preserves_samples_and_rejects_unresolved_scaling() {
		let mut converter = Converter::default();
		let mut frame = RawFrame {
			format: crate::screen::RawFormat::Nv12,
			width: 4,
			height: 2,
			stride: 8,
			data: vec![
				16, 16, 235, 235, 0xee, 0xee, 0xee, 0xee, 82, 82, 145, 145, 0xee, 0xee, 0xee, 0xee,
				90, 240, 54, 34, 0xee, 0xee, 0xee, 0xee,
			],
		};
		let mut output = vec![0; 12];
		converter.convert(&frame, 4, 2, &mut output).unwrap();
		assert_eq!(
			output,
			[16, 16, 235, 235, 82, 82, 145, 145, 90, 54, 240, 34]
		);
		assert!(converter.convert(&frame, 2, 2, &mut [0; 6]).is_err());
		frame.data.truncate(19);
		assert!(converter.convert(&frame, 4, 2, &mut output).is_err());
	}

	#[test]
	fn hdr_workspace_matches_single_band_across_color_metadata_changes() {
		let mut converter = Converter::default();
		for (unit_nits, white_nits, peak_nits) in [(80, 203, 1000), (80, 80, 80), (203, 203, 1000)]
		{
			let frame = RawFrame {
				format: crate::screen::RawFormat::LinearRgba16 {
					unit_nits,
					white_nits,
					peak_nits,
				},
				width: 4,
				height: 4,
				stride: 32,
				data: (0..64)
					.flat_map(|index| [0u16, 0x3800, 0x3c00, 0x4000][index % 4].to_le_bytes())
					.collect(),
			};
			let (width, height) = (128, 128);
			let mut parallel = vec![0; width * height * 3 / 2];
			converter
				.convert(&frame, width, height, &mut parallel)
				.unwrap();
			let mut reference = vec![0; parallel.len()];
			let (luma, uv) = reference.split_at_mut(width * height);
			let (u, v) = uv.split_at_mut(width * height / 4);
			let columns = Axis::new(4, width, width, 1);
			let rows = Axis::new(4, height, height, 1);
			let conversion = frame.conversion().unwrap();
			let mut scratch = Scratch::default();
			scratch.resize(width);
			HdrBand {
				frame: &frame,
				columns: &columns,
				rows: &rows,
				conversion: &conversion,
				width,
				first: 0,
				luma,
				u,
				v,
			}
			.run(&mut scratch.top, &mut scratch.bottom);
			assert_eq!(
				parallel, reference,
				"unit={unit_nits} white={white_nits} peak={peak_nits}"
			);
		}
	}

	#[test]
	fn hdr_conversion_keeps_black_filters_edges_and_validates_planes() {
		let mut frame = RawFrame {
			format: crate::screen::RawFormat::LinearRgba16 {
				unit_nits: 203,
				white_nits: 203,
				peak_nits: 1000,
			},
			width: 2,
			height: 2,
			stride: 16,
			data: vec![0; 32],
		};
		for row in 0..2 {
			for channel in 0..3 {
				frame.data[row * 16 + 8 + channel * 2..row * 16 + 10 + channel * 2]
					.copy_from_slice(&0x3c00u16.to_le_bytes());
			}
		}
		let mut out = vec![0; 4 * 4 * 3 / 2];
		bgra_to_i420(&frame, 4, 4, &mut out).unwrap();
		assert_eq!(out[0], 16);
		assert!(out[1] > out[0] && out[1] < out[2] && out[2] < out[3]);
		assert!((220..=235).contains(&out[3]));
		assert!(out[16..].iter().all(|&value| value == 128));
		for (transfer, full_range, black) in [(16, false, 64u16), (18, true, 0u16)] {
			frame.format = crate::screen::RawFormat::P010 {
				transfer,
				primaries: 9,
				matrix: 9,
				full_range,
				peak_nits: 1000,
			};
			frame.stride = 4;
			frame.data = (0..4)
				.flat_map(|_| (black << 6).to_le_bytes())
				.chain((0..2).flat_map(|_| (512u16 << 6).to_le_bytes()))
				.collect();
			bgra_to_i420(&frame, 4, 4, &mut out).unwrap();
			assert!(out[..16].iter().all(|&value| value == 16));
			assert!(out[16..].iter().all(|&value| value == 128));
			frame.data.pop();
			assert!(bgra_to_i420(&frame, 4, 4, &mut out).is_err());
		}
	}

	#[test]
	fn scales_letterboxes_and_matches_bt601_limited_range() {
		// Padded 4:3 red source into 16:9: red picture with black pillars.
		let frame = solid(64, 48, 64 * 4 + 12, [0, 0, 255, 255]);
		let (width, height) = (128, 72);
		let mut out = vec![0; width * height * 3 / 2];
		bgra_to_i420(&frame, width, height, &mut out).unwrap();
		let (y, uv) = out.split_at(width * height);
		assert_eq!(y[36 * width + 64], 82);
		assert_eq!(y[36 * width], 16);
		assert_eq!(
			(uv[18 * 64 + 32], uv[width * height / 4 + 18 * 64 + 32]),
			(90, 240)
		);
		assert_eq!((uv[18 * 64], uv[width * height / 4 + 18 * 64]), (128, 128));
		// Identity size keeps a sharp one-pixel edge.
		let mut frame = solid(4, 2, 16, [0, 0, 0, 255]);
		frame.data[4..8].copy_from_slice(&[255, 255, 255, 255]);
		let mut out = vec![0; 12];
		bgra_to_i420(&frame, 4, 2, &mut out).unwrap();
		assert_eq!(&out[..4], &[16, 235, 16, 16]);
		assert!(bgra_to_i420(&frame, 3, 2, &mut out).is_err());
		assert!(bgra_to_i420(&frame, 4, 2, &mut out[..11]).is_err());

		{
			let (width, height) = (320, 180);
			let mut frame = solid(333, 250, 333 * 4, [0; 4]);
			for (index, byte) in frame.data.iter_mut().enumerate() {
				*byte = (index * 31 % 251) as u8;
			}
			let mut banded = vec![0; width * height * 3 / 2];
			bgra_to_i420(&frame, width, height, &mut banded).unwrap();
			let columns = Axis::new(333, 240, width, 4);
			let rows = Axis::new(250, 180, height, frame.stride);
			let mut single = vec![0; width * height * 3 / 2];
			let (luma, uv) = single.split_at_mut(width * height);
			let (u, v) = uv.split_at_mut(width * height / 4);
			let mut scratch = Scratch::default();
			scratch.resize(width);
			Band {
				source: &frame.data,
				columns: &columns,
				rows: &rows,
				width,
				first: 0,
				luma,
				u,
				v,
			}
			.run(&mut scratch.top, &mut scratch.bottom);
			assert!(banded == single);
		}
	}
}
