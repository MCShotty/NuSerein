//! One session-only draft; saving changes only the controls the user touched.
use crate::{design, dialog, i18n::translate, icons};
use client_core::{
	Command, State,
	channel_actions::{Action, Mute},
	server_actions::NotificationOptions,
};
use model::Id;

#[derive(Default)]
pub(super) struct Editor {
	edits: NotificationOptions,
	submitted: bool,
	mute_duration: Option<Mute>,
	channel: Option<Id>,
	query: String,
}

pub(super) fn mute_label(mute: Mute) -> &'static str {
	match mute {
		Mute::For(900) => "channel-menu-report-capacity-for-15-minutes",
		Mute::For(3600) => "channel-menu-report-capacity-for-1-hour",
		Mute::For(10800) => "channel-menu-report-capacity-for-3-hours",
		Mute::For(28800) => "channel-menu-report-capacity-for-8-hours",
		Mute::For(86400) => "channel-menu-report-capacity-for-24-hours",
		_ => "channel-menu-context-until-i-turn-it-back-on",
	}
}

pub(super) fn mute_menu(ui: &mut egui::Ui, label: &str) -> Option<Mute> {
	let mut selected = None;
	ui.menu_button(crate::i18n::translate_if_key(label), |ui| {
		for mute in Mute::DURATIONS
			.into_iter()
			.map(Mute::For)
			.chain([Mute::Forever])
		{
			if ui.button(translate(mute_label(mute))).clicked() {
				selected = Some(mute);
				ui.close();
			}
		}
	});
	selected
}
impl Editor {
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		guild: Id,
		name: &str,
		commands: &mut Vec<Command>,
	) -> bool {
		let current = state.server_notification_settings(guild);
		let pending = state.server_action_pending();
		if self.submitted && !pending {
			// Show the reconciled account state after completion, including a newer
			// Gateway update. Errors remain visible; another write needs a new choice.
			self.submitted = false;
			self.edits = NotificationOptions::default();
		}
		let available = state.can_update_server_notifications(guild);
		let mut save = false;
		let mut close = false;
		let response = dialog::Dialog::new(
			"server-notification-settings",
			translate("server-notifications-title"),
		)
		.subtitle(name)
		.icon(icons::Icon::Bell)
		.width(440.0)
		.show(ctx, |d| {
			let max_height = (d.available_height() - 220.0).max(100.0);
			d.content(|ui| {
				egui::ScrollArea::vertical()
					.max_height(max_height)
					.show(ui, |ui| {
						ui.add_enabled_ui(available && !pending, |ui| {
							let level = self.edits.level.or(current.level);
							let default = match state
								.guild(guild)
								.and_then(|g| g.default_message_notifications)
							{
								Some(0) => "server-notifications-default-all",
								Some(1) => "server-notifications-default-mentions",
								_ => "server-notifications-default-unknown",
							};
							for (value, label) in [
								(3, "server-notifications-default"),
								(0, "server-notifications-all"),
								(1, "server-notifications-mentions"),
								(2, "server-notifications-nothing"),
							] {
								if design::radio_row(
									ui,
									level == Some(value),
									label,
									(value == 3).then_some(default),
								)
								.clicked()
								{
									self.edits.level = Some(value);
								}
							}
							design::card_divider(ui);
							for (label, original, edit) in [
								(
									"server-notifications-mute",
									current.muted,
									&mut self.edits.muted,
								),
								(
									"server-notifications-everyone",
									current.suppress_everyone,
									&mut self.edits.suppress_everyone,
								),
								(
									"server-notifications-roles",
									current.suppress_roles,
									&mut self.edits.suppress_roles,
								),
							] {
								let mut value = edit.or(original).unwrap_or(false);
								if design::switch(ui, label, None, &mut value).changed() {
									*edit = Some(value);
								}
							}
						});
						if self.edits.muted.or(current.muted) == Some(true) {
							ui.add_enabled_ui(available && !pending, |ui| {
								ui.label(translate("qol-mute-duration"));
								let label = self.mute_duration.map(mute_label).unwrap_or(
									if current.mute_until.is_some() && self.edits.muted.is_none() {
										"qol-current-mute"
									} else {
										mute_label(Mute::Forever)
									},
								);
								egui::ComboBox::from_id_salt("server-mute-duration")
									.selected_text(translate(label))
									.show_ui(ui, |ui| {
										for mute in Mute::DURATIONS
											.into_iter()
											.map(Mute::For)
											.chain([Mute::Forever])
										{
											if ui
												.selectable_label(
													self.mute_duration == Some(mute),
													translate(mute_label(mute)),
												)
												.clicked()
											{
												self.mute_duration = Some(mute);
												self.edits.muted = Some(true);
												ui.close();
											}
										}
									});
							});
						}
						self.override_controls(ui, state, guild, commands);
						if current.level.is_none() || current.muted.is_none() {
							dialog::notice(
								ui,
								dialog::Level::Warning,
								"server-notifications-unknown",
							);
						}
						if !available && !pending {
							dialog::notice(
								ui,
								dialog::Level::Warning,
								"server-notifications-offline",
							);
						}
						if let Some(status) = state.server_action_status(guild) {
							dialog::hint(ui, status);
						}
						if state.demo {
							dialog::hint(ui, "server-menu-show-offline-preview-no-server-changes");
						}
					});
			});
			d.footer(|ui| {
				ui.add_enabled_ui(
					available && !pending && self.edits != NotificationOptions::default(),
					|ui| {
						save = dialog::action(
							ui,
							if pending {
								"server-notifications-saving"
							} else {
								"server-notifications-save"
							},
							dialog::Action::Primary,
						)
						.clicked();
					},
				);
				close =
					dialog::action(ui, "server-menu-show-close", dialog::Action::Neutral).clicked();
			});
		});
		let options = if self.edits.muted == Some(true) {
			self.edits
				.with_mute(self.mute_duration.unwrap_or(Mute::Forever))
				.unwrap_or(self.edits)
		} else {
			self.edits
		};
		if save && let Some(command) = state.update_server_notifications(guild, options) {
			commands.push(command);
			self.submitted = true;
		}
		close || response.close
	}
	fn override_controls(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		commands: &mut Vec<Command>,
	) {
		design::card_divider(ui);
		ui.label(translate("qol-notification-overrides"));
		dialog::hint(ui, "qol-choose-channel-category");
		if self.channel.is_some_and(|id| {
			state.channel(id).is_none_or(|c| c.guild != Some(guild)) || !state.can_view(id)
		}) {
			self.channel = None;
		}
		let selected = self
			.channel
			.and_then(|id| state.channel(id))
			.map(|c| c.name.as_str())
			.unwrap_or("");
		egui::ComboBox::from_id_salt("notification-override-channel")
			.selected_text(if selected.is_empty() {
				translate("qol-select-channel")
			} else {
				selected.into()
			})
			.show_ui(ui, |ui| {
				ui.add(
					egui::TextEdit::singleline(&mut self.query)
						.char_limit(100)
						.hint_text(translate("qol-search-channels")),
				);
				let query = self.query.to_lowercase();
				let mut matches = state.channels.iter().filter(|c| {
					c.guild == Some(guild)
						&& matches!(c.kind, 0 | 2 | 4 | 5 | 15 | 16)
						&& state.can_view(c.id)
						&& c.name.to_lowercase().contains(&query)
				});
				for channel in matches.by_ref().take(50) {
					if ui
						.selectable_value(&mut self.channel, Some(channel.id), &channel.name)
						.clicked()
					{
						ui.close();
					}
				}
				if matches.next().is_some() {
					ui.small(translate("qol-more-channels"));
				}
			});
		let Some(channel) = self.channel else { return };
		let available = !state.channel_action_pending()
			&& !state.server_action_pending()
			&& (state.demo || state.gateway_connected)
			&& state.can_view(channel);
		let mut action = None;
		ui.add_enabled_ui(available, |ui| {
			let level = state.channel_notification_level(channel).unwrap_or(3);
			for (value, label) in [
				(3, "qol-use-inherited"),
				(0, "server-notifications-all"),
				(1, "server-notifications-mentions"),
				(2, "server-notifications-nothing"),
			] {
				if ui.radio(level == value, translate(label)).clicked() {
					action = Some(Action::Notifications(value));
				}
			}
			if state.guild_channel_muted(channel) == Some(true)
				&& ui.button(translate("qol-unmute-selection")).clicked()
			{
				action = Some(Action::Mute(Mute::Unmute));
			}
			if let Some(mute) = mute_menu(ui, "qol-mute-selection") {
				action = Some(Action::Mute(mute));
			}
		});
		if let Some(action) = action
			&& let Some(command) = state.request_channel_action(channel, action)
		{
			commands.push(command);
		}
		if let Some(status) = state.channel_action_status(channel) {
			ui.small(crate::i18n::translate_if_key(status));
		}
	}
}
