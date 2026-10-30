//! Explicit refresh controls; the desktop owns every request, checkpoint and cache write.
use crate::{MessagingUi, design, dialog, i18n};
use model::Id;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
	Server(Id),
	Account,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
	Start(Scope),
	Pause,
	Resume,
	Cancel,
	Retry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
	Running,
	Paused,
	Waiting,
	Complete,
	Cancelled,
}
impl Status {
	pub fn active(self) -> bool {
		matches!(self, Self::Running | Self::Paused | Self::Waiting)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
	Server,
	Roles,
	Channels,
	Members,
	Expressions,
	Preferences,
	History,
}
impl Stage {
	pub const ALL: [Self; 7] = [
		Self::Server,
		Self::Roles,
		Self::Channels,
		Self::Members,
		Self::Expressions,
		Self::Preferences,
		Self::History,
	];
	fn label(self) -> &'static str {
		match self {
			Self::Server => "force-sync-stage-server",
			Self::Roles => "force-sync-stage-roles",
			Self::Channels => "force-sync-stage-channels",
			Self::Members => "force-sync-stage-members",
			Self::Expressions => "force-sync-stage-expressions",
			Self::Preferences => "force-sync-stage-preferences",
			Self::History => "force-sync-stage-history",
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
	Updated,
	Partial,
	Unavailable,
	Failed,
}
impl Outcome {
	fn label(self) -> &'static str {
		match self {
			Self::Updated => "force-sync-outcome-updated",
			Self::Partial => "force-sync-outcome-partial",
			Self::Unavailable => "force-sync-outcome-unavailable",
			Self::Failed => "force-sync-outcome-failed",
		}
	}
}

#[derive(Clone, Debug)]
pub struct Section {
	pub stage: Stage,
	pub outcome: Outcome,
	/// Fixed, redacted status text or a localization key; never remote error text.
	pub detail: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct Progress {
	pub scope: Scope,
	pub status: Status,
	pub stage: Stage,
	pub channels_done: u64,
	pub channels_total: u64,
	pub messages_saved: u64,
	pub cache_bytes: u64,
	/// At most one result per stage; rendering also enforces that bound.
	pub sections: Vec<Section>,
	pub detail: Option<&'static str>,
	pub can_retry: bool,
}

#[derive(Clone, Debug)]
pub struct CacheSummary {
	pub total_bytes: u64,
	pub history_bytes: Option<u64>,
	pub metadata_bytes: Option<u64>,
	pub media_bytes: Option<u64>,
	pub artwork_bytes: Option<u64>,
	pub last_cleanup: Option<&'static str>,
}

impl MessagingUi {
	/// Offline fixture only; this does not enqueue a request or create a sync job.
	#[cfg(any(test, feature = "demo"))]
	pub fn preview_force_sync(&mut self) {
		self.force_sync_action = None;
		self.force_sync_open = true;
		self.force_sync_cache = Some(CacheSummary {
			total_bytes: 1_500_000_000,
			history_bytes: Some(900_000_000),
			metadata_bytes: Some(20_000_000),
			media_bytes: Some(450_000_000),
			artwork_bytes: Some(130_000_000),
			last_cleanup: Some("force-sync-cache-cleaned"),
		});
		self.force_sync_progress = Some(Progress {
			scope: Scope::Account,
			status: Status::Running,
			stage: Stage::History,
			channels_done: 19,
			channels_total: 75,
			messages_saved: 12_345,
			cache_bytes: 1_500_000_000,
			sections: Stage::ALL[..6]
				.iter()
				.map(|stage| Section {
					stage: *stage,
					outcome: if *stage == Stage::Members {
						Outcome::Partial
					} else {
						Outcome::Updated
					},
					detail: if *stage == Stage::Members {
						Some("force-sync-members-partial")
					} else {
						None
					},
				})
				.collect(),
			detail: Some("force-sync-demo"),
			can_retry: false,
		});
	}

	/// Opening progress never replaces an active job, including one paused by the owner.
	pub fn open_force_sync(&mut self, scope: Scope) {
		self.force_sync_open = true;
		if self
			.force_sync_progress
			.as_ref()
			.is_none_or(|progress| !progress.status.active())
			&& self.force_sync_action.is_none()
		{
			self.force_sync_action = Some(Action::Start(scope));
		}
	}

	pub(super) fn force_sync_settings(&mut self, ui: &mut egui::Ui, state: &client_core::State) {
		design::group(ui, "force-sync-settings-title", |ui| {
			design::hint(ui, "force-sync-description");
			ui.add_space(8.0);
			let available = state.demo || state.gateway_connected;
			let active = self
				.force_sync_progress
				.as_ref()
				.is_some_and(|p| p.status.active());
			ui.add_enabled_ui(available || self.force_sync_progress.is_some(), |ui| {
				crate::settings::reveal_search_target(ui, "force-sync-account");
				if design::button(
					ui,
					if active {
						"force-sync-view-progress"
					} else {
						"force-sync-account"
					},
					design::ButtonKind::Outline,
				)
				.clicked()
				{
					if available || active {
						self.open_force_sync(Scope::Account);
					} else {
						self.force_sync_open = true;
					}
				}
			});
			if !available {
				design::hint(ui, "force-sync-reconnect");
			}
			if let Some(cache) = &self.force_sync_cache {
				design::hint(ui, &cache_label(cache.total_bytes));
				for (key, bytes) in [
					("force-sync-cache-history", cache.history_bytes),
					("force-sync-cache-metadata", cache.metadata_bytes),
					("force-sync-cache-media", cache.media_bytes),
					("force-sync-cache-artwork", cache.artwork_bytes),
				] {
					if let Some(bytes) = bytes {
						let used = format!("{:.1}", bytes as f64 / 1_000_000.0);
						design::hint(ui, &i18n::translate_args(key, &[("used", &used)]));
					}
				}
				if let Some(cleanup) = cache.last_cleanup {
					design::hint(ui, &i18n::translate_if_key(cleanup));
				}
			} else {
				design::hint(ui, "force-sync-cache-limit");
				design::hint(ui, "force-sync-cache-unmeasured");
			}
		});
	}

	pub(super) fn show_force_sync(&mut self, ctx: &egui::Context, state: &client_core::State) {
		if let Some(guild) = self.server_menu.force_sync_requested.take()
			&& state.guild(guild).is_some()
		{
			self.open_force_sync(Scope::Server(guild));
		}
		if !self.force_sync_open {
			return;
		}
		let progress = self.force_sync_progress.as_ref();
		let response =
			dialog::Dialog::new("force-sync-progress", i18n::translate("force-sync-title"))
				.subtitle(i18n::translate("force-sync-background"))
				.width(460.0)
				.show(ctx, |dialog| {
					dialog.content(|ui| {
						ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
						if let Some(progress) = progress {
							let scope = match progress.scope {
								Scope::Account => i18n::translate("force-sync-scope-account"),
								Scope::Server(guild) => state.guild(guild).map_or_else(
									|| i18n::translate("force-sync-scope-server"),
									|guild| guild.name.clone(),
								),
							};
							ui.label(design::semibold(ui, &scope, 15.0));
							ui.label(i18n::translate(status_label(progress)));
							if progress.status.active() {
								ui.label(i18n::translate(progress.stage.label()));
							}
							let done = progress.channels_done.to_string();
							let total = progress.channels_total.to_string();
							let messages = progress.messages_saved.to_string();
							ui.label(i18n::translate_args(
								"force-sync-counts",
								&[("done", &done), ("total", &total), ("messages", &messages)],
							));
							ui.label(cache_label(progress.cache_bytes));
							if let Some(detail) = progress.detail {
								design::hint(ui, &i18n::translate_if_key(detail));
							}
							if !progress.sections.is_empty() {
								ui.separator();
								egui::ScrollArea::vertical()
									.id_salt("force-sync-sections")
									.max_height(
										(ui.ctx().content_rect().height() * 0.32)
											.clamp(64.0, 240.0),
									)
									.show(ui, |ui| {
										for stage in Stage::ALL {
											if let Some(section) = progress
												.sections
												.iter()
												.take(7)
												.find(|s| s.stage == stage)
											{
												ui.horizontal_wrapped(|ui| {
													ui.label(design::medium(
														ui,
														i18n::translate(stage.label()),
														14.0,
													));
													let colors = design::palette(ui);
													ui.colored_label(
														if section.outcome == Outcome::Failed {
															colors.danger
														} else {
															colors.muted
														},
														i18n::translate(section.outcome.label()),
													);
												});
												if let Some(detail) = section.detail {
													design::hint(
														ui,
														&i18n::translate_if_key(detail),
													);
												}
											}
										}
									});
							}
						} else {
							design::hint(ui, "force-sync-preparing");
						}
					});
					dialog.footer(|ui| {
						ui.horizontal_wrapped(|ui| {
							if let Some(progress) = progress {
								for (label, action) in available_actions(progress) {
									action_button(ui, label, action, &mut self.force_sync_action);
								}
							}
						});
					});
				});
		if response.close {
			self.force_sync_open = false;
		}
	}
}

fn status_label(progress: &Progress) -> &'static str {
	match progress.status {
		Status::Running => "force-sync-status-running",
		Status::Paused => "force-sync-status-paused",
		Status::Waiting => "force-sync-status-waiting",
		Status::Complete
			if progress
				.sections
				.iter()
				.take(7)
				.any(|s| s.outcome != Outcome::Updated) =>
		{
			"force-sync-status-partial"
		}
		Status::Complete => "force-sync-status-complete",
		Status::Cancelled => "force-sync-status-cancelled",
	}
}

fn cache_label(bytes: u64) -> String {
	let used = format!("{:.2}", bytes as f64 / 1_000_000_000.0);
	i18n::translate_args("force-sync-cache-usage", &[("used", &used)])
}

fn available_actions(progress: &Progress) -> Vec<(&'static str, Action)> {
	let mut actions = Vec::with_capacity(3);
	match progress.status {
		Status::Running | Status::Waiting => actions.push(("force-sync-pause", Action::Pause)),
		Status::Paused => actions.push(("force-sync-resume", Action::Resume)),
		Status::Complete | Status::Cancelled => {}
	}
	if progress.status.active() {
		actions.push(("force-sync-cancel", Action::Cancel));
	} else if progress.can_retry {
		actions.push(("force-sync-retry", Action::Retry));
	}
	actions
}

fn action_button(
	ui: &mut egui::Ui,
	label: &str,
	action: Action,
	requested: &mut Option<Action>,
) -> egui::Response {
	let response = design::button(ui, label, design::ButtonKind::Outline);
	if response.clicked() && requested.is_none() {
		*requested = Some(action);
	}
	response
}

#[cfg(test)]
mod tests {
	use super::*;

	fn progress(status: Status) -> Progress {
		Progress {
			scope: Scope::Server(Id(10)),
			status,
			stage: Stage::History,
			channels_done: 19,
			channels_total: 75,
			messages_saved: 12345,
			cache_bytes: 1_500_000_000,
			sections: vec![Section {
				stage: Stage::Members,
				outcome: Outcome::Partial,
				detail: None,
			}],
			detail: None,
			can_retry: true,
		}
	}

	#[test]
	fn repeated_start_reopens_without_replacing_an_active_or_pending_job() {
		let mut view = MessagingUi::default();
		view.open_force_sync(Scope::Server(Id(10)));
		assert_eq!(
			view.force_sync_action,
			Some(Action::Start(Scope::Server(Id(10))))
		);
		view.open_force_sync(Scope::Account);
		assert_eq!(
			view.force_sync_action.take(),
			Some(Action::Start(Scope::Server(Id(10))))
		);
		for status in [Status::Running, Status::Paused, Status::Waiting] {
			view.force_sync_open = false;
			view.force_sync_progress = Some(progress(status));
			view.open_force_sync(Scope::Account);
			assert!(view.force_sync_open);
			assert_eq!(view.force_sync_action, None);
			assert_eq!(
				view.force_sync_progress.as_ref().unwrap().scope,
				Scope::Server(Id(10))
			);
		}
		view.force_sync_progress = Some(progress(Status::Complete));
		view.open_force_sync(Scope::Account);
		assert_eq!(view.force_sync_action, Some(Action::Start(Scope::Account)));
	}

	#[test]
	fn progress_actions_and_partial_completion_explain_the_actual_outcome() {
		let expected = [
			(Status::Running, vec![Action::Pause, Action::Cancel]),
			(Status::Waiting, vec![Action::Pause, Action::Cancel]),
			(Status::Paused, vec![Action::Resume, Action::Cancel]),
			(Status::Complete, vec![Action::Retry]),
			(Status::Cancelled, vec![Action::Retry]),
		];
		for (status, actions) in expected {
			assert_eq!(
				available_actions(&progress(status))
					.into_iter()
					.map(|(_, a)| a)
					.collect::<Vec<_>>(),
				actions
			);
		}
		let mut complete = progress(Status::Complete);
		assert_eq!(status_label(&complete), "force-sync-status-partial");
		complete.sections[0].outcome = Outcome::Updated;
		complete.can_retry = false;
		assert_eq!(status_label(&complete), "force-sync-status-complete");
		assert!(available_actions(&complete).is_empty());
	}

	#[test]
	fn modal_wraps_status_and_uses_the_same_controls_in_narrow_scaled_themes() {
		crate::i18n::set_current(crate::i18n::Language::English);
		for (width, scale, light) in [(320.0, 1.0, false), (320.0, 2.0, true), (760.0, 1.0, true)] {
			let ctx = egui::Context::default();
			design::apply(&ctx);
			ctx.set_pixels_per_point(scale);
			ctx.set_visuals(if light {
				egui::Visuals::light()
			} else {
				egui::Visuals::dark()
			});
			let state = test_support::chat_demo_state();
			let mut view = MessagingUi {
				force_sync_open: true,
				force_sync_progress: Some(progress(Status::Complete)),
				..Default::default()
			};
			let frame = |view: &mut MessagingUi| {
				ctx.run_ui(
					egui::RawInput {
						screen_rect: Some(egui::Rect::from_min_size(
							egui::Pos2::ZERO,
							egui::vec2(width, 760.0),
						)),
						..Default::default()
					},
					|ui| view.show_force_sync(ui.ctx(), &state),
				)
			};
			for _ in 0..3 {
				frame(&mut view).drop_without_applying_deltas();
			}
			let output = frame(&mut view);
			let mut labels = Vec::new();
			for shape in &output.shapes {
				crate::test_ui::logical_text(&shape.shape, &mut labels);
			}
			output.drop_without_applying_deltas();
			assert!(labels.iter().any(|(s, _)| s == "Sync finished with gaps"));
			assert!(labels.iter().any(|(s, _)| s == "Retry failed sections"));
			assert!(
				labels
					.iter()
					.any(|(s, _)| s.contains("1.50") && s.contains("3 GB"))
			);
			for (text, rect) in labels.iter().filter(|(text, _)| {
				text == "Sync finished with gaps" || text == "Retry failed sections"
			}) {
				assert!(
					rect.left() >= 0.0 && rect.right() <= width + 1.0,
					"{width}: {text}: {rect:?}"
				);
			}
			assert_eq!(view.force_sync_action, None);
		}
	}

	#[test]
	fn keyboard_activation_dispatches_once_and_closing_does_not_cancel() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut action = None;
		let mut id = None;
		let output = ctx.run_ui(egui::RawInput::default(), |ui| {
			id = Some(action_button(ui, "force-sync-pause", Action::Pause, &mut action).id);
		});
		output.drop_without_applying_deltas();
		ctx.memory_mut(|memory| memory.request_focus(id.unwrap()));
		let output = ctx.run_ui(
			egui::RawInput {
				events: vec![egui::Event::Key {
					key: egui::Key::Enter,
					physical_key: None,
					pressed: true,
					repeat: false,
					modifiers: egui::Modifiers::NONE,
				}],
				..Default::default()
			},
			|ui| {
				action_button(ui, "force-sync-pause", Action::Pause, &mut action);
			},
		);
		output.drop_without_applying_deltas();
		assert_eq!(action, Some(Action::Pause));
		let mut view = MessagingUi {
			force_sync_open: false,
			force_sync_progress: Some(progress(Status::Running)),
			..Default::default()
		};
		view.show_force_sync(&ctx, &test_support::chat_demo_state());
		assert_eq!(view.force_sync_action, None);
		assert_eq!(
			view.force_sync_progress.as_ref().unwrap().status,
			Status::Running
		);
		view.force_sync_open = true;
		for _ in 0..3 {
			ctx.run_ui(egui::RawInput::default(), |ui| {
				view.show_force_sync(ui.ctx(), &test_support::chat_demo_state())
			})
			.drop_without_applying_deltas();
		}
		ctx.run_ui(
			egui::RawInput {
				events: vec![egui::Event::Key {
					key: egui::Key::Escape,
					physical_key: None,
					pressed: true,
					repeat: false,
					modifiers: egui::Modifiers::NONE,
				}],
				..Default::default()
			},
			|ui| view.show_force_sync(ui.ctx(), &test_support::chat_demo_state()),
		)
		.drop_without_applying_deltas();
		assert!(!view.force_sync_open);
		assert_eq!(view.force_sync_action, None);
		assert_eq!(
			view.force_sync_progress.as_ref().unwrap().status,
			Status::Running
		);
	}
}
