// Optional, bounded instrumentation for the offline native preview, never the application.
struct NativeMetrics {
	output: PathBuf,
	started: Instant,
	samples: Vec<(f64, u64, usize)>,
	written: bool,
}
impl NativeMetrics {
	fn new(output: &str) -> Self {
		Self {
			output: output.into(),
			started: Instant::now(),
			samples: Vec::new(),
			written: false,
		}
	}
	fn record(&mut self, ctx: &egui::Context, duration: Duration, wheel_events: usize) {
		if self.written {
			return;
		}
		let elapsed = self.started.elapsed().as_secs_f64();
		if (3.0..8.0).contains(&elapsed) && self.samples.len() < 8192 {
			self.samples.push((
				elapsed,
				duration.as_nanos().min(u128::from(u64::MAX)) as u64,
				wheel_events,
			));
		}
		if elapsed >= 8.0 {
			let report = serde_json::json!({
				"method":"MessagingUi::show callback only; not presented-frame or input latency",
				"warmup_seconds":3, "sample_seconds":5,
				"columns":["elapsed_seconds","callback_nanoseconds","wheel_event_count"],
				"samples":self.samples
			});
			std::fs::write(&self.output, serde_json::to_vec(&report).unwrap())
				.expect("write offline preview measurements");
			self.written = true;
		} else {
			// Ensure the report is flushed even for an otherwise idle preview.
			ctx.request_repaint_after(Duration::from_secs_f64(8.0 - elapsed));
		}
	}
}
