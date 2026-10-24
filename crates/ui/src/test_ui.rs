//! Native shape inspection shared by synthetic UI tests, without changing their bounds policy.

pub(crate) fn logical_text(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
	match shape {
		egui::Shape::Text(text) => output.push((
			text.galley.job.text.clone(),
			text.galley.rect.translate(text.pos.to_vec2()),
		)),
		egui::Shape::Vec(shapes) => {
			for shape in shapes {
				logical_text(shape, output);
			}
		}
		_ => {}
	}
}

pub(crate) fn visual_text(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
	match shape {
		egui::Shape::Text(text) => {
			output.push((text.galley.job.text.clone(), text.visual_bounding_rect()))
		}
		egui::Shape::Vec(shapes) => {
			for shape in shapes {
				visual_text(shape, output);
			}
		}
		_ => {}
	}
}
