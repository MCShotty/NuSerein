// HQDN3D coefficient calculations are derived from FFmpeg 7.1.5:
// Copyright (c) 2003 Daniel Moreno, 2010 Baptiste Coudurier, 2012 Loren Merritt.
// SPDX-License-Identifier: GPL-2.0-or-later
//! GPU implementation of the fixed webcam filter chain. Pixel operations never
//! fall back to the CPU. The shader is derived from FFmpeg's GPL/LGPL filters.

use std::{
	collections::VecDeque,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicU8, Ordering},
	},
	time::Duration,
};
use wgpu::util::DeviceExt;

use model::voice_settings::CameraDenoisingAvailability;

const UNAVAILABLE: u8 = 0;
const CHECKING: u8 = 1;
const AVAILABLE: u8 = 2;
const MAX_PIXELS: usize = 1920 * 1080;
#[cfg(not(any(test, feature = "gpu-test")))]
const WAIT: Duration = Duration::from_secs(1);
#[cfg(any(test, feature = "gpu-test"))]
const WAIT: Duration = Duration::from_secs(10);
static STATE: AtomicU8 = AtomicU8::new(UNAVAILABLE);
static CONTEXT: OnceLock<Arc<Context>> = OnceLock::new();

pub(super) fn availability() -> CameraDenoisingAvailability {
	match STATE.load(Ordering::Acquire) {
		CHECKING => CameraDenoisingAvailability::Checking,
		AVAILABLE => CameraDenoisingAvailability::Available,
		_ => CameraDenoisingAvailability::Unavailable,
	}
}

/// Initialize from the renderer's actual device; the GPU probe never opens a camera.
fn is_hardware(device_type: wgpu::DeviceType) -> bool {
	matches!(
		device_type,
		wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu
	)
}

pub(super) fn initialize(
	device: wgpu::Device,
	queue: wgpu::Queue,
	wake: Arc<dyn Fn() + Send + Sync>,
) {
	if !is_hardware(device.adapter_info().device_type) {
		STATE.store(UNAVAILABLE, Ordering::Release);
		wake();
		return;
	}
	if STATE
		.compare_exchange(UNAVAILABLE, CHECKING, Ordering::AcqRel, Ordering::Acquire)
		.is_err()
	{
		return;
	}
	let timeout_wake = wake.clone();
	if std::thread::Builder::new()
		.name("camera-gpu-timeout".into())
		.spawn(move || {
			std::thread::sleep(Duration::from_secs(5));
			if STATE
				.compare_exchange(CHECKING, UNAVAILABLE, Ordering::AcqRel, Ordering::Acquire)
				.is_ok()
			{
				timeout_wake();
			}
		})
		.is_err()
	{
		STATE.store(UNAVAILABLE, Ordering::Release);
		wake();
		return;
	}
	let worker_wake = wake.clone();
	if std::thread::Builder::new()
		.name("camera-gpu-probe".into())
		.spawn(move || {
			let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
				let context = Arc::new(Context::new(device, queue, worker_wake.clone())?);
				let mut probe = Session::new(context.clone(), 16, 16)?;
				let mut picture = vec![128; 16 * 16 * 3 / 2];
				for pts in 0..5 {
					if probe.process(&mut picture, pts)?.is_some() {
						return Err(());
					}
				}
				if probe.process(&mut picture, 5)? != Some(0) || picture.iter().any(|&v| v != 128) {
					return Err(());
				}
				Ok(context)
			}));
			if let Ok(Ok(context)) = result {
				if STATE.load(Ordering::Acquire) == CHECKING {
					let _ = CONTEXT.set(context);
					let _ = STATE.compare_exchange(
						CHECKING,
						AVAILABLE,
						Ordering::AcqRel,
						Ordering::Acquire,
					);
				}
			} else {
				let _ = STATE.compare_exchange(
					CHECKING,
					UNAVAILABLE,
					Ordering::AcqRel,
					Ordering::Acquire,
				);
			}
			worker_wake();
		})
		.is_err()
	{
		STATE.store(UNAVAILABLE, Ordering::Release);
		wake();
	}
}

pub(super) fn session(width: usize, height: usize) -> Option<Session> {
	if availability() != CameraDenoisingAvailability::Available {
		return None;
	}
	let context = CONTEXT.get()?.clone();
	match Session::new(context.clone(), width, height) {
		Ok(session) => Some(session),
		Err(()) => {
			context.fail();
			None
		}
	}
}

/// The renderer owns the device; these handles keep the selected adapter, queue,
/// validated pipelines and immutable filter coefficients alive for the camera.
struct Context {
	device: wgpu::Device,
	queue: wgpu::Queue,
	layout: wgpu::BindGroupLayout,
	pipelines: Vec<wgpu::ComputePipeline>,
	coefficients: wgpu::Buffer,
	wake: Arc<dyn Fn() + Send + Sync>,
}

fn guarded<T>(device: &wgpu::Device, build: impl FnOnce() -> T) -> Result<T, ()> {
	let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
	let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
	let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
	let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));
	let internal_error = pollster::block_on(internal.pop());
	let memory_error = pollster::block_on(oom.pop());
	let validation_error = pollster::block_on(validation.pop());
	if internal_error.is_some() || memory_error.is_some() || validation_error.is_some() {
		return Err(());
	}
	result.map_err(|_| ())
}

fn words_as_bytes(words: &[u32]) -> Vec<u8> {
	words.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

fn coefficients() -> Vec<u8> {
	[4.0_f64, 3.0, 6.0, 4.5]
		.into_iter()
		.flat_map(|strength| {
			let gamma = 0.25_f64.ln() / (1.0 - strength / 255.0 - 0.00001).ln();
			(-4096..4096).flat_map(move |index| {
				let sample = f64::from(index * 32 + 15) / 512.0;
				let value = if index == -4096 {
					1
				} else {
					((1.0 - sample.abs() / 255.0).max(0.0).powf(gamma) * 256.0 * sample)
						.round_ties_even() as i32
				};
				value.to_ne_bytes()
			})
		})
		.collect()
}

impl Context {
	fn new(
		device: wgpu::Device,
		queue: wgpu::Queue,
		wake: Arc<dyn Fn() + Send + Sync>,
	) -> Result<Self, ()> {
		let limits = device.limits();
		let max_bytes = MAX_PIXELS * 3 / 2;
		if limits.max_storage_buffers_per_shader_stage < 7
			|| limits.max_storage_buffer_binding_size < (max_bytes * 9) as u64
			|| limits.max_buffer_size < (max_bytes * 9) as u64
			|| limits.max_compute_workgroup_size_x < 64
			|| limits.max_compute_invocations_per_workgroup < 64
			|| limits.max_compute_workgroups_per_dimension < (max_bytes as u32).div_ceil(64)
		{
			return Err(());
		}
		let (layout, pipelines, coefficients) = guarded(&device, || {
			let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
				label: Some("camera denoising"),
				source: wgpu::ShaderSource::Wgsl(include_str!("camera_filter.wgsl").into()),
			});
			let entries = (0..8)
				.map(|binding| wgpu::BindGroupLayoutEntry {
					binding,
					visibility: wgpu::ShaderStages::COMPUTE,
					ty: wgpu::BindingType::Buffer {
						ty: if binding == 0 {
							wgpu::BufferBindingType::Uniform
						} else {
							wgpu::BufferBindingType::Storage {
								read_only: matches!(binding, 1 | 6),
							}
						},
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				})
				.collect::<Vec<_>>();
			let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
				label: Some("camera denoising"),
				entries: &entries,
			});
			let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
				label: Some("camera denoising"),
				bind_group_layouts: &[Some(&layout)],
				immediate_size: 0,
			});
			let pipelines = [
				"ata",
				"horizontal_hq",
				"vertical_hq",
				"blur_horizontal",
				"sharpen",
			]
			.into_iter()
			.map(|entry| {
				device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
					label: Some(entry),
					layout: Some(&pipeline_layout),
					module: &module,
					entry_point: Some(entry),
					compilation_options: Default::default(),
					cache: None,
				})
			})
			.collect();
			let coefficients = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
				label: Some("camera filter coefficients"),
				contents: &coefficients(),
				usage: wgpu::BufferUsages::STORAGE,
			});
			(layout, pipelines, coefficients)
		})?;
		Ok(Self {
			device,
			queue,
			layout,
			pipelines,
			coefficients,
			wake,
		})
	}

	fn fail(&self) {
		STATE.store(UNAVAILABLE, Ordering::Release);
		(self.wake)();
	}
}

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some("camera denoising"),
		size,
		usage,
		mapped_at_creation: false,
	})
}

pub(super) struct Session {
	context: Arc<Context>,
	group: wgpu::BindGroup,
	params: wgpu::Buffer,
	frames: wgpu::Buffer,
	packed: wgpu::Buffer,
	readback: wgpu::Buffer,
	width: u32,
	height: u32,
	length: usize,
	stride: usize,
	pending: VecDeque<(u32, i64)>,
	received: u64,
	first_output: bool,
	upload: Vec<u8>,
}

impl Session {
	fn new(context: Arc<Context>, width: usize, height: usize) -> Result<Self, ()> {
		if !super::CameraDenoiser::supports_dimensions(width, height) {
			return Err(());
		}
		let length = width
			.checked_mul(height)
			.and_then(|pixels| pixels.checked_mul(3))
			.ok_or(())?
			/ 2;
		let stride = length.checked_add(3).ok_or(())? & !3;
		let (group, params, frames, packed, readback) = guarded(&context.device, || {
			let device = &context.device;
			let params = buffer(
				device,
				80,
				wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			);
			let frames = buffer(
				device,
				(stride * 9) as u64,
				wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
			);
			let temporal = buffer(device, (length * 4) as u64, wgpu::BufferUsages::STORAGE);
			let horizontal = buffer(device, (length * 4) as u64, wgpu::BufferUsages::STORAGE);
			let previous = buffer(device, (length * 4) as u64, wgpu::BufferUsages::STORAGE);
			let filtered = buffer(device, (length * 4) as u64, wgpu::BufferUsages::STORAGE);
			let packed = buffer(
				device,
				stride as u64,
				wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
			);
			let readback = buffer(
				device,
				stride as u64,
				wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
			);
			let buffers = [
				&params,
				&frames,
				&temporal,
				&horizontal,
				&previous,
				&filtered,
				&context.coefficients,
				&packed,
			];
			let entries = buffers
				.into_iter()
				.enumerate()
				.map(|(binding, value)| wgpu::BindGroupEntry {
					binding: binding as u32,
					resource: value.as_entire_binding(),
				})
				.collect::<Vec<_>>();
			let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("camera denoising"),
				layout: &context.layout,
				entries: &entries,
			});
			(group, params, frames, packed, readback)
		})?;
		Ok(Self {
			context,
			group,
			params,
			frames,
			packed,
			readback,
			width: width as u32,
			height: height as u32,
			length,
			stride,
			pending: VecDeque::with_capacity(9),
			received: 0,
			first_output: true,
			upload: Vec::with_capacity(stride),
		})
	}

	pub(super) fn process(&mut self, picture: &mut [u8], pts: i64) -> Result<Option<i64>, ()> {
		if picture.len() != self.length || pts < 0 || self.received == u64::MAX {
			return Err(());
		}
		let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
			self.process_inner(picture, pts)
		}))
		.unwrap_or(Err(()));
		if result.is_err() {
			self.context.fail();
		}
		result
	}

	fn process_inner(&mut self, picture: &mut [u8], pts: i64) -> Result<Option<i64>, ()> {
		self.upload.clear();
		self.upload.extend_from_slice(picture);
		self.upload.resize(self.stride, 0);
		if self.received == 0 {
			for _ in 0..4 {
				self.pending.push_back((0, pts));
			}
		}
		let next = (self.received % 9) as u32;
		if self.pending.len() < 9 {
			self.context.queue.write_buffer(
				&self.frames,
				u64::from(next) * self.stride as u64,
				&self.upload,
			);
			let upload = self.context.queue.submit([]);
			self.context
				.device
				.poll(wgpu::PollType::Wait {
					submission_index: Some(upload),
					timeout: Some(WAIT),
				})
				.map_err(|_| ())?;
			self.pending.push_back((next, pts));
			self.received += 1;
			return Ok(None);
		}
		let mut params = [0_u32; 20];
		params[0] = self.width;
		params[1] = self.height;
		params[2] = self.length as u32;
		params[3] = self.width * self.height;
		for (index, (slot, _)) in self.pending.iter().enumerate() {
			params[4 + index] = *slot;
		}
		params[16] = u32::from(self.first_output);
		self.context
			.queue
			.write_buffer(&self.params, 0, &words_as_bytes(&params));
		let mut encoder =
			self.context
				.device
				.create_command_encoder(&wgpu::CommandEncoderDescriptor {
					label: Some("camera denoising"),
				});
		let groups = [
			self.length as u32,
			2 * self.height,
			2 * self.width,
			self.width * self.height,
			(self.length as u32).div_ceil(4),
		];
		for (pipeline, count) in self.context.pipelines.iter().zip(groups) {
			let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
				label: Some("camera denoising"),
				timestamp_writes: None,
			});
			pass.set_pipeline(pipeline);
			pass.set_bind_group(0, &self.group, &[]);
			pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
		}
		encoder.copy_buffer_to_buffer(&self.packed, 0, &self.readback, 0, self.stride as u64);
		self.context.queue.submit([encoder.finish()]);
		// The current capture must be written only after the previous temporal
		// window has been submitted, otherwise a ring slot can be overwritten.
		self.context.queue.write_buffer(
			&self.frames,
			u64::from(next) * self.stride as u64,
			&self.upload,
		);
		let submission = self.context.queue.submit([]);
		let timestamp = self.pending[4].1;
		let (send, receive) = std::sync::mpsc::sync_channel(1);
		self.readback
			.slice(..)
			.map_async(wgpu::MapMode::Read, move |result| {
				let _ = send.send(result);
			});
		self.context
			.device
			.poll(wgpu::PollType::Wait {
				submission_index: Some(submission),
				timeout: Some(WAIT),
			})
			.map_err(|_| ())?;
		receive
			.recv_timeout(WAIT)
			.map_err(|_| ())?
			.map_err(|_| ())?;
		let mapped = self.readback.slice(..).get_mapped_range().map_err(|_| ())?;
		picture.copy_from_slice(&mapped[..self.length]);
		drop(mapped);
		self.readback.unmap();
		self.pending.pop_front();
		self.pending.push_back((next, pts));
		self.received += 1;
		self.first_output = false;
		Ok(Some(timestamp))
	}
}

/// Explicitly invoked by offline tests; never called by the application.
#[cfg(any(test, feature = "gpu-test"))]
pub(super) fn install_test_gpu() -> bool {
	static TEST: OnceLock<bool> = OnceLock::new();
	*TEST.get_or_init(|| {
		let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
		let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
			force_fallback_adapter: true,
			..Default::default()
		}))
		.or_else(|_| {
			pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
		});
		let Ok(adapter) = adapter else { return false };
		let Ok((device, queue)) =
			pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
		else {
			return false;
		};
		let Ok(context) = Context::new(device, queue, Arc::new(|| {})) else {
			return false;
		};
		let _ = CONTEXT.set(Arc::new(context));
		STATE.store(AVAILABLE, Ordering::Release);
		true
	})
}

#[cfg(test)]
mod tests {
	#[test]
	fn software_and_unknown_adapters_are_never_accepted() {
		use wgpu::DeviceType;
		assert!(super::is_hardware(DeviceType::IntegratedGpu));
		assert!(super::is_hardware(DeviceType::DiscreteGpu));
		assert!(!super::is_hardware(DeviceType::Cpu));
		assert!(!super::is_hardware(DeviceType::VirtualGpu));
		assert!(!super::is_hardware(DeviceType::Other));
	}
}
