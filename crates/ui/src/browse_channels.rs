//! Account channel selection in the main pane; browsing never acknowledges messages.
use crate::{MessagingUi, channel_menu::ChannelMenu, design, icons, shortcuts::ShortcutView};
use client_core::{Command, State, channel_actions::Action, preferences_sync::Status};
use egui::{RichText, Vec2};
use model::Id;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
	#[default]
	All,
	Shown,
	Hidden,
}
impl Filter {
	fn label(self) -> &'static str {
		match self {
			Self::All => "All",
			Self::Shown => "In My Channel List",
			Self::Hidden => "Not In My Channel List",
		}
	}
}
#[derive(Clone, Copy)]
enum Row {
	Category(Option<Id>, usize, usize),
	Channel(Id),
}
impl Row {
	fn identity(self) -> (bool, Option<Id>) {
		match self {
			Self::Category(id, ..) => (true, id),
			Self::Channel(id) => (false, Some(id)),
		}
	}
}
#[derive(PartialEq)]
struct Key {
	revision: u64,
	search: String,
	filter: Filter,
	muted: bool,
	locked: bool,
	mute_deadline: Option<std::time::SystemTime>,
}
#[derive(Default)]
pub(super) struct Browser {
	scope: Option<(u64, Id)>,
	search: String,
	filter: Filter,
	muted_only: bool,
	key: Option<Key>,
	rows: Vec<Row>,
	mentions: BTreeMap<Id, u32>,
	attempted: Vec<Id>,
	import: bool,
	focus_search: bool,
	preferences_requested: bool,
	edit_error: Option<&'static str>,
	heights: Vec<f32>,
	row_height: f32,
	anchor: Option<((bool, Option<Id>), f32)>,
	restore_offset: Option<f32>,
}
impl Browser {
	pub fn open(&mut self, state: &mut State, guild: Id) {
		if !state.open_channel_browser(guild) {
			return;
		}
		self.sync(state, Some(guild));
		self.focus_search = true;
	}
	pub fn sync(&mut self, state: &mut State, active: Option<Id>) {
		let Some(guild) = state.browse_channels else {
			return;
		};
		if state.guild(guild).is_none() || Some(guild) != active {
			state.browse_channels = None;
			state.flush_channel_preferences();
			*self = Self::default();
			return;
		}
		if self.scope != Some((state.generation, guild)) {
			*self = Self {
				scope: Some((state.generation, guild)),
				focus_search: true,
				..Default::default()
			};
		}
	}
	pub fn focus_search(&mut self) {
		self.focus_search = true;
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		active: Option<Id>,
		(view, show_locked): (ShortcutView<'_>, bool),
		menu: &mut ChannelMenu,
		commands: &mut Vec<Command>,
	) {
		self.sync(state, active);
		let Some((generation, guild)) = self.scope else {
			return;
		};
		if generation != state.generation || state.browse_channels != Some(guild) {
			return;
		}
		if !state.channel_preferences_known(guild) && !self.preferences_requested {
			commands.extend(state.refresh_channel_preferences(guild));
			self.preferences_requested = true;
		}
		let available = ui.available_rect_before_wrap();
		ui.set_min_size(available.size());
		let margin = if available.width() < 500.0 {
			16.0
		} else {
			28.0
		};
		let width = (available.width() - margin * 2.0).clamp(120.0, 1000.0);
		let rect = egui::Rect::from_min_max(
			egui::pos2(available.center().x - width * 0.5, available.top() + 20.0),
			egui::pos2(
				available.center().x + width * 0.5,
				available.bottom() - 12.0,
			),
		);
		let mut action = None;
		let mut navigate = None;
		let mut back = false;
		ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
			let colors = design::palette(ui);
			let (header, _) = ui
				.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::hover());
			ui.scope_builder(
				egui::UiBuilder::new()
					.max_rect(egui::Rect::from_min_max(
						header.min,
						egui::pos2(header.right() - 80.0, header.bottom()),
					))
					.layout(egui::Layout::left_to_right(egui::Align::Center)),
				|ui| {
					if icons::button(ui, icons::Icon::ArrowLeft, 32.0, "Back to conversation")
						.clicked()
					{
						back = true;
					}
					ui.add(
						egui::Label::new(
							design::semibold(ui, "Browse Channels", 24.0).color(colors.text_strong),
						)
						.truncate(),
					);
				},
			);
			ui.scope_builder(
				egui::UiBuilder::new()
					.max_rect(egui::Rect::from_min_max(
						egui::pos2(header.right() - 72.0, header.top()),
						header.max,
					))
					.layout(egui::Layout::right_to_left(egui::Align::Center)),
				|ui| {
					let more =
						icons::button(ui, icons::Icon::More, 32.0, "More channel-list options");
					egui::Popup::menu(&more).show(|ui| {
						if ui.button("Import device favorites…").clicked() {
							self.import = true;
							ui.close();
						}
					});
					if icons::button(ui, icons::Icon::Reload, 32.0, "Refresh channel preferences")
						.clicked()
					{
						commands.extend(state.refresh_channel_preferences(guild));
					}
				},
			);
			ui.add_space(6.0);
			ui.add(
				egui::Label::new(
					RichText::new("Choose what appears in your channel list.")
						.size(14.0)
						.color(colors.muted),
				)
				.wrap(),
			);
			ui.add_space(18.0);
			let response = ui.add_sized(
				[ui.available_width(), 36.0],
				egui::TextEdit::singleline(&mut self.search)
					.id_salt(("browse-channel-search", guild))
					.hint_text("Search channels or categories")
					.char_limit(100)
					.margin(egui::vec2(12.0, 8.0)),
			);
			if self.focus_search {
				response.request_focus();
				self.focus_search = false;
			}
			if response.changed() {
				self.anchor = None;
				self.restore_offset = Some(0.0);
			}
			ui.add_space(10.0);
			ui.horizontal_wrapped(|ui| {
				let before = (self.filter, self.muted_only);
				for filter in [Filter::All, Filter::Shown, Filter::Hidden] {
					ui.selectable_value(&mut self.filter, filter, filter.label());
				}
				ui.separator();
				ui.toggle_value(&mut self.muted_only, "Muted");
				if before != (self.filter, self.muted_only) {
					self.anchor = None;
					self.restore_offset = Some(0.0);
				}
			});
			ui.add_space(12.0);
			egui::Frame::new()
				.fill(colors.raised)
				.corner_radius(8)
				.inner_margin(egui::Margin::symmetric(12, 8))
				.show(ui, |ui| {
					// The page's two independent switches need a visible unchecked state.
					ui.visuals_mut().widgets.inactive.bg_stroke =
						egui::Stroke::new(1.0, colors.muted);
					ui.horizontal_wrapped(|ui| {
						let mut all = state.personally_shows_all_channels(guild);
						let enabled = state.personal_channel_controls_available(guild);
						if ui
							.add_enabled(
								enabled,
								egui::Checkbox::new(&mut all, "Show All Channels"),
							)
							.changed()
						{
							action = state.personal_show_all_change(guild, all);
						}
						ui.add_space(16.0);
						let mut muted = state.hides_muted_channels(guild) == Some(true);
						if ui
							.add_enabled(
								enabled && state.hides_muted_channels(guild).is_some(),
								egui::Checkbox::new(&mut muted, "Hide Muted Channels"),
							)
							.changed()
						{
							action = Some(Action::HideMuted(muted));
						}
					});
					if state.hides_muted_channels(guild).is_none() {
						ui.small(
							"The muted-channel preference is unavailable. Refresh to load it.",
						);
					}
					if state.personally_shows_all_channels(guild) {
						ui.label(
							RichText::new(
								"Showing all channels. Your custom list is kept; removing a channel switches back to it.",
							)
							.size(12.0)
							.color(colors.muted),
						);
					}
				});
			preference_feedback(ui, state, guild, None, commands);
			if let Some(error) = self.edit_error {
				ui.horizontal_wrapped(|ui| {
					ui.colored_label(colors.danger, error);
					if ui.small_button("Refresh").clicked() {
						commands.extend(state.refresh_channel_preferences(guild));
						self.edit_error = None;
					}
					if ui.small_button("Dismiss").clicked() {
						self.edit_error = None;
					}
				});
			}
			if !state.channel_preferences_known(guild) {
				ui.add_space(8.0);
				ui.colored_label(
					colors.warning,
					"Channel preferences are not available yet. Refresh to load your saved list.",
				);
			}
			if self.import {
				egui::Frame::new()
					.fill(colors.raised)
					.corner_radius(8)
					.inner_margin(egui::Margin::same(12))
					.show(ui, |ui| {
						ui.label(
							"Add this device’s favorites to your Discord account? Existing account favorites are kept.",
						);
						ui.horizontal(|ui| {
							if ui
								.add_enabled(
									state.personal_channel_controls_available(guild),
									egui::Button::new("Import favorites"),
								)
								.clicked()
							{
								action = state.personal_import_favorites(
									guild,
									view.ids(model::Shortcut::Favorite),
								);
								self.import = false;
							}
							if ui.button("Cancel").clicked() {
								self.import = false;
							}
						});
					});
			}
			ui.add_space(12.0);
			self.prepare_rows(state, guild, show_locked);
			if self.rows.is_empty() {
				ui.add_space(28.0);
				ui.label(design::semibold(ui, "No channels match", 18.0));
				ui.weak("Choose All or clear your search to find a channel.");
				return;
			}
			let body = ui.text_style_height(&egui::TextStyle::Body);
			let stacked = ui.available_width() < 520.0;
			let channel_height = if stacked {
				(body * 6.5).max(104.0)
			} else {
				(body * 3.5).max(60.0)
			};
			if self.heights.len() != self.rows.len() + 1 || self.row_height != channel_height {
				self.heights.clear();
				self.heights.push(0.0);
				for row in &self.rows {
					let height = if let Row::Category(id, ..) = row {
						if id.is_some_and(|id| {
							state.channel_preference_status(guild, Some(id)).is_some()
						}) {
							(body * 5.0).max(80.0)
						} else {
							(body * 2.4).max(40.0)
						}
					} else if let Row::Channel(id) = row
						&& state.channel_preference_status(guild, Some(*id)).is_some()
					{
						if stacked {
							(body * 9.0).max(144.0)
						} else {
							(body * 5.5).max(88.0)
						}
					} else {
						channel_height
					};
					self.heights
						.push(self.heights.last().copied().unwrap_or(0.0) + height);
				}
				self.row_height = channel_height;
				if let Some((identity, offset)) = self.anchor
					&& let Some(index) = self.rows.iter().position(|row| row.identity() == identity)
				{
					self.restore_offset = Some((self.heights[index] - offset).max(0.0));
				}
			}
			let mut scroll = egui::ScrollArea::vertical()
				.id_salt(("browse-channel-rows", guild))
				.auto_shrink([false, false]);
			if let Some(offset) = self.restore_offset.take() {
				scroll = scroll.vertical_scroll_offset(offset);
			}
			let mut description = None;
			scroll.show_viewport(ui, |ui, viewport| {
				let total = self.heights.last().copied().unwrap_or(0.0);
				ui.set_min_height(total);
				let start = self
					.heights
					.partition_point(|offset| *offset <= viewport.top())
					.saturating_sub(1)
					.min(self.rows.len());
				let end = self
					.heights
					.partition_point(|offset| *offset < viewport.bottom())
					.min(self.rows.len());
				if let Some(row) = self.rows.get(start) {
					self.anchor = Some((row.identity(), self.heights[start] - viewport.top()));
				}
				let origin = ui.min_rect().min;
				let width = ui.available_width();
				for index in start..end {
					let rect = egui::Rect::from_min_size(
						origin + egui::vec2(0.0, self.heights[index]),
						egui::vec2(width, self.heights[index + 1] - self.heights[index]),
					);
					// Keep keyboard focus independent of visible-row order and save
					// feedback inserted above the list. Automatic child IDs include
					// their allocation position even when an id_salt is supplied.
					let row_scope = ui.scope_id().with((
						"browse-row",
						state.generation,
						guild,
						self.rows[index].identity(),
					));
					ui.scope_builder(
						egui::UiBuilder::new().scope_id(row_scope).max_rect(rect),
						|ui| match self.rows[index] {
							Row::Category(id, selected, total) => self.category_row(
								ui,
								state,
								guild,
								id,
								selected,
								total,
								view,
								menu,
								&mut action,
								commands,
							),
							Row::Channel(id) => {
								self.channel_row(
									ui,
									state,
									guild,
									id,
									view,
									menu,
									commands,
									&mut action,
									&mut navigate,
								);
								if description.is_none()
									&& state.can_view(id) && state.channel_description(id).is_none()
									&& !self.attempted.contains(&id)
								{
									description = Some(id);
								}
							}
						},
					);
				}
			});
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
		});
		if let Some(action) = action {
			let revision = state.revision;
			commands.extend(state.request_personal_channel_action(guild, action));
			self.edit_error = (state.revision == revision).then_some(
				"This change could not be queued. Refresh your preferences and try again.",
			);
		}
		if let Some(channel) = navigate {
			commands.extend(state.select(channel));
		}
		if back && let Some(step) = state.navigate_back() {
			commands.extend(step.command);
		}
	}
	fn prepare_rows(&mut self, state: &State, guild: Id, show_locked: bool) {
		let key = Key {
			revision: state.channel_list_revision(),
			search: self.search.clone(),
			filter: self.filter,
			muted: self.muted_only,
			locked: show_locked,
			mute_deadline: state.next_mute_deadline(),
		};
		if self.key.as_ref() == Some(&key) {
			return;
		}
		let query = self.search.trim().to_lowercase();
		let mut channels: Vec<_> = state
			.channels
			.iter()
			.filter(|channel| {
				channel.guild == Some(guild)
					&& !matches!(channel.kind, 4 | 10..=12)
					&& (show_locked || state.can_view(channel.id))
					&& (!self.muted_only
						|| state.guild_channel_effectively_muted(channel.id) == Some(true))
					&& (query.is_empty()
						|| channel.name.to_lowercase().contains(&query)
						|| channel
							.parent_id
							.and_then(|id| state.channel(id))
							.is_some_and(|category| category.name.to_lowercase().contains(&query))
						|| state
							.channel_description(channel.id)
							.is_some_and(|description| description.to_lowercase().contains(&query)))
					&& (match self.filter {
						Filter::All => true,
						Filter::Shown => state.channel_in_custom_list(channel.id) == Some(true),
						Filter::Hidden => state.channel_in_custom_list(channel.id) == Some(false),
					} || state
						.channel_preference_status(guild, Some(channel.id))
						.is_some())
			})
			.take(client_core::MAX_NAV)
			.collect();
		channels.sort_unstable_by_key(|channel| {
			(
				channel
					.parent_id
					.and_then(|id| state.channel(id))
					.filter(|category| category.kind == 4 && category.guild == Some(guild))
					.map_or((-1, Id(0)), |category| (category.position, category.id)),
				channel.position,
				channel.id,
			)
		});
		self.rows.clear();
		let mut index = 0;
		while index < channels.len() {
			let parent = channels[index].parent_id.filter(|id| {
				state
					.channel(*id)
					.is_some_and(|category| category.kind == 4 && category.guild == Some(guild))
			});
			let end = index
				+ channels[index..]
					.iter()
					.take_while(|channel| {
						channel.parent_id.filter(|id| {
							state.channel(*id).is_some_and(|category| {
								category.kind == 4 && category.guild == Some(guild)
							})
						}) == parent
					})
					.count();
			let (selected, total) = state
				.channels
				.iter()
				.filter(|channel| {
					channel.guild == Some(guild)
						&& channel.parent_id == parent
						&& !matches!(channel.kind, 4 | 10..=12)
						&& state.can_view(channel.id)
				})
				.fold((0, 0), |(selected, total), channel| {
					(
						selected
							+ usize::from(state.channel_in_custom_list(channel.id) == Some(true)),
						total + 1,
					)
				});
			self.rows.push(Row::Category(parent, selected, total));
			self.rows.extend(
				channels[index..end]
					.iter()
					.map(|channel| Row::Channel(channel.id)),
			);
			index = end;
		}
		self.mentions = state.channel_mention_counts();
		self.heights.clear();
		self.key = Some(key);
	}
	#[allow(clippy::too_many_arguments)]
	fn category_row(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		id: Option<Id>,
		selected: usize,
		total: usize,
		view: ShortcutView<'_>,
		menu: &mut ChannelMenu,
		action: &mut Option<Action>,
		commands: &mut Vec<Command>,
	) {
		let colors = design::palette(ui);
		ui.spacing_mut().icon_width = 20.0;
		ui.horizontal(|ui| {
			if let Some(category) = id.and_then(|id| state.channel(id)) {
				let mut checked = total > 0 && selected == total;
				let response = membership_checkbox(ui,&mut checked,selected>0&&selected<total,state.personal_channel_controls_available(guild)&&state.can_personally_manage_channel(category.id));
				response.widget_info(|| {
					if selected>0&&selected<total {
						egui::WidgetInfo::labeled(egui::Role::CheckBox,response.enabled(),format!("Include {} (partially included)",category.name))
					}else{
						egui::WidgetInfo::selected(egui::Role::CheckBox,response.enabled(),checked,format!("Include {}",category.name))
					}
				});
				if response.changed() {
					*action = state.personal_visibility_change(category.id, checked);
					self.edit_error = action.is_none().then_some("This category could not be changed. Refresh first; large selections are limited to 1,024 entries.");
				}
				let name = ui.allocate_ui_with_layout(
					egui::vec2((ui.available_width()-80.0).max(20.0),ui.text_style_height(&egui::TextStyle::Body)),
					egui::Layout::left_to_right(egui::Align::Center),
					|ui|ui.add(egui::Label::new(design::semibold(ui,&category.name,14.0).color(colors.text)).truncate()),
				).inner;
				menu.context(&name, state, category, view);
			} else {
				ui.add_space(30.0);
				ui.label(design::semibold(ui, "Channels", 14.0).color(colors.text));
			}
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.label(
					RichText::new(format!("{selected} / {total}"))
						.size(12.0)
						.color(colors.muted),
				);
			});
		});
		if let Some(id) = id {
			preference_feedback(ui, state, guild, Some(id), commands);
		}
	}
	#[allow(clippy::too_many_arguments)]
	fn channel_row(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		id: Id,
		view: ShortcutView<'_>,
		menu: &mut ChannelMenu,
		commands: &mut Vec<Command>,
		action: &mut Option<Action>,
		navigate: &mut Option<Id>,
	) {
		let Some(channel) = state.channel(id) else {
			return;
		};
		let allowed = state.can_view(id);
		let membership = state.channel_in_custom_list(id);
		let colors = design::palette(ui);
		let rect = ui.max_rect();
		let hover = ui.interact(
			rect,
			ui.scope_id().with(("browse-hover", id)),
			egui::Sense::hover(),
		);
		if hover.hovered() {
			ui.painter()
				.rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), 6, colors.hover);
		}
		let enabled =
			state.personal_channel_controls_available(guild) && allowed && membership.is_some();
		let mut checked = membership == Some(true);
		let check_rect = egui::Rect::from_center_size(
			egui::pos2(rect.left() + 17.0, rect.top() + 30.0),
			egui::vec2(30.0, 34.0),
		);
		ui.scope_builder(
			egui::UiBuilder::new()
				.max_rect(check_rect)
				.layout(egui::Layout::left_to_right(egui::Align::Center)),
			|ui| {
				ui.spacing_mut().icon_width = 20.0;
				let response = membership_checkbox(ui, &mut checked, false, enabled);
				response.widget_info(|| {
					egui::WidgetInfo::selected(
						egui::Role::CheckBox,
						enabled,
						checked,
						format!("Include {} in my channel list", channel.name),
					)
				});
				if response.changed() {
					*action = state.personal_visibility_change(id, checked);
				self.edit_error = action.is_none().then_some("This channel could not be changed. Refresh first; large selections are limited to 1,024 entries.");
				}
			},
		);
		let stacked = rect.width() < 520.0;
		let status_width = if stacked {
			0.0
		} else {
			(rect.width() * 0.3).clamp(150.0, 230.0)
		};
		let content = egui::Rect::from_min_max(
			egui::pos2(rect.left() + 44.0, rect.top() + 8.0),
			egui::pos2(
				(rect.right() - status_width - 8.0).max(rect.left() + 90.0),
				rect.bottom() - 6.0,
			),
		);
		let response = ui.interact(
			content,
			ui.scope_id().with(("browse-open", id)),
			egui::Sense::click(),
		);
		response
			.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, allowed, &channel.name));
		menu.context(&response, state, channel, view);
		if allowed && response.clicked() {
			*navigate = Some(id);
		}
		let content_response = ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
			ui.horizontal(|ui| {
				let icon = if !allowed {
					icons::Icon::Lock
				} else {
					match channel.kind {
						2 | 13 => icons::Icon::Speaker,
						15 | 16 => icons::Icon::Forum,
						5 => icons::Icon::Megaphone,
						_ => icons::Icon::Hash,
					}
				};
				let (icon_rect, _) =
					ui.allocate_exact_size(Vec2::splat(19.0), egui::Sense::hover());
				icons::paint(ui.painter(), icon, icon_rect, colors.muted);
				ui.add(
					egui::Label::new(design::semibold(ui, &channel.name, 15.0).color(if allowed {
						colors.text_strong
					} else {
						colors.muted
					}))
					.truncate()
					.selectable(false),
				);
			});
			let description = state.channel_description(id).unwrap_or(if allowed {
				""
			} else {
				"Unavailable through your current permissions"
			});
			if !description.is_empty() {
				ui.add(
					egui::Label::new(RichText::new(description).size(12.0).color(colors.muted))
						.truncate()
						.selectable(false),
				);
			}
		});
		let mut details = [None; 3];
		let mut detail_count = 0;
		let mut detail = |text| {
			details[detail_count] = Some(text);
			detail_count += 1;
		};
		if allowed && state.guild_channel_effectively_muted(id) == Some(true) {
			detail(if state.guild_channel_muted(id) == Some(true) {
				"Muted"
			} else {
				"Muted by category"
			});
		}
		if allowed && membership.is_none() {
			detail("Selection unavailable · refresh");
		} else if allowed && membership == Some(false) {
			let reason = if state
				.voice
				.active
				.as_ref()
				.is_some_and(|call| call.channel == id)
			{
				"Visible during your call"
			} else if state.selected == Some(id)
				|| state
					.selected
					.and_then(|id| state.channel(id))
					.is_some_and(|selected| selected.parent_id == Some(id))
			{
				"Visible while open"
			} else if self.mentions.contains_key(&id) {
				"Visible for mentions"
			} else if state.personally_shows_all_channels(guild) {
				"Visible with Show All"
			} else {
				"Not in your list"
			};
			detail(reason);
		} else if membership == Some(true)
			&& channel
				.parent_id
				.and_then(|id| state.personal_channel_flags(id))
				.is_some_and(|flags| flags & model::channel_visibility::OPT_IN_ENABLED != 0)
			&& !state
				.personal_channel_flags(id)
				.is_some_and(|flags| flags & model::channel_visibility::OPT_IN_ENABLED != 0)
		{
			detail("Included by category");
		}
		if state.hides_muted_channels(guild) == Some(true)
			&& state.guild_channel_effectively_muted(id) == Some(true)
		{
			detail("Hide Muted Channels is on");
		}
		let status_rect = egui::Rect::from_min_max(
			if stacked {
				egui::pos2(
					rect.left() + 44.0,
					content_response.response.rect.bottom() + 6.0,
				)
			} else {
				egui::pos2(rect.right() - status_width, rect.top() + 7.0)
			},
			rect.right_bottom() - egui::vec2(2.0, 5.0),
		);
		ui.scope_builder(
			egui::UiBuilder::new()
				.max_rect(status_rect)
				.layout(egui::Layout::top_down(if stacked {
					egui::Align::LEFT
				} else {
					egui::Align::RIGHT
				})),
			|ui| {
				ui.spacing_mut().item_spacing.y = 2.0;
				preference_feedback(ui, state, guild, Some(id), commands);
				for text in details.into_iter().flatten().take(2) {
					ui.add(
						egui::Label::new(RichText::new(text).size(11.0).color(colors.muted))
							.truncate()
							.selectable(false),
					);
				}
			},
		);
	}
}

// Keep native focus/click semantics and accessible identities, with an explicit
// outline so unchecked membership remains visible in both themes.
fn membership_checkbox(
	ui: &mut egui::Ui,
	checked: &mut bool,
	mixed: bool,
	enabled: bool,
) -> egui::Response {
	let enabled = enabled && ui.is_enabled();
	ui.add_enabled_ui(enabled, |ui| {
		let (rect, mut response) =
			ui.allocate_exact_size(egui::vec2(30.0, 32.0), egui::Sense::click());
		if response.clicked() {
			*checked = !*checked;
			response.mark_changed();
		}
		let p = design::palette(ui);
		let marker = egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0));
		let selected = *checked || mixed;
		let fill = if selected {
			p.accent
		} else if response.hovered() {
			p.hover
		} else {
			p.raised
		};
		let opacity = if enabled { 1.0 } else { 0.5 };
		ui.painter()
			.rect_filled(marker, 4, fill.gamma_multiply(opacity));
		ui.painter().rect_stroke(
			marker,
			4,
			egui::Stroke::new(
				1.5,
				if selected { p.accent } else { p.muted }.gamma_multiply(opacity),
			),
			egui::StrokeKind::Inside,
		);
		if mixed {
			ui.painter().hline(
				marker.shrink(5.0).x_range(),
				marker.center().y,
				egui::Stroke::new(2.0, p.accent_text.gamma_multiply(opacity)),
			);
		} else if *checked {
			icons::paint(
				ui.painter(),
				icons::Icon::Check,
				marker.shrink(3.0),
				p.accent_text.gamma_multiply(opacity),
			);
		}
		if response.has_focus() {
			ui.painter().rect_stroke(
				marker.expand(3.0),
				6,
				egui::Stroke::new(1.5, p.accent),
				egui::StrokeKind::Inside,
			);
		}
		response
	})
	.inner
}

pub(super) fn preference_feedback(
	ui: &mut egui::Ui,
	state: &mut State,
	guild: Id,
	channel: Option<Id>,
	commands: &mut Vec<Command>,
) {
	let colors = design::palette(ui);
	match state.channel_preference_status(guild, channel) {
		Some(Status::Saving) => {
			ui.horizontal(|ui| {
				ui.add(egui::Spinner::new().size(12.0));
				ui.label(RichText::new("Saving…").size(12.0).color(colors.muted));
			});
		}
		Some(Status::Failed(error)) => {
			ui.horizontal_wrapped(|ui| {
				ui.colored_label(colors.danger, RichText::new("Not saved").size(12.0))
					.on_hover_text(error);
				if ui.small_button("Retry").clicked() {
					state.retry_channel_preference(guild, channel);
				}
			});
		}
		Some(Status::Conflict) => {
			ui.horizontal_wrapped(|ui| {
				ui.colored_label(
					colors.warning,
					RichText::new("Changed in Discord").size(12.0),
				);
				if ui.small_button("Apply Again").clicked() {
					state.retry_channel_preference(guild, channel);
				}
			});
		}
		Some(Status::Unconfirmed) => {
			ui.horizontal_wrapped(|ui| {
				ui.colored_label(colors.warning, RichText::new("Not confirmed").size(12.0));
				if ui.small_button("Refresh").clicked() {
					commands.extend(state.refresh_channel_preferences(guild));
				}
			});
		}
		None => {}
	}
}
impl MessagingUi {
	#[cfg(feature = "demo")]
	pub fn preview_browse_channels(&mut self, state: &mut State, guild: Id) {
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
		focused: Option<String>,
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
			browser.open(&mut state, Id(10));
			// Description retrieval is independent of the explicit menu action under test.
			browser.attempted = state.channels.iter().map(|channel| channel.id).collect();
			Self {
				state,
				browser,
				menu: ChannelMenu::default(),
				local: Default::default(),
				commands: vec![],
				copied: vec![],
				focused: None,
			}
		}
		fn frame(&mut self, ctx: &egui::Context, events: Vec<Event>) -> Vec<(String, Rect)> {
			self.frame_size(ctx, events, egui::vec2(1100.0, 1100.0))
		}
		fn frame_size(
			&mut self,
			ctx: &egui::Context,
			events: Vec<Event>,
			size: Vec2,
		) -> Vec<(String, Rect)> {
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
					events,
					..Default::default()
				},
				|ui| {
					self.browser.show(
						ui,
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
			for event in &output.platform_output.events {
				if let egui::output::OutputEvent::FocusGained(info) = event {
					self.focused.clone_from(&info.label);
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
		fn key(&mut self, ctx: &egui::Context, key: egui::Key) {
			for pressed in [true, false] {
				self.frame(
					ctx,
					vec![Event::Key {
						key,
						physical_key: None,
						pressed,
						repeat: false,
						modifiers: Modifiers::NONE,
					}],
				);
			}
		}
		fn open_menu(&mut self, ctx: &egui::Context) -> Vec<(String, Rect)> {
			self.frame(ctx, vec![]);
			let labels = self.frame(ctx, vec![]);
			let name = self.state.channel(Id(20)).unwrap().name.clone();
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
		assert!(h.commands.is_empty(), "preference edits are debounced");
		assert!(!h.state.channel_personally_selected(Id(20)));
		assert_eq!(
			h.state.channel_preference_status(Id(10), None),
			Some(Status::Saving),
			"the first hide saves the list-wide customization plus sibling selections"
		);
		h.state.flush_channel_preferences();
		assert!(
			matches!(h.state.poll_channel_preferences(std::time::Instant::now()), Some(Command::ChannelPreferences(request)) if request.guild == Id(10))
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
	#[test]
	fn membership_checkbox_never_navigates_or_acknowledges_and_other_rows_stay_usable() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut h = Harness::new();
		h.frame(&ctx, vec![]);
		let labels = h.frame(&ctx, vec![]);
		let name = &h.state.channel(Id(20)).unwrap().name;
		let text = labels.iter().find(|(label, _)| label == name).unwrap().1;
		let selected = h.state.selected;
		let request = h.state.request;
		let rows_left = text.left() - 71.0;
		h.click(
			&ctx,
			egui::pos2(rows_left + 17.0, text.center().y),
			PointerButton::Primary,
		);
		assert_eq!(h.state.channel_in_custom_list(Id(20)), Some(false));
		assert_eq!(h.state.browse_channels, Some(Id(10)));
		assert_eq!(h.state.selected, selected);
		assert_eq!(h.state.request, request);
		assert!(h.commands.is_empty());
		let labels = h.frame(&ctx, vec![]);
		assert!(labels.iter().any(|(label, _)| label == "Saving…"));
		assert!(
			labels
				.iter()
				.any(|(label, _)| label == "Visible while open")
		);
		h.state.flush_channel_preferences();
		let Command::ChannelPreferences(request) = h
			.state
			.poll_channel_preferences(std::time::Instant::now())
			.unwrap()
		else {
			panic!()
		};
		assert!(
			h.state.personal_channel_controls_available(Id(10)),
			"a pending write does not lock the browser"
		);
		let result = h.state.demo_channel_preferences(&request);
		h.state.apply(client_core::Envelope {
			generation: h.state.generation,
			event: client_core::Event::ChannelPreferences(result),
		});
		assert_eq!(h.state.channel_in_custom_list(Id(20)), Some(false));
	}

	#[test]
	fn selecting_a_channel_leaves_the_page_without_reloading_the_current_conversation() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut h = Harness::new();
		h.frame(&ctx, vec![]);
		let labels = h.frame(&ctx, vec![]);
		let name = &h.state.channel(Id(20)).unwrap().name;
		let rect = labels.iter().find(|(label, _)| label == name).unwrap().1;
		let request = h.state.request;
		h.click(&ctx, rect.center(), PointerButton::Primary);
		assert_eq!(h.state.browse_channels, None);
		assert_eq!(h.state.selected, Some(Id(20)));
		assert_eq!(h.state.request, request);
		assert!(h.commands.is_empty());
		assert!(h.state.navigate_back().unwrap().command.is_none());
		assert_eq!(h.state.browse_channels, Some(Id(10)));
	}

	#[test]
	fn keyboard_membership_survives_row_reflow_and_never_opens_or_acknowledges() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut h = Harness::new();
		h.frame(&ctx, vec![]);
		h.frame(&ctx, vec![]);
		let target = format!(
			"Include {} in my channel list",
			h.state.channel(Id(20)).unwrap().name
		);
		let mut reached = false;
		for step in 1..=24 {
			h.key(&ctx, egui::Key::Tab);
			if h.focused.as_deref() == Some(target.as_str()) {
				eprintln!("First channel checkbox: {step} Tab presses from the search field");
				reached = true;
				break;
			}
		}
		assert!(
			reached,
			"membership must be reachable through ordinary keyboard navigation: {:?}",
			h.focused
		);
		let selected = h.state.selected;
		let request = h.state.request;
		h.key(&ctx, egui::Key::Space);
		assert_eq!(h.state.channel_in_custom_list(Id(20)), Some(false));
		h.key(&ctx, egui::Key::Space);
		assert_eq!(
			h.state.channel_in_custom_list(Id(20)),
			Some(true),
			"the same focused checkbox survives membership/layout changes"
		);
		assert_eq!(h.state.browse_channels, Some(Id(10)));
		assert_eq!(h.state.selected, selected);
		assert_eq!(h.state.request, request);
		assert!(h.commands.is_empty());
	}
	#[test]
	fn filters_use_saved_membership_not_show_all_or_temporary_reveals() {
		let mut h = Harness::new();
		let action = h.state.personal_visibility_change(Id(20), false).unwrap();
		h.state.request_personal_channel_action(Id(10), action);
		h.state.flush_channel_preferences();
		let Command::ChannelPreferences(request) = h
			.state
			.poll_channel_preferences(std::time::Instant::now())
			.unwrap()
		else {
			panic!()
		};
		let result = h.state.demo_channel_preferences(&request);
		h.state.apply(client_core::Envelope {
			generation: h.state.generation,
			event: client_core::Event::ChannelPreferences(result),
		});
		h.browser.filter = Filter::Hidden;
		h.browser.prepare_rows(&h.state, Id(10), false);
		assert!(
			h.browser
				.rows
				.iter()
				.any(|row| matches!(row, Row::Channel(Id(20))))
		);
		assert!(
			!h.browser
				.rows
				.iter()
				.any(|row| matches!(row, Row::Channel(Id(21))))
		);
		let all = h.state.personal_show_all_change(Id(10), true).unwrap();
		h.state.request_personal_channel_action(Id(10), all);
		h.browser.prepare_rows(&h.state, Id(10), false);
		assert!(h.state.channel_personally_selected(Id(20)));
		assert!(
			h.browser
				.rows
				.iter()
				.any(|row| matches!(row, Row::Channel(Id(20))))
		);
	}

	#[test]
	fn descriptions_and_pending_muted_status_do_not_overlap_in_stacked_rows() {
		for light in [false, true] {
			for scale in [1.0, 2.0] {
				let ctx = egui::Context::default();
				design::apply(&ctx);
				ctx.set_zoom_factor(scale);
				ctx.set_visuals(if light {
					egui::Visuals::light()
				} else {
					egui::Visuals::dark()
				});
				let mut h = Harness::new();
				let Some(Command::ChannelAction {
					guild,
					channel,
					request,
					..
				}) = h.state.request_channel_action(Id(20), Action::Description)
				else {
					panic!("description request")
				};
				h.state.apply(client_core::Envelope {
					generation: h.state.generation,
					event: client_core::Event::ChannelAction(
						client_core::channel_actions::Event::Finished {
							guild,
							channel,
							request,
							result: Ok(client_core::channel_actions::Outcome::Description(
								"Description remains readable".into(),
							)),
						},
					),
				});
				h.state.request_channel_action(
					Id(20),
					Action::Mute(client_core::channel_actions::Mute::Forever),
				);
				let size = egui::vec2(500.0, 1100.0);
				h.frame_size(&ctx, vec![], size);
				let labels = h.frame_size(&ctx, vec![], size);
				let description = labels
					.iter()
					.find(|(text, _)| text == "Description remains readable")
					.expect("description")
					.1;
				let muted = labels
					.iter()
					.find(|(text, rect)| text == "Muted" && rect.top() > description.bottom())
					.expect("muted status")
					.1;
				let saving = labels
					.iter()
					.find(|(text, rect)| text == "Saving…" && rect.top() > description.bottom())
					.expect("pending save")
					.1;
				assert!(
					description.bottom() < saving.top(),
					"description and saving overlap at {scale}: {description:?}, {saving:?}"
				);
				assert!(
					saving.bottom() < muted.top(),
					"saving and muted overlap at {scale}: {saving:?}, {muted:?}"
				);
				let next_name = &h.state.channel(Id(21)).expect("next channel").name;
				let next = labels
					.iter()
					.find(|(text, _)| text == next_name)
					.expect("next row")
					.1;
				assert!(
					muted.bottom() < next.top(),
					"status overlaps next row at {scale}: {muted:?}, {next:?}"
				);
			}
		}
	}
	#[test]
	fn main_page_controls_remain_inside_narrow_scaled_viewports() {
		for light in [false, true] {
			for scale in [1.0, 2.0] {
				let ctx = egui::Context::default();
				design::apply(&ctx);
				ctx.set_zoom_factor(scale);
				ctx.set_visuals(if light {
					egui::Visuals::light()
				} else {
					egui::Visuals::dark()
				});
				let mut h = Harness::new();
				let size = egui::vec2(500.0, 900.0);
				h.frame_size(&ctx, vec![], size);
				let labels = h.frame_size(&ctx, vec![], size);
				for text in [
					"Browse Channels",
					"Show All Channels",
					"Hide Muted Channels",
					"Not In My Channel List",
				] {
					let rect = labels
						.iter()
						.find(|(label, _)| label == text)
						.unwrap_or_else(|| panic!("missing {text}: {labels:?}"))
						.1;
					assert!(
						Rect::from_min_size(Pos2::ZERO, size).contains_rect(rect),
						"{text} clips at {scale}: {rect:?}"
					);
				}
			}
		}
	}
	#[test]
	fn main_pane_browsing_keeps_the_sidebar_and_never_acknowledges_the_conversation() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::chat_demo_state();
		let selected = state.selected;
		let request = state.request;
		let mut view = MessagingUi {
			guild: Some(Id(10)),
			..Default::default()
		};
		view.browse_channels.open(&mut state, Id(10));
		let name = state.channel(Id(20)).unwrap().name.clone();
		fn occurrences(shape: &egui::Shape, text: &str) -> usize {
			match shape {
				egui::Shape::Text(value) => usize::from(value.galley.job.text == text),
				egui::Shape::Vec(shapes) => {
					shapes.iter().map(|shape| occurrences(shape, text)).sum()
				}
				_ => 0,
			}
		}
		let mut copies = 0;
		for _ in 0..3 {
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 900.0))),
					..Default::default()
				},
				|ui| {
					let commands = view.show(ui, &mut state);
					assert!(
						!commands.iter().any(|command| matches!(
							command,
							Command::MarkRead { .. }
								| Command::MarkGuildRead { .. }
								| Command::MarkAllRead { .. }
						)),
						"browsing never acknowledges conversation rows"
					);
				},
			);
			let footer = output
				.shapes
				.iter()
				.enumerate()
				.find_map(|(index, shape)| {
					if let egui::Shape::Text(text) = &shape.shape
						&& text.galley.job.text.starts_with("Offline preview")
					{
						Some((index, text.visual_bounding_rect().center()))
					} else {
						None
					}
				})
				.expect("account controls remain on the page");
			for shape in output.shapes.iter().skip(footer.0 + 1) {
				if let egui::Shape::Rect(rect) = &shape.shape {
					assert!(
						!(rect.fill.a() == 255
							&& rect.rect.contains(footer.1)
							&& shape.clip_rect.contains(footer.1)),
						"navigation must not paint over account controls"
					);
				}
			}
			copies = output
				.shapes
				.iter()
				.map(|shape| occurrences(&shape.shape, &name))
				.sum::<usize>();
			output.drop_without_applying_deltas();
		}
		assert_eq!(state.selected, selected);
		assert_eq!(state.request, request);
		assert_eq!(state.browse_channels, Some(Id(10)));
		assert_eq!(view.guild, Some(Id(10)));
		assert!(
			copies >= 2,
			"the channel appears in both the sidebar and browser, not a detached popup"
		);
	}
}
