//! Private own-account details and explicit, service-confirmed connection changes.
use crate::{MessagingUi, design};
use client_core::{Command, State};
use model::{
	Id,
	account_settings::{ConnectionChange, ConnectionKey, ConnectionSetting},
};

#[derive(Default)]
pub(super) struct AccountUi {
	generation: Option<u64>,
	user: Option<Id>,
	email_revealed: bool,
	phone_revealed: bool,
	disconnect: Option<ConnectionKey>,
	opening: Option<String>,
}
impl AccountUi {
	pub(super) fn conceal(&mut self) {
		self.email_revealed = false;
		self.phone_revealed = false;
	}
	fn scope(&mut self, state: &State) {
		if !state.gateway_connected && !state.demo {
			self.conceal();
		}
		let user = state.user.as_ref().map(|user| user.id);
		if self.generation != Some(state.generation) || self.user != user {
			*self = Self {
				generation: Some(state.generation),
				user,
				..Self::default()
			};
		}
	}
}

impl MessagingUi {
	pub(super) fn account_overview_details(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		self.settings.account.scope(state);
		if state.account_settings.overview.is_none()
			&& !state.account_settings.overview_pending
			&& state.account_settings.overview_error.is_none()
			&& let Some(command) = state.request_account_overview()
		{
			commands.push(command);
		}
		if state.account_settings.overview_pending {
			ui.horizontal(|ui| {
				ui.add(egui::Spinner::new().size(14.0));
				ui.label("Loading account details…");
			});
		}
		if let Some(error) = state.account_settings.overview_error {
			design::notice(ui, design::Level::Error, error);
			if !state.account_settings.overview_pending
				&& design::text_action(ui, "Reload account details").clicked()
				&& let Some(command) = state.request_account_overview()
			{
				commands.push(command);
			}
		}
		if let Some(overview) = &state.account_settings.overview {
			design::group(ui, "Account information", |ui| {
				detail_row(ui, "Username", &overview.username);
				design::row(ui, "User ID", None, |ui| {
					ui.horizontal_wrapped(|ui| {
						ui.label(overview.user.id.to_string());
						if design::text_action(ui, "Copy").clicked() {
							ui.ctx().copy_text(overview.user.id.to_string());
						}
					});
				});
				private_row(
					ui,
					"Email",
					overview.email.as_deref(),
					&mut self.settings.account.email_revealed,
				);
				private_row(
					ui,
					"Phone number",
					overview.phone.as_deref(),
					&mut self.settings.account.phone_revealed,
				);
				detail_row(
					ui,
					"Email verification",
					match overview.verified {
						Some(true) => "Verified",
						Some(false) => "Not verified",
						None => "Unavailable",
					},
				);
				detail_row(
					ui,
					"Multi-factor authentication",
					match overview.mfa_enabled {
						Some(true) => "Enabled",
						Some(false) => "Not enabled",
						None => "Unavailable",
					},
				);
				detail_row(
					ui,
					"Subscription",
					match overview.premium_type {
						Some(0) => "No Nitro subscription",
						Some(1) => "Nitro Classic",
						Some(2) => "Nitro",
						Some(3) => "Nitro Basic",
						_ => "Managed in Discord",
					},
				);
			});
		}
		design::group(ui, "Account security and billing", |ui| {
			design::hint(
				ui,
				"Use Discord for password and email changes, multi-factor setup, account recovery, purchases and subscription management.",
			);
			if design::button(ui, "Open official Discord", design::ButtonKind::Outline).clicked() {
				self.settings.account.opening = Some("https://discord.com/app".into());
			}
		});
		crate::markdown::confirm_external_link(ui.ctx(), &mut self.settings.account.opening, true);
	}
	pub(super) fn connection_settings(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		self.settings.account.scope(state);
		if state.account_settings.connections.is_none()
			&& !state.account_settings.connections_pending
			&& state.account_settings.connections_error.is_none()
			&& let Some(command) = state.request_account_connections()
		{
			commands.push(command);
		}
		if state.account_settings.connections_pending {
			ui.horizontal(|ui| {
				ui.add(egui::Spinner::new().size(14.0));
				ui.label(if state.account_settings.connections_saving {
					"Saving connection…"
				} else {
					"Loading connections…"
				});
			});
		}
		if let Some(error) = state.account_settings.connections_error {
			design::notice(ui, design::Level::Error, error);
			if !state.account_settings.connections_pending
				&& design::text_action(ui, "Reload connections").clicked()
				&& let Some(command) = state.request_account_connections()
			{
				commands.push(command);
			}
		}
		let editable = !state.account_settings.connections_pending
			&& !state.account_settings.connections_reload_required;
		let mut change = None;
		if let Some(draft) = state
			.account_settings
			.connection_draft
			.as_ref()
			.filter(|_| !state.account_settings.connections_pending)
		{
			design::notice(
				ui,
				design::Level::Warning,
				&format!(
					"Unsaved change: {}. Reload to check the connection before applying again.",
					connection_change_label(draft)
				),
			);
			let draft = draft.clone();
			let supported = state
				.account_settings
				.connections
				.as_ref()
				.and_then(|connections| connections.find(&draft.key))
				.is_some_and(|connection| draft.supported(connection));
			if !supported && !state.account_settings.connections_reload_required {
				design::hint(
					ui,
					"The saved connection change is no longer available. Discard it to continue.",
				);
			}
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(
						editable && supported,
						egui::Button::new("Apply saved change"),
					)
					.clicked()
				{
					if draft.setting == ConnectionSetting::Remove {
						self.settings.account.disconnect = Some(draft.key);
					} else {
						change = Some(draft);
					}
				}
				if design::text_action(ui, "Discard saved change").clicked() {
					state.account_settings.connection_draft = None;
				}
			});
		}
		if let Some(connections) = &state.account_settings.connections {
			if connections.items.is_empty() {
				design::hint(ui, "You have no connected accounts.");
			}
			for connection in &connections.items {
				ui.push_id((&connection.key.kind, &connection.key.id), |ui| {
					design::card(ui, |ui| {
						ui.add(egui::Label::new(design::semibold(ui, provider_name(&connection.key.kind), 16.0)).wrap());
						ui.add(egui::Label::new(&connection.name).wrap());
						if connection.verified { ui.weak("Verified"); }
						if connection.revoked { design::notice(ui, design::Level::Warning, "This connection needs to be linked again in Discord."); }
						ui.add_enabled_ui(editable && !connection.revoked, |ui| {
							for (label, current, make) in [("Display on profile", connection.visibility.filter(|value| *value <= 1).map(|value| value == 1), ConnectionSetting::DisplayOnProfile as fn(bool)->ConnectionSetting),
								("Display account details on profile", connection.metadata_visibility.filter(|value| *value <= 1).map(|value| value == 1), ConnectionSetting::DisplayMetadata),
								("Display activity as a status", connection.show_activity, ConnectionSetting::DisplayActivity), ("Sync friends", connection.friend_sync, ConnectionSetting::SyncFriends)] {
								if let Some(mut value) = current && design::switch(ui, label, None, &mut value).changed() { change = Some(ConnectionChange { key: connection.key.clone(), setting: make(value) }); }
							}
						});
						if ui.add_enabled(editable, egui::Button::new("Disconnect")).clicked() { self.settings.account.disconnect = Some(connection.key.clone()); }
						if self.settings.account.disconnect.as_ref() == Some(&connection.key) {
							design::notice(ui, design::Level::Warning, "Disconnecting removes this account from your Discord connections and may remove you from servers joined through its integrations.");
							ui.horizontal_wrapped(|ui| {
								ui.add_enabled_ui(editable, |ui| { if design::button(ui, "Confirm disconnect", design::ButtonKind::Danger).clicked() { change = Some(ConnectionChange { key: connection.key.clone(), setting: ConnectionSetting::Remove }); self.settings.account.disconnect = None; } });
								if design::button(ui, "Cancel", design::ButtonKind::Neutral).clicked() { self.settings.account.disconnect = None; }
							});
						}
					});
				});
				ui.add_space(12.0);
			}
		}
		if let Some(change) = change
			&& let Some(command) = state.update_account_connection(change)
		{
			commands.push(command);
		}
		design::group(ui, "Connect another account", |ui| {
			design::hint(
				ui,
				"Link or reauthorize external accounts through official Discord. Reload this page after completing the connection.",
			);
			if design::button(ui, "Open official Discord", design::ButtonKind::Outline).clicked() {
				self.settings.account.opening = Some("https://discord.com/app".into());
			}
		});
		crate::markdown::confirm_external_link(ui.ctx(), &mut self.settings.account.opening, true);
	}
}
fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
	design::row(ui, label, None, |ui| {
		ui.add(egui::Label::new(value).wrap());
	});
}
fn private_row(ui: &mut egui::Ui, label: &str, value: Option<&str>, revealed: &mut bool) {
	design::row(ui, label, None, |ui| {
		ui.horizontal_wrapped(|ui| {
			ui.add(
				egui::Label::new(match value {
					Some(value) if *revealed => value,
					Some(_) => "Hidden",
					None => "Not added",
				})
				.wrap(),
			);
			if value.is_some()
				&& design::text_action(ui, if *revealed { "Hide" } else { "Reveal" }).clicked()
			{
				*revealed = !*revealed;
			}
		});
	});
}
fn provider_name(kind: &str) -> &str {
	match kind {
		"spotify" => "Spotify",
		"github" => "GitHub",
		"twitch" => "Twitch",
		"youtube" => "YouTube",
		"reddit" => "Reddit",
		"steam" => "Steam",
		"xbox" => "Xbox",
		"playstation" | "playstation-stg" => "PlayStation Network",
		"twitter" => "X",
		"battlenet" => "Battle.net",
		"crunchyroll" => "Crunchyroll",
		"epicgames" => "Epic Games",
		"tiktok" => "TikTok",
		"instagram" => "Instagram",
		kind => kind,
	}
}
fn connection_change_label(change: &ConnectionChange) -> String {
	let (setting, value) = match change.setting {
		ConnectionSetting::DisplayOnProfile(value) => ("profile visibility", Some(value)),
		ConnectionSetting::DisplayMetadata(value) => ("profile details", Some(value)),
		ConnectionSetting::DisplayActivity(value) => ("activity display", Some(value)),
		ConnectionSetting::SyncFriends(value) => ("friend sync", Some(value)),
		ConnectionSetting::Remove => ("disconnect", None),
	};
	format!(
		"{} · {setting}{}",
		provider_name(&change.key.kind),
		value.map_or(String::new(), |value| if value {
			" — enabled".into()
		} else {
			" — disabled".into()
		})
	)
}
