//! Native, action-scoped security check. Secret text never enters preferences or the clipboard.
use client_core::{
	Command, State,
	mfa::{Method, Phase},
};
use zeroize::{Zeroize, Zeroizing};

fn method_label(method: Method) -> String {
	crate::i18n::translate(match method {
		Method::Totp => "admin-mfa-authenticator",
		Method::Backup | Method::BackupViaTotp => "admin-mfa-backup",
		Method::Sms => "admin-mfa-sms",
		Method::Password => "admin-mfa-password",
		Method::Passkey => "admin-mfa-passkey",
		Method::Unsupported => "admin-mfa-unsupported",
	})
}

fn error_text(error: &str) -> String {
	use client_core::auth::Failure;
	let known = [
		(Failure::Forbidden.label(), "admin-mfa-error-permission"),
		(Failure::RateLimited.label(), "admin-mfa-error-rate"),
		(Failure::Network.label(), "admin-mfa-error-network"),
		(Failure::Ambiguous.label(), "admin-mfa-error-ambiguous"),
		(Failure::Protocol.label(), "admin-mfa-error-protocol"),
		(Failure::Expired.label(), "admin-mfa-error-expired"),
		(Failure::Challenged.label(), "admin-mfa-error-challenge"),
		(Failure::Capacity.label(), "admin-mfa-error-capacity"),
		(
			Failure::InvalidCredential.label(),
			"admin-mfa-error-credential",
		),
		(
			"Verification was not queued; try again",
			"admin-mfa-error-queue",
		),
	];
	known
		.into_iter()
		.find(|(label, _)| *label == error)
		.map_or_else(|| error.to_owned(), |(_, key)| crate::i18n::translate(key))
}

#[derive(Default)]
pub(super) struct MfaUi {
	scope: Option<(u64, u64)>,
	method: Option<Method>,
	input: Zeroizing<String>,
}
impl MfaUi {
	fn clear(&mut self, ctx: &egui::Context) {
		if self.scope.is_none() && self.input.is_empty() {
			return;
		}
		self.input.zeroize();
		self.scope = None;
		self.method = None;
		ctx.data_mut(|data| {
			data.remove::<egui::text_edit::TextEditState>(egui::Id::unique("admin-mfa-input"))
		});
	}
	pub fn retire(&mut self, ctx: &egui::Context, state: &State) {
		if self.scope.is_some_and(|(generation, id)| {
			generation != state.generation
				|| state.auth != client_core::auth::AuthState::Authenticated
				|| state
					.mfa
					.pending
					.as_ref()
					.is_none_or(|p| p.id != id || p.remaining().is_zero())
		}) {
			self.clear(ctx);
		}
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		if let Some(command) = state.maintain_mfa() {
			commands.push(command);
		}
		let Some(pending) = state.mfa.pending.as_ref() else {
			self.clear(ctx);
			return;
		};
		let scope = (state.generation, pending.id);
		let changed_scope = self.scope != Some(scope);
		if changed_scope {
			self.input.zeroize();
			self.scope = Some(scope);
			self.method = pending
				.methods
				.iter()
				.copied()
				.find(|method| method.supported())
				.or_else(|| pending.methods.first().copied());
		}
		let methods = pending.methods.clone();
		let phase = pending.phase;
		let error = pending.error;
		let sms_sent = pending.sms_sent;
		let guild = pending.target.guild();
		let remaining = pending.remaining();
		let ready = phase == Phase::Editing;
		let mut cancel = false;
		let mut verify = false;
		let mut sms = false;
		let before = self.method;
		let response = crate::dialog::Dialog::new("admin-mfa", "admin-mfa-title")
			.subtitle("admin-mfa-subtitle")
			.width(440.0)
			.show(ctx, |dialog| {
				dialog.content(|ui| {
					let selected = self.method.unwrap_or(Method::Unsupported);
					ui.add_enabled_ui(ready, |ui| {
						egui::ComboBox::from_id_salt("admin-mfa-method")
							.selected_text(method_label(selected))
							.show_ui(ui, |ui| {
								for method in &methods {
									ui.selectable_value(
										&mut self.method,
										Some(*method),
										method_label(*method),
									);
								}
							});
						let method = self.method.unwrap_or(Method::Unsupported);
						if method.supported() {
							if method == Method::Sms {
								sms = ui
									.button(crate::i18n::translate(if sms_sent {
										"admin-mfa-send-another-sms"
									} else {
										"admin-mfa-send-sms"
									}))
									.clicked();
								if sms_sent {
									ui.weak(crate::i18n::translate("admin-mfa-sms-sent"));
								}
							}
							let mut edit = egui::TextEdit::singleline(&mut *self.input)
								.id(egui::Id::unique("admin-mfa-input"))
								.password(true)
								.char_limit(if method == Method::Password { 256 } else { 32 })
								.hint_text(method_label(method))
								.desired_width(f32::INFINITY)
								.show(ui);
							// Do not retain secret text in the persistent egui undo state.
							edit.state.clear_undoer();
							edit.state.store(ctx, edit.response.id);
							if before != self.method || changed_scope {
								edit.response.request_focus();
							}
							verify = ready
								&& method.accepts(&self.input)
								&& (method != Method::Sms || sms_sent)
								&& (edit.response.has_focus() || edit.response.lost_focus())
								&& ui.input(|i| i.key_pressed(egui::Key::Enter));
						} else {
							ui.label(crate::i18n::translate("admin-mfa-unavailable"));
							if ui
								.button(crate::i18n::translate("admin-mfa-open-discord"))
								.clicked()
							{
								ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
									"https://discord.com/channels/{guild}"
								)));
							}
						}
					});
					if !ready {
						ui.horizontal(|ui| {
							ui.spinner();
							ui.label(crate::i18n::translate(
								if matches!(phase, Phase::Verified | Phase::Resuming) {
									"admin-mfa-completing"
								} else {
									"admin-mfa-checking"
								},
							));
						});
					}
					if let Some(error) = error {
						crate::dialog::notice(ui, crate::dialog::Level::Error, &error_text(error));
					}
					ui.weak(crate::i18n::translate("admin-mfa-expiry"));
				});
				dialog.footer(|ui| {
					let method = self.method.unwrap_or(Method::Unsupported);
					ui.add_enabled_ui(
						ready && method.accepts(&self.input) && (method != Method::Sms || sms_sent),
						|ui| {
							verify |= crate::dialog::action(
								ui,
								"verification-show-verify",
								crate::dialog::Action::Primary,
							)
							.clicked();
						},
					);
					cancel |= crate::dialog::action(
						ui,
						"verification-show-cancel",
						crate::dialog::Action::Neutral,
					)
					.clicked();
				});
			});
		ctx.request_repaint_after(remaining);
		if before != self.method {
			self.input.zeroize();
		}
		if cancel || response.close {
			self.input.zeroize();
			if let Some(command) = state.cancel_mfa() {
				commands.push(command);
			}
		} else if sms {
			self.input.zeroize();
			if let Some(command) = state.send_mfa_sms() {
				commands.push(command);
			}
		} else if verify && let Some(method) = self.method {
			let value = std::mem::take(&mut *self.input);
			if let Some(command) = state.submit_mfa(method, value) {
				commands.push(command);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::{Envelope, Event, auth::AuthState, mfa};
	use model::Id;

	fn challenged_state() -> State {
		let mut state = test_support::demo_state();
		state.auth = AuthState::Authenticated;
		state.gateway_connected = true;
		let mut permissions = test_support::permission_snapshot(&state);
		for guild in &mut permissions.guilds {
			guild.owner = state.user.as_ref().map(|user| user.id);
		}
		state.permissions.replace(permissions).unwrap();
		let command = state.delete_server(Id(10)).expect("synthetic owner action");
		let target = mfa::Target::from_command(&command).unwrap();
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Mfa(mfa::Event::Required(mfa::Notice {
				id: 7,
				target,
				methods: vec![Method::Totp, Method::Sms],
			})),
		});
		assert!(state.mfa.pending.is_some());
		state
	}

	fn frame(
		ctx: &egui::Context,
		view: &mut MfaUi,
		state: &mut State,
		events: Vec<egui::Event>,
	) -> (Vec<(String, egui::Rect)>, Vec<Command>) {
		let mut commands = Vec::new();
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(800.0, 700.0),
				)),
				events,
				..Default::default()
			},
			|_| view.show(ctx, state, &mut commands),
		);
		fn labels(shape: &egui::Shape, result: &mut Vec<(String, egui::Rect)>) {
			match shape {
				egui::Shape::Text(text) => result.push((
					text.galley.job.text.clone(),
					text.galley.rect.translate(text.pos.to_vec2()),
				)),
				egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| labels(shape, result)),
				_ => {}
			}
		}
		let mut text = Vec::new();
		for shape in &output.shapes {
			labels(&shape.shape, &mut text);
		}
		assert!(
			!output
				.platform_output
				.commands
				.iter()
				.any(|command| matches!(command, egui::OutputCommand::CopyText(_)))
		);
		output.drop_without_applying_deltas();
		(text, commands)
	}

	#[test]
	fn translated_challenge_keeps_secret_masked_and_verifies_only_on_explicit_enter() {
		crate::i18n::set_current(crate::i18n::Language::Czech);
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut state = challenged_state();
		let mut view = MfaUi::default();
		for _ in 0..3 {
			let (text, commands) = frame(&ctx, &mut view, &mut state, vec![]);
			assert!(commands.is_empty());
			for key in [
				"admin-mfa-title",
				"admin-mfa-subtitle",
				"admin-mfa-authenticator",
				"verification-show-cancel",
			] {
				let expected = crate::i18n::translate(key);
				assert!(
					text.iter().any(|(label, _)| label == &expected),
					"{key}: {text:?}"
				);
			}
		}
		view.input = Zeroizing::new("123456".into());
		ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("admin-mfa-input")));
		let (text, commands) = frame(&ctx, &mut view, &mut state, vec![]);
		assert!(commands.is_empty());
		assert!(
			!text.iter().any(|(label, _)| label.contains("123456")),
			"secret must not be painted in clear text"
		);
		let (_, commands) = frame(
			&ctx,
			&mut view,
			&mut state,
			vec![egui::Event::Key {
				key: egui::Key::Enter,
				physical_key: None,
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}],
		);
		assert!(
			matches!(commands.as_slice(), [Command::Mfa(request)] if matches!(&request.action, mfa::Action::Verify { method: Method::Totp, input } if !format!("{input:?}").contains("123456")))
		);
		assert!(view.input.is_empty());
		assert_eq!(state.mfa.pending.as_ref().unwrap().phase, Phase::Verifying);
		crate::i18n::set_current(crate::i18n::Language::English);
	}

	#[test]
	fn translated_sms_action_remains_explicit_and_clears_the_previous_secret() {
		crate::i18n::set_current(crate::i18n::Language::Czech);
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut state = challenged_state();
		let mut view = MfaUi::default();
		for _ in 0..3 {
			assert!(frame(&ctx, &mut view, &mut state, vec![]).1.is_empty());
		}
		view.method = Some(Method::Sms);
		view.input = Zeroizing::new("123456".into());
		let (text, commands) = frame(&ctx, &mut view, &mut state, vec![]);
		assert!(commands.is_empty(), "selecting SMS does not send a code");
		let label = crate::i18n::translate("admin-mfa-send-sms");
		let position = text
			.iter()
			.find(|(text, _)| text == &label)
			.expect("translated SMS action")
			.1
			.center();
		let mut sent = Vec::new();
		for pressed in [true, false] {
			let (_, commands) = frame(
				&ctx,
				&mut view,
				&mut state,
				vec![
					egui::Event::PointerMoved(position),
					egui::Event::PointerButton {
						pos: position,
						button: egui::PointerButton::Primary,
						pressed,
						modifiers: egui::Modifiers::NONE,
					},
				],
			);
			sent.extend(commands);
		}
		assert!(
			matches!(sent.as_slice(), [Command::Mfa(request)] if matches!(&request.action, mfa::Action::SendSms))
		);
		assert!(view.input.is_empty());
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Mfa(mfa::Event::Updated {
				id: 7,
				result: Ok(mfa::Status::SmsSent),
			}),
		});
		let (text, commands) = frame(&ctx, &mut view, &mut state, vec![]);
		assert!(commands.is_empty());
		for key in ["admin-mfa-sms-sent", "admin-mfa-send-another-sms"] {
			let expected = crate::i18n::translate(key);
			assert!(
				text.iter().any(|(label, _)| label == &expected),
				"{key}: {text:?}"
			);
		}
		crate::i18n::set_current(crate::i18n::Language::English);
	}

	#[test]
	fn method_and_known_error_labels_are_translated_without_losing_unknown_errors() {
		for language in crate::i18n::Language::ALL {
			crate::i18n::set_current(language);
			for suffix in [
				"title",
				"subtitle",
				"send-sms",
				"send-another-sms",
				"sms-sent",
				"unavailable",
				"open-discord",
				"completing",
				"checking",
				"expiry",
				"error-permission",
				"error-rate",
				"error-network",
				"error-ambiguous",
				"error-protocol",
				"error-expired",
				"error-challenge",
				"error-capacity",
				"error-credential",
				"error-queue",
			] {
				let key = format!("admin-mfa-{suffix}");
				let label = crate::i18n::translate(&key);
				assert!(
					!label.starts_with("Unknown localization key:") && !label.is_empty(),
					"{language:?}: {key}"
				);
			}
			for method in [
				Method::Totp,
				Method::Backup,
				Method::BackupViaTotp,
				Method::Sms,
				Method::Password,
				Method::Passkey,
				Method::Unsupported,
			] {
				let label = method_label(method);
				assert!(
					!label.starts_with("Unknown localization key:") && !label.is_empty(),
					"{language:?}: {method:?}"
				);
			}
			for error in [
				"Permission denied",
				"Rate limited · wait before retrying",
				"Connection failed",
				"Verification was not queued; try again",
			] {
				let label = error_text(error);
				assert!(
					!label.starts_with("Unknown localization key:") && !label.is_empty(),
					"{language:?}: {error}"
				);
			}
			assert_eq!(
				error_text("Synthetic local stage detail"),
				"Synthetic local stage detail"
			);
		}
		crate::i18n::set_current(crate::i18n::Language::English);
	}

	#[test]
	fn secret_input_is_retired_when_session_changes_even_without_rendering_a_dialog() {
		let ctx = egui::Context::default();
		let mut ui = MfaUi {
			scope: Some((1, 7)),
			method: Some(Method::Password),
			input: Zeroizing::new("SYNTHETIC_PASSWORD".into()),
		};
		let state = State {
			generation: 2,
			..State::default()
		};
		ui.retire(&ctx, &state);
		assert!(ui.input.is_empty());
		assert!(ui.scope.is_none());
		assert!(ui.method.is_none());
	}
}
