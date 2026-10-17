//! One bounded "start a thread" request at a time, from a message or from the channel header.
use crate::dialog;
use client_core::{Command, State, channel_actions::Action};
use model::Id;

struct Request {
	channel: Id,
	view: Option<Id>,
	/// The message the thread hangs off, when it was started from one.
	message: Option<Id>,
	name: String,
	submitted: Option<u64>,
	private: bool,
	invitable: bool,
}

/// First line of the starter message, collapsed whitespace, cut at a word boundary near 60 chars.
pub(crate) fn suggested_name(text: &str) -> String {
	let line = text
		.lines()
		.map(str::trim)
		.find(|line| !line.is_empty())
		.unwrap_or_default();
	let words: Vec<&str> = line.split_whitespace().collect();
	let mut out = String::new();
	for word in words {
		let next = out.chars().count() + word.chars().count() + usize::from(!out.is_empty());
		if next > 60 {
			break;
		}
		if !out.is_empty() {
			out.push(' ');
		}
		out.push_str(word);
	}
	if out.is_empty() {
		line.chars().take(60).collect()
	} else {
		out
	}
}

#[derive(Default)]
pub(crate) struct ThreadCreateUi {
	request: Option<Request>,
	generation: u64,
}

impl ThreadCreateUi {
	/// Opens the editor for `channel`, optionally starting from one of its messages.
	pub fn open(&mut self, state: &State, channel: Id, message: Option<Id>, name: String) {
		self.generation = state.generation;
		self.request = Some(Request {
			channel,
			view: state.selected,
			message,
			name: name.chars().take(100).collect(),
			submitted: None,
			private: message.is_none()
				&& !state.can_create_thread(channel)
				&& state.can_create_private_thread(channel),
			invitable: true,
		});
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		if self.generation != state.generation {
			self.generation = state.generation;
			self.request = None;
			return;
		}
		if self
			.request
			.as_ref()
			.is_some_and(|request| request.view != state.selected)
		{
			self.request = None;
			return;
		}
		let Some(request) = &mut self.request else {
			return;
		};
		let channel = request.channel;
		if request
			.submitted
			.is_some_and(|sequence| state.channel_action_completed(channel, sequence) == Some(true))
		{
			// The reducer admitted the new thread; open it like Discord does.
			let created = request
				.submitted
				.and_then(|request| state.created_thread(channel, request));
			self.request = None;
			if let Some(id) = created
				&& let Some(command) = state.select(id)
			{
				commands.push(command);
			}
			return;
		}
		if request.submitted.is_some_and(|sequence| {
			state.channel_action_completed(channel, sequence) == Some(false)
		}) {
			request.submitted = None;
		}
		let mut close = false;
		let result = dialog::Dialog::new(
			("create-thread", channel),
			crate::i18n::translate("thread-create-show-create-thread"),
		)
		.subtitle(crate::i18n::translate_if_key(if request.private {
			"thread-create-private-detail"
		} else if request.message.is_some() {
			"thread-create-show-the-selected-message-starts-the-thread-everyone-who-can-see"
		} else {
			"thread-create-show-everyone-who-can-see-this-channel-can-see-the-thread"
		}))
		.width(420.0)
		.show(ctx, |d| {
			d.content(|ui| {
				if request.message.is_none() {
					ui.add_enabled_ui(!state.channel_action_pending(), |ui| {
						ui.add_enabled_ui(state.can_create_thread(channel), |ui| {
							ui.radio_value(
								&mut request.private,
								false,
								crate::i18n::translate("thread-create-public"),
							);
						});
						ui.add_enabled_ui(state.can_create_private_thread(channel), |ui| {
							ui.radio_value(
								&mut request.private,
								true,
								crate::i18n::translate("thread-create-private"),
							);
						});
						if request.private {
							ui.checkbox(
								&mut request.invitable,
								crate::i18n::translate("thread-create-invitable"),
							);
						}
					});
				}
				let label = dialog::label(ui, "thread-create-show-thread-name");
				dialog::input(
					ui,
					egui::TextEdit::singleline(&mut request.name)
						.align(egui::Align2::LEFT_CENTER)
						.char_limit(100),
				)
				.labelled_by(label.id);
				request.name.shrink_to_fit();
				if let Some(error) = state
					.channel_action_status(channel)
					.filter(|_| !state.channel_action_succeeded(channel))
				{
					dialog::notice(ui, dialog::Level::Error, error);
				}
			});
			d.footer(|ui| {
				ui.add_enabled_ui(
					!state.channel_action_pending()
						&& request.submitted.is_none()
						&& if request.private {
							state.can_create_private_thread(channel)
						} else {
							state.can_create_thread(channel)
						} && (state.demo || state.gateway_connected)
						&& client_core::channel_actions::valid_name(&request.name),
					|ui| {
						if dialog::action(ui, "thread-create-show-create", dialog::Action::Primary)
							.clicked()
						{
							let action = if request.private {
								Action::CreatePrivateThread {
									name: request.name.trim().to_owned(),
									invitable: request.invitable,
								}
							} else {
								Action::CreateThread {
									name: request.name.trim().to_owned(),
									message: request.message,
								}
							};
							if let Some(command) = state.request_channel_action(channel, action) {
								if let Command::ChannelAction {
									request: sequence, ..
								} = &command
								{
									request.submitted = Some(*sequence);
								}
								commands.push(command);
							}
						}
					},
				);
				close = dialog::action(ui, "thread-create-show-cancel", dialog::Action::Neutral)
					.clicked();
			});
		});
		if close || result.close {
			self.request = None;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn labels(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
		match shape {
			egui::Shape::Text(text) => output.push((
				text.galley.job.text.clone(),
				text.galley.rect.translate(text.pos.to_vec2()),
			)),
			egui::Shape::Vec(shapes) => {
				for shape in shapes {
					labels(shape, output);
				}
			}
			_ => {}
		}
	}
	fn frame(
		ctx: &egui::Context,
		state: &mut State,
		editor: &mut ThreadCreateUi,
		width: f32,
		events: Vec<egui::Event>,
	) -> (Vec<(String, egui::Rect)>, Vec<Command>) {
		let mut commands = Vec::new();
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(width, 700.0),
				)),
				events,
				..Default::default()
			},
			|ui| editor.show(ui.ctx(), state, &mut commands),
		);
		let mut text = Vec::new();
		for shape in &output.shapes {
			labels(&shape.shape, &mut text);
		}
		output.drop_without_applying_deltas();
		(text, commands)
	}
	fn click(
		ctx: &egui::Context,
		state: &mut State,
		editor: &mut ThreadCreateUi,
		width: f32,
		pos: egui::Pos2,
	) -> Vec<Command> {
		let mut commands = Vec::new();
		for pressed in [true, false] {
			commands.extend(
				frame(
					ctx,
					state,
					editor,
					width,
					vec![
						egui::Event::PointerMoved(pos),
						egui::Event::PointerButton {
							pos,
							button: egui::PointerButton::Primary,
							pressed,
							modifiers: egui::Modifiers::NONE,
						},
					],
				)
				.1,
			);
		}
		commands
	}
	#[test]
	fn private_creation_submits_once_and_reconnect_preserves_editor_until_generation_changes() {
		for width in [360.0, 900.0] {
			let ctx = egui::Context::default();
			crate::design::apply(&ctx);
			let mut state = test_support::demo_state();
			let channel = Id(20);
			let mut editor = ThreadCreateUi::default();
			editor.open(&state, channel, None, "Private planning".into());
			let mut text = Vec::new();
			for _ in 0..4 {
				(text, _) = frame(&ctx, &mut state, &mut editor, width, vec![]);
			}
			let private = text
				.iter()
				.find(|(text, _)| text == &crate::i18n::translate("thread-create-private"))
				.unwrap()
				.1
				.center();
			assert!(click(&ctx, &mut state, &mut editor, width, private).is_empty());
			assert!(editor.request.as_ref().unwrap().private);
			for _ in 0..4 {
				(text, _) = frame(&ctx, &mut state, &mut editor, width, vec![]);
			}
			let create = text
				.iter()
				.find(|(text, _)| text == &crate::i18n::translate("thread-create-show-create"))
				.unwrap()
				.1
				.center();
			let commands = click(&ctx, &mut state, &mut editor, width, create);
			assert!(matches!(
				commands.as_slice(),
				[Command::ChannelAction {
					channel: Id(20),
					action: Action::CreatePrivateThread { name, invitable: true },
					..
				}] if name == "Private planning"
			));
			assert!(click(&ctx, &mut state, &mut editor, width, create).is_empty());
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::Disconnected,
			});
			frame(&ctx, &mut state, &mut editor, width, vec![]);
			let draft = editor.request.as_ref().unwrap();
			assert_eq!(draft.name, "Private planning");
			assert!(draft.private);
			assert!(draft.submitted.is_none());
			state.demo = false;
			state.gateway_connected = false;
			assert!(click(&ctx, &mut state, &mut editor, width, create).is_empty());
			state.generation += 1;
			frame(&ctx, &mut state, &mut editor, width, vec![]);
			assert!(editor.request.is_none());
		}
	}
}
