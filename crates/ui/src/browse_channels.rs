//! A recoverable, account-scoped channel selector. Browsing never acknowledges messages.
use crate::{MessagingUi, channel_menu::ChannelMenu, design, shortcuts::ShortcutView};
use client_core::{Command, State, channel_actions::Action};
use egui::RichText;
use model::Id;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
	#[default]
	All,
	Shown,
	Hidden,
}
#[derive(Clone, Copy)]
enum Row {
	Category(Id),
	Channel(Id),
}
#[derive(Default)]
pub(super) struct Browser {
	scope: Option<(u64, Id)>,
	open: bool,
	search: String,
	filter: Filter,
	key: Option<(u64, String, Filter, bool)>,
	rows: Vec<Row>,
	attempted: Vec<Id>,
	import: bool,
}
impl Browser {
	pub fn open(&mut self, state: &State, guild: Id) {
		if self.scope != Some((state.generation, guild)) {
			*self = Self::default();
		}
		self.scope = Some((state.generation, guild));
		self.open = true;
	}
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		active: Option<Id>,
		(view, show_locked): (ShortcutView<'_>, bool),
		menu: &mut ChannelMenu,
		commands: &mut Vec<Command>,
	) {
		let Some((generation, guild)) = self.scope else {
			return;
		};
		if generation != state.generation || Some(guild) != active || state.guild(guild).is_none() {
			*self = Self::default();
			return;
		}
		if !self.open {
			return;
		}
		let mut open = self.open;
		let mut action = None;
		let mut navigate = None;
		egui::Window::new("Browse Channels")
			.id(egui::Id::unique(("browse-channels", generation, guild)))
			.open(&mut open)
			.default_size(egui::vec2(700.0, 560.0))
			.min_width(280.0)
			.resizable(true)
			.show(ctx, |ui| {
				(action, navigate) =
					self.contents(ui, state, guild, (view, show_locked), menu, commands);
			});
		self.open = open;
		if let Some(action) = action
			&& let Some(command) = state.request_personal_channel_action(guild, action)
		{
			commands.push(command);
		}
		if let Some(channel) = navigate {
			self.open = false;
			commands.extend(state.select(channel));
		}
	}
	fn contents(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		(view, show_locked): (ShortcutView<'_>, bool),
		menu: &mut ChannelMenu,
		commands: &mut Vec<Command>,
	) -> (Option<Action>, Option<Id>) {
		let mut action = None;
		let mut navigate = None;
		ui.label("Choose the channels in your sidebar. Hidden channels remain available here.");
		let available = state.personal_channel_controls_available(guild);
		ui.horizontal_wrapped(|ui| {
			let mut all = state.personally_shows_all_channels(guild);
			if ui
				.add_enabled(
					available,
					egui::Checkbox::new(&mut all, "Show All Channels"),
				)
				.changed()
			{
				action = state.personal_show_all_change(guild, all);
			}
			if ui
				.add_enabled(available, egui::Button::new("Import device favorites…"))
				.clicked()
			{
				self.import = true;
			}
		});
		if self.import {
			ui.group(|ui| {
				ui.label("Add this device’s existing favorites to your Discord account? Existing account favorites are kept.");
				ui.horizontal(|ui| {
					if ui.add_enabled(available, egui::Button::new("Import favorites")).clicked() {
						action = state.personal_import_favorites(guild, view.ids(model::Shortcut::Favorite));
						self.import = false;
					}
					if ui.button("Cancel").clicked() {
						self.import = false;
					}
				});
			});
		}
		ui.horizontal_wrapped(|ui| {
			ui.add(
				egui::TextEdit::singleline(&mut self.search)
					.hint_text("Search channels")
					.char_limit(100)
					.desired_width(260.0),
			);
			for (filter, label) in [
				(Filter::All, "All"),
				(Filter::Shown, "Shown"),
				(Filter::Hidden, "Hidden"),
			] {
				ui.selectable_value(&mut self.filter, filter, label);
			}
		});
		if !available && !state.channel_action_pending() {
			ui.weak(
				"Channel preferences are unavailable. Reconnect to load your account settings.",
			);
		}
		let key = (
			state.channel_list_revision(),
			self.search.clone(),
			self.filter,
			show_locked,
		);
		if self.key.as_ref() != Some(&key) {
			let query = self.search.trim().to_lowercase();
			let mut channels: Vec<_> =
				state
					.channels
					.iter()
					.filter(|channel| {
						channel.guild == Some(guild)
							&& !matches!(channel.kind, 4 | 10..=12)
							&& (show_locked || state.can_view(channel.id))
							&& (query.is_empty()
								|| channel.name.to_lowercase().contains(&query)
								|| state.channel_description(channel.id).is_some_and(
									|description| description.to_lowercase().contains(&query),
								)) && match self.filter {
							Filter::All => true,
							Filter::Shown => state.channel_personally_selected(channel.id),
							Filter::Hidden => !state.channel_personally_selected(channel.id),
						}
					})
					.take(client_core::MAX_NAV)
					.collect();
			channels.sort_unstable_by_key(|channel| {
				(
					channel
						.parent_id
						.and_then(|id| state.channel(id))
						.map_or((-1, Id(0)), |category| (category.position, category.id)),
					channel.position,
					channel.id,
				)
			});
			self.rows.clear();
			let mut parent = None;
			for channel in channels {
				if channel.parent_id != parent {
					parent = channel.parent_id;
					if let Some(parent) = parent.filter(|id| {
						state
							.channel(*id)
							.is_some_and(|c| c.kind == 4 && c.guild == Some(guild))
					}) {
						self.rows.push(Row::Category(parent));
					}
				}
				self.rows.push(Row::Channel(channel.id));
			}
			self.key = Some(key);
		}
		ui.separator();
		if self.rows.is_empty() {
			ui.label("No channels match this filter. Choose All or clear your search.");
		}
		let mut description = None;
		egui::ScrollArea::vertical()
			.id_salt(("browse-channel-rows", guild))
			.max_height((ui.available_height() - 35.0).max(180.0))
			.show_rows(ui, 58.0, self.rows.len(), |ui, range| {
				for index in range {
					let id = match self.rows[index] {
						Row::Category(id) | Row::Channel(id) => id,
					};
					ui.push_id(("browse-channel", id), |ui| match self.rows[index] {
						Row::Category(id) => {
							if let Some(category) = state.channel(id) {
								let row = ui.horizontal(|ui| {
									ui.strong(&category.name);
									for (label, shown) in
										[("Show category", true), ("Hide category", false)]
									{
										if ui
											.add_enabled(
												available
													&& state.can_personally_manage_channel(id),
												egui::Button::new(label).small(),
											)
											.clicked()
										{
											action = state.personal_visibility_change(id, shown);
										}
									}
								});
								menu.context(&row.response, state, category, view);
							}
						}
						Row::Channel(id) => {
							if let Some(channel) = state.channel(id) {
								let allowed = state.can_view(id);
								let mut shown = state.channel_personally_selected(id);
								ui.horizontal(|ui| {
									if ui
										.add_enabled(
											available && allowed,
											egui::Checkbox::new(&mut shown, ""),
										)
										.on_hover_text("Show this channel in your sidebar")
										.changed()
									{
										action = state.personal_visibility_change(id, shown);
									}
									ui.vertical(|ui| {
										let label = format!(
											"{} {}",
											if matches!(channel.kind, 2 | 13) {
												"◖"
											} else {
												"#"
											},
											channel.name
										);
										let response = ui.add_enabled(
											allowed,
											egui::Button::new(RichText::new(label).strong())
												.frame(false),
										);
										menu.context(&response, state, channel, view);
										if response.clicked() {
											navigate = Some(id);
										}
										if !allowed {
											ui.small(
												"Unavailable through your current permissions",
											);
										} else if state.guild_channel_effectively_muted(id)
											== Some(true)
										{
											ui.small(
												if state.hides_muted_channels(guild) == Some(true) {
													"Muted · filtered by Hide Muted Channels"
												} else {
													"Muted"
												},
											);
										} else if let Some(value) = state
											.channel_description(id)
											.filter(|value| !value.is_empty())
										{
											ui.add(egui::Label::new(value).truncate());
										} else if !shown {
											ui.small("Hidden from your sidebar");
										}
									});
								});
								if allowed
									&& description.is_none() && state
									.channel_description(id)
									.is_none() && !self.attempted.contains(&id)
								{
									description = Some(id);
								}
							}
						}
					});
				}
			});
		if state.channel_action_pending() {
			ui.horizontal(|ui| {
				ui.spinner();
				ui.weak(if state.channel_description_pending() {
					"Loading channel information…"
				} else {
					"Updating channel preferences…"
				});
			});
		}
		if let Some((_, message, false)) = state
			.channels
			.iter()
			.filter(|c| c.guild == Some(guild))
			.find_map(|c| {
				state
					.channel_action_status(c.id)
					.map(|message| (c.id, message, state.channel_action_succeeded(c.id)))
			}) {
			ui.colored_label(design::palette(ui).danger, message);
			if ui.button("Retry loading descriptions").clicked() {
				self.attempted.clear();
			}
		}
		if action.is_none()
			&& !state.channel_action_pending()
			&& let Some(id) = description
			&& let Some(command) = state.request_channel_action(id, Action::Description)
		{
			if self.attempted.len() >= 64 {
				self.attempted.remove(0);
			}
			self.attempted.push(id);
			commands.push(command);
		}
		(action, navigate)
	}
}
impl MessagingUi {
	#[cfg(feature = "demo")]
	pub fn preview_browse_channels(&mut self, state: &State, guild: Id) {
		self.guild = Some(guild);
		self.browse_channels.open(state, guild);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
	struct Harness {
		state: State,
		browser: Browser,
		menu: ChannelMenu,
		local: model::ChannelPreferences,
		commands: Vec<Command>,
		copied: Vec<String>,
	}
	impl Harness {
		fn new() -> Self {
			crate::i18n::set_current(crate::i18n::Language::English);
			let mut state = test_support::chat_demo_state();
			state
				.permissions
				.replace(test_support::permission_snapshot(&state))
				.unwrap();
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::NotificationPreferences(
					client_core::notifications::Event::Settings {
						entries: vec![client_core::notifications::Setting {
							guild: Some(Id(10)),
							flags: Some(0),
							overrides_known: true,
							..Default::default()
						}],
						replace: false,
					},
				),
			});
			let mut browser = Browser::default();
			browser.open(&state, Id(10));
			// Description retrieval is independent of the explicit menu action under test.
			browser.attempted = state.channels.iter().map(|channel| channel.id).collect();
			Self {
				state,
				browser,
				menu: ChannelMenu::default(),
				local: Default::default(),
				commands: vec![],
				copied: vec![],
			}
		}
		fn frame(&mut self, ctx: &egui::Context, events: Vec<Event>) -> Vec<(String, Rect)> {
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 1100.0))),
					events,
					..Default::default()
				},
				|ui| {
					self.browser.show(
						ui.ctx(),
						&mut self.state,
						Some(Id(10)),
						(ShortcutView::new(&self.local, true), false),
						&mut self.menu,
						&mut self.commands,
					);
					self.menu.show(
						ui.ctx(),
						&mut self.state,
						Some(Id(10)),
						&mut crate::avatars::Avatars::default(),
						&mut self.commands,
					);
				},
			);
			fn text(shape: &egui::Shape, labels: &mut Vec<(String, Rect)>) {
				match shape {
					egui::Shape::Text(value) => labels.push((
						value.galley.job.text.clone(),
						value.galley.rect.translate(value.pos.to_vec2()),
					)),
					egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| text(shape, labels)),
					_ => {}
				}
			}
			let mut labels = vec![];
			for shape in &output.shapes {
				text(&shape.shape, &mut labels);
			}
			for command in &output.platform_output.commands {
				if let egui::OutputCommand::CopyText(value) = command {
					self.copied.push(value.clone());
				}
			}
			output.drop_without_applying_deltas();
			labels
		}
		fn click(&mut self, ctx: &egui::Context, position: Pos2, button: PointerButton) {
			for pressed in [true, false] {
				self.frame(
					ctx,
					vec![
						Event::PointerMoved(position),
						Event::PointerButton {
							pos: position,
							button,
							pressed,
							modifiers: Modifiers::NONE,
						},
					],
				);
			}
		}
		fn open_menu(&mut self, ctx: &egui::Context) -> Vec<(String, Rect)> {
			self.frame(ctx, vec![]);
			let labels = self.frame(ctx, vec![]);
			let name = format!("# {}", self.state.channel(Id(20)).unwrap().name);
			let row = labels.iter().find(|(label, _)| label == &name).unwrap().1;
			self.click(ctx, row.center(), PointerButton::Secondary);
			self.frame(ctx, vec![])
		}
	}
	#[test]
	fn browse_rows_reuse_personal_menus_and_dispatch_the_exact_channel_once() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut h = Harness::new();
		let selected = h.state.selected;
		let labels = h.open_menu(&ctx);
		for key in [
			"channel-menu-context-mark-as-read",
			"channel-menu-context-add-to-favorites",
			"channel-menu-context-mute-channel",
			"channel-menu-context-notification-settings",
			"channel-menu-context-copy-channel-id",
		] {
			let label = crate::i18n::translate(key);
			assert!(
				labels.iter().any(|(text, _)| text == &label),
				"missing {label}"
			);
		}
		let copy = crate::i18n::translate("channel-menu-context-copy-link");
		let position = labels
			.iter()
			.find(|(label, _)| label == &copy)
			.unwrap()
			.1
			.center();
		h.click(&ctx, position, PointerButton::Primary);
		assert_eq!(h.copied, ["https://discord.com/channels/10/20"]);
		assert_eq!(h.state.selected, selected);
		assert!(h.commands.is_empty());
		let labels = h.open_menu(&ctx);
		let position = labels
			.iter()
			.find(|(label, _)| label == "Hide Channel")
			.unwrap()
			.1
			.center();
		h.click(&ctx, position, PointerButton::Primary);
		assert!(
			matches!(h.commands.as_slice(),[Command::ChannelAction {guild:Id(10),channel:Id(20),action:Action::Personal(change),..}] if change.guild_flags==Some((0,model::channel_visibility::OPT_IN_CHANNELS_ON)))
		);
		assert_eq!(h.state.selected, selected);
	}
	#[test]
	fn browse_menu_rechecks_channel_access_before_hiding() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut h = Harness::new();
		let labels = h.open_menu(&ctx);
		let position = labels
			.iter()
			.find(|(label, _)| label == "Hide Channel")
			.unwrap()
			.1
			.center();
		h.state.permissions.channels.remove(&Id(20));
		h.state.permissions.clear_cache();
		assert!(!h.state.can_view(Id(20)));
		h.click(&ctx, position, PointerButton::Primary);
		assert!(h.commands.is_empty());
	}
}
