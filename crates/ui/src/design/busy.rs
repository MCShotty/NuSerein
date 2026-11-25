//! A three-tick signal for work of unknown duration. No percentage is implied.
use egui::{Color32, Response, Sense, Ui, Widget, WidgetInfo};

#[derive(Default)]
pub struct Busy {
	size: Option<f32>,
	color: Option<Color32>,
}
impl Busy {
	pub fn new() -> Self {
		Self::default()
	}
	pub fn size(mut self, value: f32) -> Self {
		self.size = Some(value);
		self
	}
	pub fn color(mut self, value: Color32) -> Self {
		self.color = Some(value);
		self
	}
	pub fn paint_at(self, ui: &Ui, rect: egui::Rect) {
		if !ui.is_rect_visible(rect) {
			return;
		}
		let unit = rect.width().min(rect.height()) / 12.0;
		let color = self.color.unwrap_or_else(|| super::palette(ui).muted);
		let animate = ui.style().animation_time > 0.0 && ui.input(|i| i.focused);
		let tick = if animate {
			ui.input(|i| (i.time * 4.0).floor() as usize % 3)
		} else {
			1
		};
		for index in 0..3 {
			let center = rect.center() + egui::vec2((index as f32 - 1.0) * 3.2 * unit, 0.0);
			let size = egui::vec2(2.0 * unit, if index == tick { 7.0 } else { 3.5 } * unit);
			ui.painter()
				.rect_filled(egui::Rect::from_center_size(center, size), 2, color);
		}
		if animate {
			ui.ctx()
				.request_repaint_after(std::time::Duration::from_millis(250));
		}
	}
}
impl Widget for Busy {
	fn ui(self, ui: &mut Ui) -> Response {
		let size = self.size.unwrap_or(ui.spacing().interact_size.y);
		let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
		response.widget_info(|| WidgetInfo::new(egui::Role::ProgressIndicator));
		self.paint_at(ui, rect);
		response
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reduced_motion_preserves_the_signal_and_does_not_cycle() {
		let ctx = egui::Context::default();
		super::super::set_reduced_motion(&ctx, true);
		let ink = Color32::from_rgb(23, 127, 159);
		let frame = |time| {
			let out = ctx.run_ui(
				egui::RawInput {
					time: Some(time),
					..Default::default()
				},
				|ui| {
					let response = ui.add(Busy::new().size(24.0).color(ink));
					assert_eq!(response.rect.size(), egui::Vec2::splat(24.0));
				},
			);
			let ticks: Vec<_> = out
				.shapes
				.iter()
				.filter_map(|s| match &s.shape {
					egui::Shape::Rect(r) if r.fill == ink => Some(r.rect),
					_ => None,
				})
				.collect();
			out.drop_without_applying_deltas();
			ticks
		};
		let a = frame(0.0);
		assert_eq!(a.len(), 3);
		assert_eq!(a, frame(0.6));
	}
}
