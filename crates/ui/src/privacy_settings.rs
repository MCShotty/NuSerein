//! Native, service-confirmed Profile Privacy and Content Filters.
use crate::{MessagingUi, design};
use client_core::{Command, State};
use model::{
	messaging_permissions::Change,
	privacy_preferences::{
		Audience, Change as PrivacyChange, ContentKind, Keyword, ProfileVisibility, Redaction,
	},
};

impl MessagingUi {
	pub(super) fn account_preferences_header(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		let nav = &mut self.settings.messaging_permissions;
		if nav.generation != state.generation {
			*nav = crate::messaging_permissions::Navigation {
				generation: state.generation,
				..Default::default()
			};
		}
		if state.messaging_permissions.snapshot.is_none()
			&& !state.messaging_permissions.pending
			&& state.messaging_permissions.error.is_none()
		{
			nav.requested = false;
		}
		if !nav.requested && !state.messaging_permissions.pending {
			if let Some(command) = state.request_messaging_permissions() {
				commands.push(command);
			}
			nav.requested = true;
		}
		if state.messaging_permissions.pending {
			ui.horizontal(|ui| {
				ui.add(egui::Spinner::new().size(14.0));
				ui.label("Updating account preferences…");
			});
		}
		if let Some(error) = state.messaging_permissions.error {
			design::notice(ui, design::Level::Error, error.label());
			if !state.messaging_permissions.pending
				&& design::text_action(ui, "Reload account preferences").clicked()
				&& let Some(command) = state.request_messaging_permissions()
			{
				commands.push(command);
			}
		}
		if let Some(draft) = state
			.messaging_permissions
			.draft
			.as_ref()
			.filter(|_| !state.messaging_permissions.pending)
		{
			let description = match draft {
				Change::Privacy(change) => change_description(*change),
				_ => "Your previous messaging preference change".into(),
			};
			design::notice(
				ui,
				design::Level::Warning,
				&format!(
					"Unsaved change: {description}. Reload to check the current value before applying it again."
				),
			);
			let draft = draft.clone();
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(
						!state.messaging_permissions.reload_required,
						egui::Button::new("Apply saved change"),
					)
					.clicked() && let Some(command) = state.update_messaging_permissions(draft)
				{
					commands.push(command);
				}
				if design::text_action(ui, "Discard saved change").clicked() {
					state.messaging_permissions.draft = None;
				}
			});
		}
	}
	pub(super) fn privacy_settings(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
		content_filters: bool,
	) {
		self.account_preferences_header(ui, state, commands);
		let Some(snapshot) = state.messaging_permissions.snapshot.as_ref() else {
			return;
		};
		let settings = &snapshot.privacy;
		let enabled =
			!state.messaging_permissions.pending && !state.messaging_permissions.reload_required;
		let mut change = None;
		ui.add_enabled_ui(enabled, |ui| {
			if content_filters {
				for (kind, heading, description) in [
					(ContentKind::Sensitive, "Sensitive media", "Choose how Discord handles media it identifies as sensitive."),
					(ContentKind::Gore, "Graphic content", "Choose how Discord handles media it identifies as graphic or violent."),
					(ContentKind::SelfHarm, "Self-harm content", "Choose how Discord handles media it identifies as self-harm content."),
				] {
					design::group(ui, heading, |ui| {
						design::hint(ui, description);
						for (audience, label) in [(Audience::Servers, "In servers"), (Audience::Friends, "DMs from friends"), (Audience::Others, "DMs from non-friends")] {
							let current = settings.content(kind)[audience.index()];
							design::row(ui, label, None, |ui| {
								let mut selected = current;
								egui::ComboBox::from_id_salt(("content-redaction",kind.field(),audience.index())).width(150.0).selected_text(redaction_label(current)).show_ui(ui, |ui| {
									for level in [Redaction::Show, Redaction::Blur, Redaction::Block] { ui.selectable_value(&mut selected, level, redaction_label(level)); }
								});
								if selected != current { change = Some(PrivacyChange::Content { kind, audience, level: selected }); }
							});
						}
					});
				}
				design::group(ui, "Filtered language", |ui| {
					for (keyword, label) in [(Keyword::Profanity, "Profanity"), (Keyword::SexualContent, "Sexual language"), (Keyword::Slurs, "Slurs")] {
						if let Some(enabled) = optional_toggle(ui, label, settings.keywords[keyword.index()]) { change = Some(PrivacyChange::Keyword { keyword, enabled }); }
					}
				});
			} else {
				design::group(ui, "Profile Privacy", |ui| {
					design::hint(ui, "Choose who can see your bio, pronouns, badges, connected accounts and other profile details.");
					if matches!(settings.profile_visibility, ProfileVisibility::Default | ProfileVisibility::Unknown(_)) { ui.weak(format!("Current audience: {}", visibility_label(settings.profile_visibility))); }
					for (value, hint) in [(ProfileVisibility::Friends, "Only your friends can see these profile details."), (ProfileVisibility::FriendsAndSmallServers, "Share with friends and people in your smaller servers."), (ProfileVisibility::FriendsAndAllServers, "Share with friends and people in any of your servers.")] {
						if design::radio_row(ui, settings.profile_visibility == value, visibility_label(value), Some(hint)).clicked() && settings.profile_visibility != value { change = Some(PrivacyChange::ProfileVisibility(value)); }
					}
				});
				design::group(ui, "Privacy preferences", |ui| {
					for (label, current, make) in [
						("Conversation safety warnings", settings.conversation_warnings, PrivacyChange::ConversationWarnings as fn(bool)->PrivacyChange),
						("Show my local time", settings.show_local_time, PrivacyChange::ShowLocalTime),
						("Detect accounts from other platforms", settings.detect_platform_accounts, PrivacyChange::DetectPlatformAccounts),
						("Allow non-spam message data to improve spam detection", settings.non_spam_training, PrivacyChange::NonSpamTraining),
					] { if let Some(value) = optional_toggle(ui, label, current) { change = Some(make(value)); } }
				});
			}
		});
		if let Some(change) = change
			&& let Some(command) = state.update_messaging_permissions(Change::Privacy(change))
		{
			commands.push(command);
		}
	}
	pub(super) fn accessibility_settings(&mut self, ui: &mut egui::Ui) {
		let mut preferences = self.reading_preferences;
		design::group(ui, "Text and display", |ui| {
			self.zoom_row(ui, &mut preferences);
			design::switch(
				ui,
				"Compact messages",
				Some("Use tighter spacing between message groups."),
				&mut preferences.compact_messages,
			);
		});
		design::group(ui, "Motion and scrolling", |ui| {
			design::switch(
				ui,
				"Play animated images and profile artwork",
				Some("Turn off to keep GIFs, animated emoji, avatars and profile artwork still."),
				&mut preferences.animate_gifs,
			);
			design::switch(
				ui,
				"Smooth scrolling",
				Some("Turn off to move directly to each new scroll position."),
				&mut preferences.smooth_scrolling,
			);
			design::slider_row(
				ui,
				"Scroll speed",
				None,
				&mut preferences.scroll_speed_percent,
				25..=300,
				"%",
			);
		});
		if preferences != self.reading_preferences {
			self.apply_reading_preferences(ui.ctx(), preferences);
		}
	}
}

fn optional_toggle(ui: &mut egui::Ui, label: &str, current: Option<bool>) -> Option<bool> {
	if let Some(mut value) = current {
		return design::switch(ui, label, None, &mut value)
			.changed()
			.then_some(value);
	}
	let mut value = None;
	design::row(ui, label, None, |ui| {
		egui::ComboBox::from_id_salt(("optional-privacy-setting", label))
			.width(150.0)
			.selected_text("Discord default")
			.show_ui(ui, |ui| {
				ui.selectable_value(&mut value, Some(true), "Enabled");
				ui.selectable_value(&mut value, Some(false), "Disabled");
			});
	});
	value
}
fn visibility_label(value: ProfileVisibility) -> &'static str {
	match value {
		ProfileVisibility::Default => "Discord default",
		ProfileVisibility::Friends => "Friends only",
		ProfileVisibility::FriendsAndSmallServers => "Friends and small servers",
		ProfileVisibility::FriendsAndAllServers => "Friends and all servers",
		ProfileVisibility::Unknown(_) => "Unsupported audience",
	}
}
fn redaction_label(value: Redaction) -> &'static str {
	match value {
		Redaction::Default => "Discord default",
		Redaction::Show => "Show",
		Redaction::Blur => "Blur",
		Redaction::Block => "Block",
		Redaction::Unknown(_) => "Unsupported setting",
	}
}
fn change_description(change: PrivacyChange) -> String {
	match change {
		PrivacyChange::ProfileVisibility(value) => {
			format!("Profile Privacy — {}", visibility_label(value))
		}
		PrivacyChange::Content {
			kind,
			audience,
			level,
		} => format!(
			"{} · {} — {}",
			match kind {
				ContentKind::Sensitive => "Sensitive media",
				ContentKind::Gore => "Graphic content",
				ContentKind::SelfHarm => "Self-harm content",
			},
			match audience {
				Audience::Servers => "Servers",
				Audience::Friends => "DMs from friends",
				Audience::Others => "DMs from non-friends",
			},
			redaction_label(level)
		),
		PrivacyChange::Keyword { keyword, enabled } => format!(
			"{} filter — {}",
			match keyword {
				Keyword::Profanity => "Profanity",
				Keyword::SexualContent => "Sexual language",
				Keyword::Slurs => "Slurs",
			},
			if enabled { "enabled" } else { "disabled" }
		),
		PrivacyChange::ConversationWarnings(value) => format!(
			"Conversation warnings — {}",
			if value { "enabled" } else { "disabled" }
		),
		PrivacyChange::ShowLocalTime(value) => {
			format!("Local time — {}", if value { "shown" } else { "hidden" })
		}
		PrivacyChange::DetectPlatformAccounts(value) => format!(
			"Platform account detection — {}",
			if value { "enabled" } else { "disabled" }
		),
		PrivacyChange::NonSpamTraining(value) => format!(
			"Non-spam training — {}",
			if value { "enabled" } else { "disabled" }
		),
	}
}
