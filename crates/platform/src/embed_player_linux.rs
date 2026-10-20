//! WebKitGTK owns a separate video window; it never borrows login's network session or bridge.
use super::{Arc, Instant, LOAD_TIMEOUT, REFERRER, YouTubeVideo, allowed_navigation};
use std::{cell::Cell, rc::Rc};
use webkit6::{glib, prelude::*};

#[derive(Default)]
struct Status {
	closed: Cell<bool>,
	failed: Cell<bool>,
	loaded: Cell<bool>,
}
pub struct ProviderPlayer {
	view: webkit6::WebView,
	window: gtk4::Window,
	display: gtk4::gdk::Display,
	status: Rc<Status>,
	opened: Instant,
}
impl ProviderPlayer {
	pub fn open(
		parent: Arc<winit::window::Window>,
		target: &YouTubeVideo,
		wake: impl Fn() + Send + Sync + 'static,
	) -> Result<Self, &'static str> {
		let _ = parent;
		Self::build(target, wake, None)
	}
	fn build(
		target: &YouTubeVideo,
		wake: impl Fn() + Send + Sync + 'static,
		fixture: Option<&str>,
	) -> Result<Self, &'static str> {
		gtk4::init().map_err(|_| "video-provider-load-failed")?;
		crate::ensure_webkit_locale().map_err(|_| "video-provider-load-failed")?;
		crate::ensure_gtk_application_id();
		let session = webkit6::NetworkSession::new_ephemeral();
		session.set_persistent_credential_storage_enabled(false);
		session.set_tls_errors_policy(webkit6::TLSErrorsPolicy::Fail);
		session.connect_download_started(|_, download| download.cancel());
		let settings = webkit6::Settings::new();
		settings.set_enable_developer_extras(false);
		settings.set_enable_write_console_messages_to_stdout(false);
		settings.set_allow_file_access_from_file_urls(false);
		settings.set_allow_universal_access_from_file_urls(false);
		settings.set_allow_top_navigation_to_data_urls(false);
		settings.set_allow_modal_dialogs(false);
		settings.set_javascript_can_open_windows_automatically(false);
		settings.set_javascript_can_access_clipboard(false);
		settings.set_enable_media_stream(false);
		settings.set_enable_webrtc(false);
		let view = webkit6::WebView::builder()
			.network_session(&session)
			.settings(&settings)
			.build();
		let selected = target.clone();
		view.connect_decide_policy(move |_, decision, kind| {
			let allowed = match kind {
				webkit6::PolicyDecisionType::NavigationAction => decision
					.downcast_ref::<webkit6::NavigationPolicyDecision>()
					.and_then(|d| d.navigation_action())
					.and_then(|a| a.request())
					.and_then(|r| r.uri())
					.is_some_and(|uri| allowed_navigation(&uri, &selected)),
				webkit6::PolicyDecisionType::Response => decision
					.downcast_ref::<webkit6::ResponsePolicyDecision>()
					.is_some_and(|d| {
						d.is_mime_type_supported()
							&& (!d.is_main_frame_main_resource()
								|| d.request()
									.and_then(|r| r.uri())
									.is_some_and(|uri| allowed_navigation(&uri, &selected)))
					}),
				_ => false,
			};
			if allowed {
				decision.use_();
			} else {
				decision.ignore();
			}
			true
		});
		view.connect_create(|_, _| None);
		view.connect_permission_request(|_, request| {
			request.deny();
			true
		});
		view.connect_query_permission_state(|_, query| {
			query.finish(webkit6::PermissionState::Denied);
			true
		});
		view.connect_run_file_chooser(|_, request| {
			request.cancel();
			true
		});
		view.connect_authenticate(|_, request| {
			request.cancel();
			true
		});
		view.connect_context_menu(|_, _, _| true);
		view.connect_print(|_, _| true);
		view.connect_show_notification(|_, _| true);
		let status = Rc::new(Status::default());
		let loaded = Rc::downgrade(&status);
		view.connect_load_changed(move |_, event| {
			if event == webkit6::LoadEvent::Finished
				&& let Some(s) = loaded.upgrade()
			{
				s.loaded.set(true);
			}
		});
		let failed = Rc::downgrade(&status);
		view.connect_load_failed(move |_, _, _, _| {
			if let Some(s) = failed.upgrade() {
				s.failed.set(true);
			}
			true
		});
		let failed = Rc::downgrade(&status);
		view.connect_web_process_terminated(move |_, _| {
			if let Some(s) = failed.upgrade() {
				s.failed.set(true);
			}
		});
		let window = gtk4::Window::builder()
			.title("YouTube · Serein")
			.default_width(960)
			.default_height(540)
			.child(&view)
			.build();
		let closed = Rc::downgrade(&status);
		let stop = view.downgrade();
		window.connect_close_request(move |_| {
			if let Some(s) = closed.upgrade() {
				s.closed.set(true);
			}
			if let Some(view) = stop.upgrade() {
				view.stop_loading();
				view.terminate_web_process();
			}
			wake();
			glib::Propagation::Proceed
		});
		if let Some(html) = fixture {
			view.load_html(html, Some(&target.embed_url()));
		} else {
			let request = webkit6::URIRequest::new(&target.embed_url());
			if let Some(headers) = request.http_headers() {
				headers.replace("Referer", REFERRER);
			}
			view.load_request(&request);
		}
		window.present();
		let display = gtk4::prelude::WidgetExt::display(&window);
		Ok(Self {
			view,
			window,
			display,
			status,
			opened: Instant::now(),
		})
	}
	pub fn set_bounds(&self, _: i32, _: i32, _: u32, _: u32) -> Result<(), &'static str> {
		Ok(())
	}
	pub fn poll(&self) -> Result<bool, &'static str> {
		let context = glib::MainContext::default();
		for _ in 0..8 {
			if !context.pending() {
				break;
			}
			context.iteration(false);
		}
		self.display.flush();
		if self.status.closed.get() {
			Ok(false)
		} else if self.status.failed.get()
			|| (!self.status.loaded.get() && self.opened.elapsed() >= LOAD_TIMEOUT)
		{
			Err("video-provider-load-failed")
		} else {
			Ok(true)
		}
	}
}
impl Drop for ProviderPlayer {
	fn drop(&mut self) {
		self.status.closed.set(true);
		self.view.stop_loading();
		self.view.terminate_web_process();
		self.window.set_child(None::<&gtk4::Widget>);
		self.window.destroy();
		self.display.flush();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	#[ignore = "requires a native display and WebKit; invoked separately with a network-free document"]
	fn native_provider_fixture_releases_its_view() {
		const TEST: &str = "embed_player::linux::tests::native_provider_fixture_releases_its_view";
		if std::env::var_os("SEREIN_PROVIDER_FIXTURE_CHILD").is_none() {
			assert!(
				std::process::Command::new(std::env::current_exe().unwrap())
					.args([TEST, "--exact", "--ignored", "--test-threads=1"])
					.env("SEREIN_PROVIDER_FIXTURE_CHILD", "1")
					.status()
					.unwrap()
					.success()
			);
			return;
		}
		let target = YouTubeVideo::new("synthet1c01", 0).unwrap();
		let player = ProviderPlayer::build(&target, || {}, Some("<!doctype html><meta http-equiv=Content-Security-Policy content=\"default-src 'none'\"><p>Offline synthetic provider player</p>")).unwrap();
		let weak = player.view.downgrade();
		let status = Rc::downgrade(&player.status);
		let deadline = Instant::now() + std::time::Duration::from_secs(10);
		while !player.status.loaded.get() {
			assert!(player.poll().unwrap());
			assert!(Instant::now() < deadline, "offline player did not load");
			std::thread::sleep(std::time::Duration::from_millis(10));
		}
		player.window.close();
		assert!(!player.poll().unwrap());
		drop(player);
		let context = glib::MainContext::default();
		for _ in 0..50 {
			for _ in 0..8 {
				if !context.pending() {
					break;
				}
				context.iteration(false);
			}
			if weak.upgrade().is_none() {
				break;
			}
			std::thread::sleep(std::time::Duration::from_millis(10));
		}
		assert!(
			weak.upgrade().is_none(),
			"provider view retained after closing"
		);
		assert!(
			status.upgrade().is_none(),
			"provider status retained after closing"
		);
	}
}
