//! Shared presentation and precise hit targets for message, link and media menus.
use crate::{
	design,
	icons::{self, Icon},
};
use model::Id;

/// Native host request. No text enters Discord or the microphone pipeline.
pub struct MessageSpeech {
	pub generation: u64,
	pub channel: Id,
	pub message: Id,
	pub text: Option<String>,
}

impl crate::MessagingUi {
	pub fn take_message_speech(&mut self) -> Option<MessageSpeech> {
		self.message_speech.take()
	}
	pub fn finish_message_speech(&mut self, error: Option<&'static str>) {
		self.timeline.menu.speaking = None;
		self.message_speech_error = error;
	}
	pub fn message_speech_allowed(
		&self,
		state: &client_core::State,
		generation: u64,
		channel: Id,
		message: Id,
	) -> bool {
		valid_owner(
			state,
			Owner {
				generation,
				channel,
				message,
			},
		) && self.timeline.speech_allowed(state, message)
	}
	pub(crate) fn sync_message_actions(
		&mut self,
		ctx: &egui::Context,
		state: &mut client_core::State,
		commands: &mut Vec<client_core::Command>,
	) {
		if let Some(owner) = self.timeline.menu.speaking
			&& !self.message_speech_allowed(state, owner.generation, owner.channel, owner.message)
		{
			self.timeline.menu.speaking = None;
			self.message_speech = Some(MessageSpeech {
				generation: owner.generation,
				channel: owner.channel,
				message: owner.message,
				text: None,
			});
		}
		if let Some(owner) = self.timeline.menu.speech.take()
			&& valid_owner(state, owner)
		{
			let stopping = self.timeline.menu.speaking == Some(owner);
			let text = (!stopping)
				.then(|| self.timeline.speech_text(state, owner.message))
				.flatten();
			self.timeline.menu.speaking = text.as_ref().map(|_| owner);
			self.message_speech = Some(MessageSpeech {
				generation: owner.generation,
				channel: owner.channel,
				message: owner.message,
				text,
			});
			self.message_speech_error = None;
		}
		self.timeline.menu.reminder_dialog(ctx, state);
		if let Some(owner) = self.timeline.menu.report.take()
			&& valid_owner(state, owner)
			&& let Some(command) = state.open_report(model::reporting::Target {
				channel: owner.channel,
				message: owner.message,
			}) {
			commands.push(command);
		}
		self.report_dialog(ctx, state, commands);
		if let Some(error) = self.message_speech_error.take() {
			self.toasts.push(crate::design::Level::Warning, error);
		}
	}

	fn report_dialog(
		&mut self,
		ctx: &egui::Context,
		state: &mut client_core::State,
		commands: &mut Vec<client_core::Command>,
	) {
		use model::reporting::{Button, Kind};
		state.sync_reporting();
		let Some(target) = state.reporting.target else {
			return;
		};
		let mut close = false;
		let mut back = false;
		let mut next = None;
		let mut submit = false;
		let mut original = false;
		let node = state.report_node().cloned();
		let preview = self
			.timeline
			.speech_text(state, target.message)
			.map(|text| text.chars().take(240).collect::<String>());
		let response = crate::dialog::Dialog::new(
			"report-message",
			crate::i18n::translate("menu-report-message"),
		)
		.show(ctx, |dialog| {
			dialog.content(|ui| {
				if state.reporting.loading {
					ui.spinner();
				}
				if state.reporting.sent.is_some() {
					ui.label(crate::i18n::translate(if state.demo {
						"menu-report-demo-sent"
					} else {
						"menu-report-sent"
					}));
				}
				if let Some(error) = state.reporting.error {
					ui.label(error.label());
				}
				if let Some(node) = &node {
					ui.heading(&node.header);
					if !node.detail.is_empty() {
						ui.label(&node.detail);
					}
					ui.add_enabled_ui(
						!state.reporting.loading && state.reporting.sent.is_none(),
						|ui| {
							for element in &node.elements {
								let mut ignored =
									std::collections::BTreeMap::<String, Vec<String>>::new();
								let fields = if element.submit {
									&mut state.reporting.values
								} else {
									&mut ignored
								};
								ui.add_enabled_ui(
									element.submit
										|| matches!(
											element.kind,
											Kind::Notice(_) | Kind::Link { .. } | Kind::Preview
										),
									|ui| match &element.kind {
										Kind::Checkbox(options) => {
											let values =
												fields.entry(element.name.clone()).or_default();
											for (value, label) in options {
												let mut checked = values.contains(value);
												if ui.checkbox(&mut checked, label).changed() {
													if checked {
														values.push(value.clone());
													} else {
														values.retain(|old| old != value);
													}
												}
											}
										}
										Kind::Dropdown { title, options } => {
											let values =
												fields.entry(element.name.clone()).or_default();
											let selected = values
												.first()
												.and_then(|value| {
													options.iter().find(|(id, _)| id == value)
												})
												.map_or(title.as_str(), |(_, label)| {
													label.as_str()
												});
											egui::ComboBox::from_id_salt((
												"report-field",
												&element.name,
											))
											.selected_text(selected)
											.show_ui(ui, |ui| {
												for (value, label) in options {
													if ui
														.selectable_label(
															values.first() == Some(value),
															label,
														)
														.clicked()
													{
														*values = vec![value.clone()];
													}
												}
											});
										}
										Kind::Text {
											title,
											placeholder,
											limit,
										} => {
											ui.label(title);
											let values = fields
												.entry(element.name.clone())
												.or_insert_with(|| vec![String::new()]);
											if values.is_empty() {
												values.push(String::new());
											}
											ui.add(
												egui::TextEdit::multiline(&mut values[0])
													.hint_text(placeholder)
													.char_limit(*limit)
													.desired_rows(3),
											);
										}
										Kind::Notice(text) => {
											ui.label(text);
										}
										Kind::Link { label, url } => {
											if ui.link(label).clicked() {
												self.timeline.opening = Some(url.clone());
											}
										}
										Kind::Preview => {
											if let Some(preview) = &preview {
												ui.weak(preview);
											}
										}
									},
								);
							}
							let ready = state.reporting.menu.as_ref().is_some_and(|menu| {
								menu.validate_values(
									&state.reporting.breadcrumbs,
									&state.reporting.values,
								)
							});
							ui.add_enabled_ui(ready, |ui| {
								for (label, target) in &node.children {
									if ui.button(label).clicked() {
										next = Some(*target);
									}
								}
							});
							match node.button {
								Button::Next(target) => {
									if ui
										.add_enabled(
											ready,
											egui::Button::new(crate::i18n::translate(
												"menu-report-next",
											)),
										)
										.clicked()
									{
										next = Some(target);
									}
								}
								Button::Submit => {
									if ui
										.add_enabled(
											ready
												&& state.reporting.error
													!= Some(client_core::auth::Failure::Ambiguous),
											egui::Button::new(crate::i18n::translate(
												"menu-report-submit",
											)),
										)
										.clicked()
									{
										submit = true;
									}
								}
								Button::Done | Button::Cancel => {
									if ui.button(crate::i18n::translate("menu-close")).clicked() {
										close = true;
									}
								}
								Button::None => {}
							}
						},
					);
				}
			});
			dialog.footer(|ui| {
				if ui
					.add_enabled(
						!state.reporting.loading
							&& state.reporting.breadcrumbs.len() > 1
							&& state.reporting.sent.is_none(),
						egui::Button::new(crate::i18n::translate("menu-back")),
					)
					.clicked()
				{
					back = true;
				}
				if ui
					.button(crate::i18n::translate("menu-open-discord"))
					.clicked()
				{
					original = true;
				}
			});
		});
		if response.close || close {
			state.reporting.close();
		} else if back {
			state.report_back();
		} else if let Some(target) = next {
			state.choose_report_node(target);
		} else if submit && let Some(command) = state.submit_report() {
			commands.push(command);
		}
		if original {
			let guild = state
				.channel(target.channel)
				.and_then(|c| c.guild)
				.map_or_else(|| "@me".into(), |id| id.to_string());
			self.timeline.browser_opening = Some(format!(
				"https://discord.com/channels/{guild}/{}/{}",
				target.channel, target.message
			));
		}
	}
}

#[derive(Default)]
pub(crate) struct Controls {
	pub reaction: Option<(Id, model::ReactionEmoji)>,
	pub picker: Option<(Id, egui::Rect, egui::Id)>,
	pub save: Option<client_core::inbox::Action>,
	pub custom_reminder: Option<Owner>,
	pub speech: Option<Owner>,
	pub speaking: Option<Owner>,
	pub report: Option<Owner>,
	pub reminder_text: String,
	pub reminder_choice: Option<usize>,
	reminder_revision: u64,
	scope: Option<(u64, Option<Id>)>,
	recent: std::collections::VecDeque<model::ReactionEmoji>,
}

impl Controls {
	pub(crate) fn reminder_dialog(&mut self, ctx: &egui::Context, state: &client_core::State) {
		let Some(owner) = self.custom_reminder else {
			return;
		};
		if !valid_owner(state, owner) {
			self.custom_reminder = None;
			return;
		}
		let revision = crate::local_time::format_revision();
		if self.reminder_revision != revision {
			self.reminder_revision = revision;
			self.reminder_choice = None;
		}
		let mut saved = false;
		let response = crate::dialog::Dialog::new(
			"message-reminder",
			crate::i18n::translate("menu-create-reminder"),
		)
		.show(ctx, |dialog| {
			dialog.content(|ui| {
				ui.label(crate::local_time::timezone_label());
				if ui
					.add(
						egui::TextEdit::singleline(&mut self.reminder_text)
							.hint_text("YYYY-MM-DD HH:MM")
							.char_limit(16),
					)
					.changed()
				{
					self.reminder_choice = None;
				}
				let times = crate::local_time::wall_time(&self.reminder_text);
				if times.len() == 2 {
					ui.label(crate::i18n::translate("menu-reminder-ambiguous"));
					for (index, at) in times.iter().enumerate() {
						ui.radio_value(
							&mut self.reminder_choice,
							Some(index),
							format!("{} · UTC{}", crate::local_time::clock(*at), at.offset()),
						);
					}
				}
				let chosen = if times.len() == 1 {
					times.first()
				} else {
					self.reminder_choice.and_then(|i| times.get(i))
				};
				let now = time::OffsetDateTime::now_utc();
				let chosen =
					chosen.filter(|at| **at > now && **at - now <= time::Duration::days(366));
				if !self.reminder_text.is_empty() && chosen.is_none() {
					ui.weak(crate::i18n::translate(
						if crate::local_time::timezone_status() == Some("timezone-updating") {
							"timezone-updating"
						} else {
							"menu-reminder-invalid"
						},
					));
				}
				if ui
					.add_enabled(
						chosen.is_some() && !state.inbox.loading,
						egui::Button::new(crate::i18n::translate("inbox-set-reminder")),
					)
					.clicked()
				{
					self.save = chosen.map(|at| client_core::inbox::Action::Save {
						channel: owner.channel,
						message: owner.message,
						due_at: Some(at.unix_timestamp_nanos()),
					});
					saved = true;
				}
			});
		});
		if response.close || saved {
			self.custom_reminder = None;
			self.reminder_text.clear();
			self.reminder_choice = None;
		}
	}
	pub(crate) fn sync(&mut self, ctx: &egui::Context, state: &client_core::State) {
		let scope = (state.generation, state.selected);
		if self.scope != Some(scope) || !egui::Popup::is_any_open(ctx) {
			if self.scope != Some(scope)
				&& let Some((_, popup)) = ctx.data(|data| {
					data.get_temp::<(Owner, egui::Id)>(egui::Id::unique("message-menu-open-owner"))
				}) && egui::Popup::is_id_open(ctx, popup)
			{
				egui::Popup::close_all(ctx);
			}
			ctx.data_mut(|data| {
				data.remove::<OpenMenu>(egui::Id::unique("message-menu-active"));
				data.remove::<Hit>(egui::Id::unique("message-menu-hit"));
				data.remove::<(Owner, egui::Id)>(egui::Id::unique("message-menu-open-owner"));
			});
		}
		if self.scope != Some(scope) {
			if self.scope.is_some_and(|old| old.0 != scope.0) {
				self.recent.clear();
			}
			self.custom_reminder = None;
			self.reminder_text.clear();
			self.reminder_choice = None;
			self.speech = None;
			self.report = None;
			self.save = None;
			self.picker = None;
			self.reaction = None;
			self.scope = Some(scope);
		}
		if let Some((owner, popup)) = ctx.data(|data| {
			data.get_temp::<(Owner, egui::Id)>(egui::Id::unique("message-menu-open-owner"))
		}) && egui::Popup::is_id_open(ctx, popup)
			&& !valid_owner(state, owner)
		{
			egui::Popup::close_all(ctx);
			ctx.data_mut(|data| {
				data.remove::<(Owner, egui::Id)>(egui::Id::unique("message-menu-open-owner"))
			});
		}
		if self
			.custom_reminder
			.is_some_and(|owner| !valid_owner(state, owner))
		{
			self.custom_reminder = None;
		}
	}
	pub(crate) fn record(&mut self, emoji: &model::ReactionEmoji) {
		if !emoji.valid() {
			return;
		}
		self.recent.retain(|old| !old.same(emoji));
		self.recent.push_front(emoji.clone());
		self.recent.truncate(32);
	}

	pub(crate) fn quick(
		&self,
		state: &client_core::State,
		message: &model::Message,
		defaults: [&str; 3],
	) -> Vec<model::ReactionEmoji> {
		let guild = state
			.channel(message.channel)
			.and_then(|channel| channel.guild);
		let mut recent: Vec<_> = self.recent.iter().collect();
		recent.sort_by_key(|emoji| {
			!emoji
				.id
				.and_then(|id| state.custom_emoji(id))
				.is_some_and(|(server, _)| Some(server.id) == guild)
		});
		let mut result: Vec<model::ReactionEmoji> = Vec::with_capacity(4);
		for emoji in recent
			.into_iter()
			.cloned()
			.chain(
				defaults
					.into_iter()
					.chain(["🔥"])
					.map(|name| model::ReactionEmoji {
						id: None,
						name: Some(name.into()),
					}),
			) {
			let reacted = state
				.reactions
				.display(message)
				.is_some_and(|items| items.iter().any(|r| r.emoji.same(&emoji) && r.me));
			if !result.iter().any(|old| old.same(&emoji))
				&& state.can_react(message.id, Some(&emoji), !reacted)
			{
				result.push(emoji);
				if result.len() == 4 {
					break;
				}
			}
		}
		result
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
	pub generation: u64,
	pub channel: Id,
	pub message: Id,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
	Message,
	Link(String),
	Image,
}

#[derive(Clone)]
struct Hit {
	owner: Owner,
	frame: u64,
	target: Target,
}

#[derive(Clone)]
struct OpenMenu {
	popup: egui::Id,
	owner: Owner,
	target: Option<Target>,
	anchor: Option<(egui::Id, egui::Pos2)>,
}

pub(crate) fn set_owner(ctx: &egui::Context, owner: Owner) {
	ctx.data_mut(|data| data.insert_temp(egui::Id::unique("message-menu-owner"), owner));
}

pub(crate) fn owner(ctx: &egui::Context) -> Option<Owner> {
	ctx.data(|data| data.get_temp(egui::Id::unique("message-menu-owner")))
}

pub(crate) fn register_hit(response: &egui::Response, target: Target) {
	if ((response.contains_pointer() && response.ctx.input(|i| i.pointer.secondary_clicked()))
		|| (response.has_focus() && keyboard_requested(&response.ctx))
		|| ((response.contains_pointer() || response.has_focus())
			&& crate::select::open_menu(&response.ctx)))
		&& let Some(owner) = owner(&response.ctx)
	{
		let hit = Hit {
			owner,
			frame: response.ctx.cumulative_frame_nr(),
			target,
		};
		response
			.ctx
			.data_mut(|data| data.insert_temp(egui::Id::unique("message-menu-hit"), hit));
	}
}

pub(crate) fn keyboard_requested(ctx: &egui::Context) -> bool {
	ctx.input(|input| {
		input.events.iter().any(|event| {
			matches!(event,
				egui::Event::Key { key: egui::Key::F10, pressed: true, modifiers, .. }
					if *modifiers == egui::Modifiers::SHIFT
			)
		})
	})
}

pub(crate) fn has_hit(ctx: &egui::Context, owner: Owner) -> bool {
	ctx.data(|data| data.get_temp::<Hit>(egui::Id::unique("message-menu-hit")))
		.is_some_and(|hit| hit.owner == owner && hit.frame == ctx.cumulative_frame_nr())
}

pub(crate) fn anchor(
	ctx: &egui::Context,
	popup: egui::Id,
	owner: Owner,
) -> Option<(egui::Id, egui::Pos2)> {
	ctx.data(|data| data.get_temp::<OpenMenu>(egui::Id::unique("message-menu-active")))
		.filter(|open| open.popup == popup && open.owner == owner)
		.and_then(|open| open.anchor)
}

/// Remember a context target while its popup is open. Toolbar overflow stays distinct.
pub(crate) fn target(
	ctx: &egui::Context,
	popup: egui::Id,
	owner: Owner,
	context: bool,
	toolbar: bool,
	anchor: Option<(egui::Id, egui::Pos2)>,
) -> Option<Target> {
	let key = egui::Id::unique("message-menu-active");
	if context {
		let hit = ctx
			.data(|data| data.get_temp::<Hit>(egui::Id::unique("message-menu-hit")))
			.filter(|hit| hit.owner == owner && hit.frame == ctx.cumulative_frame_nr())
			.map_or(Target::Message, |hit| hit.target);
		ctx.data_mut(|data| {
			data.insert_temp(
				key,
				OpenMenu {
					popup,
					owner,
					target: Some(hit),
					anchor,
				},
			)
		});
	} else if toolbar {
		ctx.data_mut(|data| {
			data.insert_temp(
				key,
				OpenMenu {
					popup,
					owner,
					target: None,
					anchor: None,
				},
			)
		});
	}
	if context || toolbar {
		mark_open(ctx, owner, popup);
	}
	ctx.data(|data| data.get_temp::<OpenMenu>(key))
		.filter(|open| open.popup == popup && open.owner == owner)
		.and_then(|open| open.target)
}

pub(crate) fn mark_open(ctx: &egui::Context, owner: Owner, popup: egui::Id) {
	ctx.data_mut(|data| {
		data.insert_temp(egui::Id::unique("message-menu-open-owner"), (owner, popup))
	});
}

pub(crate) fn valid_owner(state: &client_core::State, owner: Owner) -> bool {
	owner.generation == state.generation
		&& state.selected == Some(owner.channel)
		&& state.can_read_history(owner.channel)
		&& !state.timeline.is_deleted(owner.message)
		&& state
			.timeline
			.get(owner.message)
			.or_else(|| state.thread_starter().filter(|m| m.id == owner.message))
			.or_else(|| {
				state
					.interactions
					.ephemeral
					.iter()
					.find(|m| m.id == owner.message)
			})
			.is_some_and(|message| message.channel == owner.channel)
}

pub(crate) fn frame(ctx: &egui::Context) -> egui::Frame {
	let colors = design::palette_for(ctx);
	egui::Frame::popup(&ctx.style_of(ctx.theme()))
		.fill(colors.raised)
		.stroke(egui::Stroke::new(1.0, colors.border))
		.corner_radius(8)
		.inner_margin(8)
}

pub(crate) fn content<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
	ui.set_width(204.0_f32.min((ui.ctx().content_rect().width() - 32.0).max(80.0)));
	// The default floating-area height is only 400 points. Allow the menu to grow to
	// the viewport before the scroll area decides that its contents need clipping.
	let height = (ui.ctx().content_rect().height() - 32.0).max(80.0);
	ui.set_max_height(height);
	ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
	ui.spacing_mut().item_spacing.y = 0.0;
	ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0);
	egui::ScrollArea::vertical()
		.max_height(height)
		.show(ui, |ui| {
			crate::scroll::claim_wheel_content(ui);
			add(ui)
		})
		.inner
}

pub(crate) fn row(
	ui: &mut egui::Ui,
	key: &str,
	icon: Icon,
	enabled: bool,
	danger: bool,
) -> egui::Response {
	let colors = design::palette(ui);
	let color = if danger { colors.danger } else { colors.text };
	let mut button = egui::Button::new((
		glyph(key, icon, color),
		design::medium(ui, crate::i18n::translate_if_key(key), 14.0).color(color),
		egui::Atom::grow(),
	))
	.min_size(egui::vec2(ui.available_width(), 36.0))
	.frame_when_inactive(false)
	.corner_radius(4);
	if matches!(key, "menu-open-link" | "menu-open-media-link") {
		button = button.right_text(icons::atom(Icon::External, 14.0, color));
	}
	if key == "menu-add-reaction" {
		button = button.right_text(icons::atom(Icon::ChevronRight, 16.0, color));
	}
	ui.add_enabled(enabled, button)
}

pub(crate) fn submenu<R>(
	ui: &mut egui::Ui,
	key: &str,
	icon: Icon,
	add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
	let colors = design::palette(ui);
	ui.spacing_mut().interact_size.y = 36.0;
	ui.menu_button(
		(
			glyph(key, icon, colors.text),
			design::medium(ui, crate::i18n::translate_if_key(key), 14.0).color(colors.text),
		),
		|ui| content(ui, add),
	)
}

// Small original line drawings for the three menu symbols absent from the shared atlas.
fn glyph(key: &str, fallback: Icon, color: egui::Color32) -> egui::Atom<'static> {
	let symbol = match key {
		"menu-bookmark" | "menu-remove-bookmark" => 0,
		"menu-create-reminder" => 1,
		"menu-report-message" => 2,
		_ => return icons::atom(fallback, 18.0, color),
	};
	egui::Atom::paint(egui::Vec2::splat(18.0), move |ui, args| {
		let rect = args.rect;
		let point = |x, y| rect.min + egui::vec2(x * rect.width(), y * rect.height());
		let stroke = egui::Stroke::new(1.7, color);
		match symbol {
			0 => {
				ui.painter().add(egui::Shape::closed_line(
					vec![
						point(0.25, 0.12),
						point(0.75, 0.12),
						point(0.75, 0.9),
						point(0.5, 0.7),
						point(0.25, 0.9),
					],
					stroke,
				));
			}
			1 => {
				ui.painter().circle_stroke(rect.center(), 7.5, stroke);
				ui.painter().add(egui::Shape::line(
					vec![point(0.5, 0.23), point(0.5, 0.5), point(0.7, 0.65)],
					stroke,
				));
			}
			_ => {
				ui.painter()
					.line_segment([point(0.2, 0.12), point(0.2, 0.92)], stroke);
				ui.painter().add(egui::Shape::convex_polygon(
					vec![
						point(0.2, 0.15),
						point(0.8, 0.15),
						point(0.65, 0.38),
						point(0.8, 0.6),
						point(0.2, 0.6),
					],
					color,
					egui::Stroke::NONE,
				));
			}
		}
	})
}

pub(crate) fn separator(ui: &mut egui::Ui) {
	ui.add_space(6.0);
	ui.separator();
	ui.add_space(6.0);
}

pub(crate) fn quick_button(
	ui: &mut egui::Ui,
	state: &client_core::State,
	emoji: &model::ReactionEmoji,
	images: &mut crate::avatars::Avatars,
) -> egui::Response {
	let colors = design::palette(ui);
	let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(44.0), egui::Sense::click());
	ui.painter().rect_filled(
		rect,
		7,
		if response.hovered() || response.has_focus() {
			colors.hover
		} else {
			colors.chat
		},
	);
	let image = emoji.id.and_then(|id| state.custom_emoji(id));
	let target = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(26.0));
	if let Some((_, custom)) = image {
		if let Some(image) =
			images.custom_emoji(ui.ctx(), custom.id, custom.animated, 26.0, state.demo)
		{
			image.paint_at(ui, target);
		} else {
			icons::paint(ui.painter(), Icon::Smile, target, colors.muted);
		}
	} else if let Some(name) = &emoji.name
		&& let Some(image) = crate::emoji::image(ui.ctx(), name, 26.0)
	{
		image.paint_at(ui, target);
	}
	response.widget_info(|| {
		egui::WidgetInfo::labeled(
			egui::Role::Button,
			ui.is_enabled(),
			emoji.name.as_deref().unwrap_or("Emoji"),
		)
	});
	response
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn action_icons_precede_aligned_labels_and_external_arrows_stay_on_the_right() {
		fn collect(
			shape: &egui::Shape,
			text: &mut Vec<(String, egui::Rect)>,
			images: &mut Vec<egui::Rect>,
		) {
			match shape {
				egui::Shape::Text(value) => text.push((
					value.galley.job.text.clone(),
					value.galley.rect.translate(value.pos.to_vec2()),
				)),
				egui::Shape::Mesh(mesh) => images.push(mesh.calc_bounds()),
				egui::Shape::Vec(shapes) => {
					for shape in shapes {
						collect(shape, text, images);
					}
				}
				_ => {}
			}
		}
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut rows = Vec::new();
		let mut labels = Vec::new();
		let mut images = Vec::new();
		for _ in 0..2 {
			rows.clear();
			labels.clear();
			images.clear();
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(400.0, 400.0),
					)),
					..Default::default()
				},
				|ui| {
					content(ui, |ui| {
						rows.push(row(ui, "menu-copy-text", Icon::Copy, true, false).rect);
						row(ui, "menu-bookmark", Icon::Pin, true, false);
						submenu(ui, "menu-create-reminder", Icon::Calendar, |_ui| {});
						rows.push(row(ui, "menu-open-link", Icon::Globe, true, false).rect);
					});
				},
			);
			for shape in &output.shapes {
				collect(&shape.shape, &mut labels, &mut images);
			}
			output.drop_without_applying_deltas();
		}
		let texts: Vec<_> = [
			"Copy Text",
			"Bookmark Message",
			"Create Reminder",
			"Open Link",
		]
		.map(|label| labels.iter().find(|(text, _)| text == label).unwrap().1)
		.into();
		assert!(
			texts
				.iter()
				.all(|rect| (rect.left() - texts[0].left()).abs() < 1.0),
			"menu labels must share one column: {texts:?}"
		);
		for (row, label) in rows.iter().zip([texts[0], texts[3]]) {
			let icons: Vec<_> = images
				.iter()
				.filter(|rect| (rect.center().y - row.center().y).abs() < 2.0)
				.collect();
			assert!(
				icons.iter().any(|icon| icon.right() < label.left()),
				"missing leading icon for {label:?}: {icons:?}"
			);
			assert!((row.height() - 36.0).abs() < 1.0);
		}
		assert!(
			images
				.iter()
				.any(|icon| (icon.center().y - rows[1].center().y).abs() < 2.0
					&& icon.left() > texts[3].right()),
			"external arrow must stay after the label"
		);
	}

	#[test]
	fn one_active_target_keeps_signed_links_exact_and_never_crosses_messages() {
		let ctx = egui::Context::default();
		let owner = Owner {
			generation: 4,
			channel: Id(20),
			message: Id(777),
		};
		let popup = egui::Id::unique("synthetic-message-menu");
		let url = "https://cdn.discordapp.com/attachments/1/2/clip.mov?ex=abc&hm=123";
		let frame = ctx.cumulative_frame_nr();
		ctx.data_mut(|data| {
			data.insert_temp(
				egui::Id::unique("message-menu-hit"),
				Hit {
					owner,
					frame,
					target: Target::Link(url.into()),
				},
			)
		});
		assert_eq!(
			target(
				&ctx,
				popup,
				owner,
				true,
				false,
				Some((egui::Id::unique("focused-source"), egui::pos2(50.0, 60.0)))
			),
			Some(Target::Link(url.into()))
		);
		assert_eq!(
			target(&ctx, popup, owner, false, false, None),
			Some(Target::Link(url.into()))
		);
		assert_eq!(
			target(
				&ctx,
				popup,
				Owner {
					message: Id(778),
					..owner
				},
				false,
				false,
				None
			),
			None
		);
		assert_eq!(
			target(&ctx, popup, owner, false, true, None),
			None,
			"toolbar overflow does not inherit a link"
		);
	}

	#[test]
	fn channel_and_account_changes_retire_menu_payloads_and_private_drafts() {
		let ctx = egui::Context::default();
		let mut state = test_support::chat_demo_state();
		let mut controls = Controls::default();
		controls.sync(&ctx, &state);
		let owner = Owner {
			generation: state.generation,
			channel: state.selected.unwrap(),
			message: Id(777),
		};
		controls.custom_reminder = Some(owner);
		controls.reminder_text = "2026-12-25 12:00".into();
		target(
			&ctx,
			egui::Id::unique("test-menu"),
			owner,
			true,
			false,
			None,
		);
		controls.record(&model::ReactionEmoji {
			id: None,
			name: Some("🔥".into()),
		});
		state.generation += 1;
		controls.sync(&ctx, &state);
		assert!(
			controls.custom_reminder.is_none()
				&& controls.reminder_text.is_empty()
				&& controls.recent.is_empty()
		);
		assert!(
			ctx.data(|data| data.get_temp::<OpenMenu>(egui::Id::unique("message-menu-active")))
				.is_none()
		);
	}

	#[test]
	fn quick_choices_keep_custom_emoji_use_and_remain_bounded() {
		let mut state = test_support::chat_demo_state();
		let mut message = test_support::message(777, Id(20));
		message.reactions = Some(vec![]);
		state
			.timeline
			.insert(message.clone(), false, false)
			.unwrap();
		let mut controls = Controls::default();
		let custom = model::ReactionEmoji {
			id: Some(Id(9001)),
			name: Some("serein_wave".into()),
		};
		controls.record(&custom);
		controls.record(&model::ReactionEmoji {
			id: None,
			name: Some("🔥".into()),
		});
		let quick = controls.quick(&state, &message, crate::reactions::QUICK_DEFAULTS);
		assert_eq!(quick.len(), 4);
		assert!(
			quick[0].same(&custom),
			"current server's recent emoji stays first"
		);
		for index in 1..=100 {
			controls.record(&model::ReactionEmoji {
				id: Some(Id(10000 + index)),
				name: Some("test".into()),
			});
		}
		assert_eq!(controls.recent.len(), 32);
		let quick = controls.quick(&state, &message, crate::reactions::QUICK_DEFAULTS);
		assert_eq!(quick.len(), 4);
		assert!(
			quick.iter().all(|emoji| emoji.id.is_none()),
			"unknown or removed custom emojis never leak back into the menu"
		);
	}

	#[test]
	fn spoken_text_resolves_mentions_and_excludes_unrevealed_spoilers() {
		let state = test_support::chat_demo_state();
		let mut message = test_support::message(777, Id(20));
		message.content = "Hello <@2> ||private words|| visible".into();
		let parsed = crate::markdown::Formatted::parse(&message.content);
		let hidden = parsed.speech_text(&state, &message, 0);
		assert!(
			hidden.contains("visible")
				&& !hidden.contains("private words")
				&& !hidden.contains("<@2>")
		);
		assert!(
			parsed
				.speech_text(&state, &message, u32::MAX)
				.contains("private words")
		);
	}
}
