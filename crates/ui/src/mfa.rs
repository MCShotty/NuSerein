//! Native, action-scoped security check. Secret text never enters preferences or the clipboard.
use client_core::{
	Command, State,
	mfa::{Method, Phase},
};
use zeroize::{Zeroize, Zeroizing};

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
			data.remove::<egui::text_edit::TextEditState>(egui::Id::new("admin-mfa-input"))
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
		let response = crate::dialog::Dialog::new("admin-mfa", "Verify this action")
            .subtitle("Discord requires a security check before completing this server action.")
            .width(440.0)
            .show(ctx, |dialog| {
                dialog.content(|ui| {
                    let selected = self.method.unwrap_or(Method::Unsupported);
                    ui.add_enabled_ui(ready, |ui| {
                        egui::ComboBox::from_id_salt("admin-mfa-method")
                            .selected_text(selected.label()).show_ui(ui, |ui| {
                                for method in &methods { ui.selectable_value(&mut self.method, Some(*method), method.label()); }
                            });
                        let method = self.method.unwrap_or(Method::Unsupported);
                        if method.supported() {
                            if method == Method::Sms {
                                sms = ui.button(if sms_sent { "Send another SMS code" } else { "Send SMS code" }).clicked();
                                if sms_sent { ui.weak("Use the code sent to your phone."); }
                            }
                            let mut edit = egui::TextEdit::singleline(&mut *self.input)
                                .id(egui::Id::new("admin-mfa-input"))
                                .password(true).char_limit(if method == Method::Password { 256 } else { 32 })
                                .hint_text(method.label()).desired_width(f32::INFINITY).show(ui);
                            // Do not retain secret text in the persistent egui undo state.
                            edit.state.clear_undoer(); edit.state.store(ctx, edit.response.id);
                            if before != self.method || changed_scope { edit.response.request_focus(); }
                            verify = ready && method.accepts(&self.input) && (method != Method::Sms || sms_sent)
                                && edit.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        } else {
                            ui.label("This security method is not available here. Choose another method or complete the action in Discord.");
                            if ui.button("Open Discord").clicked() { ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://discord.com/channels/{guild}"))); }
                        }
                    });
                    if !ready { ui.horizontal(|ui| { ui.spinner(); ui.label(if matches!(phase, Phase::Verified | Phase::Resuming) { "Completing the verified action…" } else { "Checking…" }); }); }
                    if let Some(error) = error { crate::dialog::notice(ui, crate::dialog::Level::Error, error); }
                    ui.weak("Verification expires after five minutes. Nothing is saved.");
                });
                dialog.footer(|ui| {
                    let method = self.method.unwrap_or(Method::Unsupported);
                    ui.add_enabled_ui(ready && method.accepts(&self.input) && (method != Method::Sms || sms_sent), |ui| {
                        verify |= crate::dialog::action(ui, "Verify", crate::dialog::Action::Primary).clicked();
                    });
                    cancel |= crate::dialog::action(ui, "Cancel", crate::dialog::Action::Neutral).clicked();
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
