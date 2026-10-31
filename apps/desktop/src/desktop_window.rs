//! Native window, tray and compositor integration.
use super::{Desktop, Duration, app_settings, cache, egui};

impl Desktop {
	pub(super) fn sync_tray(&mut self, ctx: &egui::Context) {
		let previous_status = self.messaging.tray_status;
		self.tray_setting.observe(self.messaging.minimize_to_tray);
		if self.tray_setting.dirty && !self.tray_setting.saving {
			self.tray_error = None;
			let accepted = !self.fixture_only
				&& !self.state.demo
				&& self.cache.as_ref().is_some_and(|cache| {
					cache.queue(
						self.state.generation,
						model::Id(0),
						cache::Operation::SaveMinimizeToTray(self.tray_setting.enabled),
					)
				});
			self.tray_setting.dirty = false;
			self.tray_setting.saving = accepted;
			self.tray_setting.failed = !accepted && !self.fixture_only && !self.state.demo;
			self.cache_pending += usize::from(accepted);
		}
		if self.tray_error.is_some() {
			self.tray = None;
		} else if self.tray.is_none() {
			let wake = ctx.clone();
			#[cfg(target_os = "linux")]
			let tray = {
				let _runtime = self.runtime.enter();
				platform::tray::Tray::new(
					move || wake.request_repaint(),
					self.tray_window.restorer(),
				)
			};
			#[cfg(not(target_os = "linux"))]
			let tray = platform::tray::Tray::new(self.window.clone(), move || wake.request_repaint());
			match tray {
				Ok(tray) => self.tray = Some(tray),
				Err(error) => self.tray_error = Some(error),
			}
		}
		if let Some(tray) = &self.tray {
			let deafened = self.messaging.voice_deafened
				|| self
					.state
					.voice
					.active
					.as_ref()
					.is_some_and(|c| c.deafened || c.server_deafened);
			let muted = self.messaging.voice_muted
				|| self
					.state
					.voice
					.active
					.as_ref()
					.is_some_and(|c| c.muted || c.server_muted);
			let speaking = self
				.state
				.user
				.as_ref()
				.is_some_and(|own| self.messaging.voice_speaking.contains(&own.id));
			let voice_state = if deafened {
				platform::tray::VoiceState::Deafened
			} else if muted {
				platform::tray::VoiceState::Muted
			} else if speaking {
				platform::tray::VoiceState::Speaking
			} else {
				platform::tray::VoiceState::Unmuted
			};
			tray.set_voice_state(voice_state);
		}
		if self.tray_window.hidden && !self.background_available() {
			self.tray_window.show(ctx);
		}
		// The icon remains registered independently of minimize-on-close.
		self.messaging.tray_status = self
			.tray_error
			.unwrap_or_else(|| self.tray_setting.status());
		if previous_status != self.messaging.tray_status {
			ctx.request_repaint();
		}
	}
	/// Native menu bar commands. Editing commands arrive as the events their shortcuts send.
	#[cfg(target_os = "macos")]
	pub(super) fn app_menu(&mut self, ctx: &egui::Context) {
		use platform::app_menu::Command;
		let Some((menu, language)) = &mut self.app_menu else {
			return;
		};
		if *language != self.messaging.language {
			*language = self.messaging.language;
			let language = *language;
			menu.translate(|key| language.text(key));
		}
		let key = |key, modifiers, pressed| egui::Event::Key {
			key,
			physical_key: Some(key),
			pressed,
			repeat: false,
			modifiers: egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND | modifiers,
		};
		while let Some(command) = menu.take_command() {
			let event = match command {
				Command::Settings | Command::Updates => {
					self.tray_window.show(ctx);
					if self.state.user.is_some() {
						if command == Command::Settings {
							self.messaging.open_settings();
						} else {
							self.messaging.open_update_settings();
						}
					}
					None
				}
				Command::Quit => {
					self.tray_window.quit(ctx);
					None
				}
				// Never quits, tray or not; that is Quit's job.
				Command::CloseWindow => {
					self.tray_window.hide(ctx);
					None
				}
				Command::Undo => Some(key(egui::Key::Z, egui::Modifiers::NONE, true)),
				Command::Redo => Some(key(egui::Key::Z, egui::Modifiers::SHIFT, true)),
				Command::SelectAll => Some(key(egui::Key::A, egui::Modifiers::NONE, true)),
				Command::Cut => Some(egui::Event::Cut),
				Command::Copy => Some(egui::Event::Copy),
				Command::Paste => {
					// Like eframe's Cmd+V: the text, then the key release the composer also
					// reads to paste files and images.
					if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text())
						&& !text.is_empty()
					{
						let text = text.replace("\r\n", "\n");
						ctx.input_mut(|input| input.events.push(egui::Event::Paste(text)));
					}
					Some(key(egui::Key::V, egui::Modifiers::NONE, false))
				}
				Command::ZoomIn => {
					egui::gui_zoom::zoom_in(ctx);
					None
				}
				Command::ZoomOut => {
					egui::gui_zoom::zoom_out(ctx);
					None
				}
				Command::ActualSize => {
					ctx.set_zoom_factor(1.0);
					None
				}
				Command::Website => {
					ctx.open_url(egui::OpenUrl::new_tab("https://github.com/MCShotty/NuSerein"));
					None
				}
				Command::ReportIssue => {
					ctx.open_url(egui::OpenUrl::new_tab(
						"https://github.com/MCShotty/NuSerein/issues/new/choose",
					));
					None
				}
			};
			if let Some(event) = event {
				ctx.input_mut(|input| input.events.push(event));
			}
		}
	}
	/// Whether closing may keep Serein running without a window. macOS apps stay in the Dock
	/// until quit, and the Dock icon reopens the window.
	pub(super) fn background_available(&self) -> bool {
		cfg!(target_os = "macos") || self.tray_available()
	}
	pub(super) fn tray_available(&self) -> bool {
		if !self.tray_setting.enabled || self.tray_error.is_some() {
			return false;
		}
		#[cfg(target_os = "linux")]
		{
			self.tray
				.as_ref()
				.is_some_and(platform::tray::Tray::is_available)
		}
		#[cfg(not(target_os = "linux"))]
		{
			self.tray.is_some()
		}
	}
	pub(super) fn sync_window_effects(&mut self) {
		if !self.transparency_available {
			return;
		}
		let effects = ui::design::window_effects();
		let transparent = effects.0 && effects.1 > 0;
		if transparent != self.window_transparent {
			self.window.set_transparent(transparent);
			self.window_transparent = transparent;
		}
		if let Some(window_blur) = &mut self.window_blur {
			window_blur.set_enabled(transparent && effects.2 > 0);
		}
	}
	/// Frame period of the display the window is on; egui otherwise assumes 60 Hz.
	pub(super) fn frame_period(&self) -> Option<Duration> {
		self.monitor_period
	}
	pub(super) fn refresh_monitor(&mut self) -> Option<Duration> {
		let monitor = self.window.current_monitor()?;
		app_settings::update_window_minimum(&self.window, &monitor);
		let millihertz = monitor.refresh_rate_millihertz()?;
		(1_000..=1_000_000)
			.contains(&millihertz)
			.then(|| Duration::from_secs_f64(1000.0 / f64::from(millihertz)))
	}
}
