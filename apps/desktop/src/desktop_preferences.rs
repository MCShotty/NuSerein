//! Apply and persist device-local preferences without owning their wire/storage formats.
use super::{
	AuthState, Desktop, Duration, cache, credentials, egui, font_import, plaintext_login_dir,
};

impl Desktop {
	/// Hands the owner's plaintext saved-login choice to the credential worker, in order with
	/// its other writes, and saves a login that failed earlier for want of a keyring.
	pub(super) fn sync_plaintext_login(&mut self) {
		let enabled = self.messaging.plaintext_login;
		let (Some(store), Some(directory)) = (&mut self.store, plaintext_login_dir()) else {
			return;
		};
		if enabled != self.plaintext_login {
			let operation = credentials::Operation::SetPlaintext { directory, enabled };
			if store
				.send
				.try_send((self.state.generation, credentials::Request::NONE, operation))
				.is_err()
			{
				return; // Retried after the worker wakes the UI.
			}
			self.plaintext_login = enabled;
			self.plaintext_save_pending = enabled;
		}
		if !enabled || self.state.auth != AuthState::Authenticated {
			return;
		}
		let Some(secret) = self.current_login.clone() else {
			return;
		};
		if self.plaintext_save_pending
			&& let Some(account) = self.state.user.as_ref().map(|user| user.id)
			&& store.save_account(self.state.generation, account, secret, true)
		{
			// Keep keyed and launch saves in one ordered command, including fallback saves.
			self.plaintext_save_pending = false;
		}
	}
	pub(super) fn save_app_preferences(&mut self) {
		if self.fixture_only || self.state.demo {
			return;
		}
		self.app_settings.observe(&self.messaging);
		self.cache_pending += usize::from(
			self.app_settings
				.save(self.cache.as_ref(), self.state.generation),
		);
	}
	pub(super) fn accept_font(&mut self, ctx: &egui::Context, result: &font_import::Selected) {
		self.messaging.custom_font.busy = false;
		match result {
			Ok(font) => {
				if ui::fonts::apply_custom(ctx, font.as_ref()) {
					self.messaging.custom_font.name = font.as_ref().map(|font| font.name.clone());
					self.messaging.custom_font.status = "";
				} else {
					self.messaging.custom_font.status =
						"Could not apply the font. Restart Serein and try again.";
				}
			}
			Err(error) => self.messaging.custom_font.status = error,
		}
	}
	pub(super) fn save_font(&mut self, ctx: &egui::Context, font: Option<ui::fonts::CustomFont>) {
		if self.fixture_only || self.state.demo {
			self.accept_font(ctx, &Ok(font));
			self.messaging.custom_font.status = "Preview only; this font is not saved.";
		} else {
			self.messaging.custom_font.busy =
				self.queue_cache_for(model::Id(0), cache::Operation::SaveCustomFont(font));
			self.messaging.custom_font.status = if self.messaging.custom_font.busy {
				"Saving font…"
			} else {
				"Could not save the font. Try again."
			};
		}
	}
	pub(super) fn sync_fonts(&mut self, ctx: &egui::Context) {
		if let Some(picker) = &self.font_picker {
			let result = match picker.try_recv() {
				Ok(result) => Some(result),
				Err(std::sync::mpsc::TryRecvError::Empty) => None,
				Err(std::sync::mpsc::TryRecvError::Disconnected) => {
					Some(Err("Font import interrupted. Try again."))
				}
			};
			if let Some(result) = result {
				self.font_picker = None;
				self.messaging.custom_font.busy = false;
				self.messaging.custom_font.status = "";
				match result {
					Ok(Some(font)) => self.save_font(ctx, Some(font)),
					Ok(None) => {}
					Err(error) => self.messaging.custom_font.status = error,
				}
			}
		}
		if let Some(families) = &self.font_families
			&& let Ok(families) = families.try_recv()
		{
			self.font_families = None;
			self.messaging.custom_font.families = Some(families);
		}
		// Listing is independent of a pending save, so it is never dropped while busy.
		if matches!(
			self.messaging.custom_font.request,
			Some(ui::fonts::Action::List)
		) {
			self.messaging.custom_font.request = None;
			if self.font_families.is_none() && self.messaging.custom_font.families.is_none() {
				self.font_families = Some(font_import::installed(&self.runtime, ctx));
			}
		}
		if let Some(action) = self.messaging.custom_font.request.take()
			&& !self.messaging.custom_font.busy
		{
			match action {
				ui::fonts::Action::List => {}
				ui::fonts::Action::Select(family) => {
					self.font_picker = Some(font_import::load(&self.runtime, ctx, family));
					self.messaging.custom_font.busy = true;
					self.messaging.custom_font.status = "Loading font…";
				}
				ui::fonts::Action::Reset => self.save_font(ctx, None),
			}
		}
	}
	pub(super) fn save_reading_preferences(&mut self, ctx: &egui::Context) {
		if self.fixture_only {
			return;
		}
		let now = std::time::Instant::now();
		// Finish changes made before entering preview; never persist preview controls.
		if !self.state.demo {
			self.reading
				.observe(self.messaging.reading_preferences, now);
			if std::mem::take(&mut self.messaging.reading_save_requested) {
				self.reading.request_save(now);
			}
		}
		if self.reading.ready(now) {
			let accepted = self.cache.as_ref().is_some_and(|cache| {
				cache.queue(
					self.state.generation,
					model::Id(0),
					cache::Operation::SaveReadingPreferences(self.reading.current),
				)
			});
			self.reading.queued(accepted);
			self.cache_pending += usize::from(accepted);
		}
		if let Some(delay) = self.reading.remaining(now) {
			ctx.request_repaint_after(delay);
		}
		self.messaging.reading_status = self.reading.status();
	}
	/// egui resolves System through `fallback_theme` when winit reports no system theme, as on
	/// Wayland and X11. A reported system theme still takes precedence on Windows and macOS.
	pub(super) fn sync_system_theme(&self, ctx: &egui::Context) {
		let Some(dark) = self.system_theme.dark() else {
			return;
		};
		let theme = if dark {
			egui::Theme::Dark
		} else {
			egui::Theme::Light
		};
		if ctx.options(|options| options.fallback_theme) != theme {
			ctx.options_mut(|options| options.fallback_theme = theme);
		}
	}
	/// Native timezone discovery runs in the existing blocking pool; publish it only
	/// at a frame boundary so every timestamp in this frame uses the same zone.
	pub(super) fn sync_timezone(&mut self, ctx: &egui::Context) {
		if let Some(recovered) = ui::local_time::begin_frame() {
			self.messaging.time_zone = recovered;
		}
		ui::local_time::set_timezone(&self.messaging.time_zone);
		let focused = ctx.input(|input| input.focused && input.viewport().minimized != Some(true));
		if focused {
			if !self.timezone_focused || self.timezone_checked.elapsed() >= Duration::from_secs(60)
			{
				ui::local_time::refresh_system_timezone();
				self.timezone_checked = std::time::Instant::now();
			}
			ctx.request_repaint_after(Duration::from_secs(60));
		}
		self.timezone_focused = focused;
		if let Some(request) = ui::local_time::take_timezone_request() {
			let wake = ctx.clone();
			self.runtime.spawn_blocking(move || {
				request.resolve();
				wake.request_repaint();
			});
		}
	}
	pub(super) fn sync_customization(&mut self, ctx: &egui::Context) {
		if self.messaging.primary_color != ui::design::primary_color() {
			ui::design::set_primary_color(self.messaging.primary_color);
			ui::design::apply(ctx);
			ctx.request_repaint();
		}
		let effects = (
			self.transparency_available,
			self.messaging.transparency,
			self.messaging.blur,
		);
		if effects != ui::design::default_window_effects() {
			ui::design::set_window_effects(effects.0, effects.1, effects.2);
			ui::design::apply(ctx);
			ctx.request_repaint();
		}
	}
}
