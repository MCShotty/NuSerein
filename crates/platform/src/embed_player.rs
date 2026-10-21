//! One explicitly opened public provider player, isolated from Discord authentication.
use model::YouTubeVideo;
#[cfg(not(target_os = "linux"))]
use std::sync::Mutex;
use std::{
	sync::Arc,
	time::{Duration, Instant},
};

pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const WRAPPER: &str = "serein-player://localhost/";
pub(super) const APP_BASE: &str = "https://github.com/ViceVerse-cz/Serein/player/";

#[cfg(target_os = "linux")]
#[path = "embed_player_linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::ProviderPlayer;

pub(super) fn wrapper_url(raw: &str) -> bool {
	url::Url::parse(raw).is_ok_and(|url| {
		matches!(
			url.as_str(),
			WRAPPER | APP_BASE | "https://serein-player.localhost/"
		)
	})
}
pub(super) fn player_html(target: &YouTubeVideo) -> String {
	let video = serde_json::json!({"id":target.id(), "start":target.start_seconds()});
	include_str!("embed_player.html").replace("__VIDEO__", &video.to_string())
}
/// Provider navigation cannot leave the selected player or open a general browser.
pub(super) fn allowed_navigation(raw: &str, target: &YouTubeVideo) -> bool {
	if wrapper_url(raw) {
		return true;
	}
	let Ok(url) = url::Url::parse(raw) else {
		return false;
	};
	url.scheme() == "https"
		&& url.host_str() == Some("www.youtube-nocookie.com")
		&& url.port_or_known_default() == Some(443)
		&& url.username().is_empty()
		&& url.password().is_none()
		&& url.fragment().is_none()
		&& url.path() == format!("/embed/{}", target.id())
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PlaybackState {
	#[default]
	Loading,
	Ready,
	Playing,
	Paused,
	Buffering,
	Ended,
	Failed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackStatus {
	pub state: PlaybackState,
	pub position: f64,
	pub error: Option<&'static str>,
}
pub(super) fn status_message(raw: &str) -> Option<PlaybackStatus> {
	if raw.len() > 256 {
		return None;
	}
	let data: serde_json::Value = serde_json::from_str(raw).ok()?;
	let position = data.get("position")?.as_f64()?;
	if !position.is_finite() || !(0.0..=86400.0).contains(&position) {
		return None;
	}
	let state = match data.get("state")?.as_str()? {
		"loading" => PlaybackState::Loading,
		"ready" => PlaybackState::Ready,
		"playing" => PlaybackState::Playing,
		"paused" => PlaybackState::Paused,
		"buffering" => PlaybackState::Buffering,
		"ended" => PlaybackState::Ended,
		"failed" => PlaybackState::Failed,
		_ => return None,
	};
	let error = if state == PlaybackState::Failed {
		Some(match data.get("error")?.as_u64()? {
			100 => "video-provider-unavailable",
			101 | 150 => "video-provider-embedding-disabled",
			153 => "video-provider-identity-failed",
			_ => "video-provider-load-failed",
		})
	} else {
		None
	};
	Some(PlaybackStatus {
		state,
		position,
		error,
	})
}
pub(super) struct Progress {
	opened: Instant,
	advanced: Instant,
	position: f64,
	started: bool,
}
impl Default for Progress {
	fn default() -> Self {
		Self {
			opened: Instant::now(),
			advanced: Instant::now(),
			position: 0.0,
			started: false,
		}
	}
}
impl Progress {
	pub(super) fn suspend(&mut self, now: Instant) {
		self.opened = now;
		self.advanced = now;
	}
	pub(super) fn check(
		&mut self,
		status: PlaybackStatus,
		now: Instant,
	) -> Result<(), &'static str> {
		if let Some(error) = status.error {
			return Err(error);
		}
		if status.state == PlaybackState::Playing && !self.started {
			self.started = true;
			self.advanced = now;
		}
		if status.position != self.position
			|| matches!(status.state, PlaybackState::Paused | PlaybackState::Ended)
		{
			self.advanced = now;
			self.position = status.position;
		}
		if !self.started
			&& !matches!(status.state, PlaybackState::Paused | PlaybackState::Ended)
			&& now.duration_since(self.opened) >= LOAD_TIMEOUT
		{
			return Err("video-provider-load-failed");
		}
		if self.started
			&& matches!(
				status.state,
				PlaybackState::Loading | PlaybackState::Buffering | PlaybackState::Playing
			) && now.duration_since(self.advanced) >= Duration::from_secs(15)
		{
			return Err("video-provider-buffering-failed");
		}
		Ok(())
	}
}
/// Physical pixels relative to the main application window. Clipping never resizes the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
	pub x: i32,
	pub y: i32,
	pub width: u32,
	pub height: u32,
}

#[cfg(not(target_os = "linux"))]
pub struct ProviderPlayer {
	view: wry::WebView,
	_context: wry::WebContext,
	_storage: tempfile::TempDir,
	_parent: Arc<winit::window::Window>,
	status: Arc<Mutex<PlaybackStatus>>,
	progress: std::cell::RefCell<Progress>,
	visible: std::cell::Cell<bool>,
	visibility_applied: std::cell::Cell<bool>,
	#[cfg(target_os = "macos")]
	clip: objc2::rc::Retained<objc2_app_kit::NSView>,
}
#[cfg(not(target_os = "linux"))]
impl ProviderPlayer {
	pub fn open(
		parent: Arc<winit::window::Window>,
		target: &YouTubeVideo,
		wake: impl Fn() + Send + Sync + 'static,
	) -> Result<Self, &'static str> {
		let status = Arc::new(Mutex::new(PlaybackStatus::default()));
		let events = status.clone();
		let html = player_html(target);
		let selected = target.clone();
		let storage = tempfile::Builder::new()
			.prefix("serein-video-")
			.tempdir()
			.map_err(|_| "video-provider-load-failed")?;
		let mut context = wry::WebContext::new(Some(storage.path().to_owned()));
		let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
			.with_incognito(true)
			.with_visible(false)
			.with_devtools(false)
			.with_autoplay(true)
			.with_permission_handler(|_| wry::PermissionResponse::Deny)
			.with_navigation_handler(move |url| allowed_navigation(&url, &selected))
			.with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
			.with_download_started_handler(|_, _| false)
			.with_ipc_handler(move |request| {
				if wrapper_url(&request.uri().to_string())
					&& let Some(update) = status_message(request.body())
				{
					if let Ok(mut current) = events.lock() {
						*current = update;
					}
					wake();
				}
			});
		#[cfg(target_os = "windows")]
		let builder = {
			use wry::WebViewBuilderExtWindows;
			builder
				.with_custom_protocol("serein-player".into(), move |_, request| {
					let ok =
						request.uri().path() == "/" && request.method() == wry::http::Method::GET;
					wry::http::Response::builder()
						.status(if ok { 200 } else { 404 })
						.header("Content-Type", "text/html; charset=utf-8")
						.header("Cache-Control", "no-store")
						.body(std::borrow::Cow::Owned(if ok {
							html.as_bytes().to_vec()
						} else {
							Vec::new()
						}))
						.expect("static response")
				})
				.with_https_scheme(true)
				.with_url(WRAPPER)
		};
		let view = builder
			.build_as_child(parent.as_ref())
			.map_err(|_| "video-provider-load-failed")?;
		#[cfg(target_os = "macos")]
		let clip = macos::attach(&view, &html)?;
		Ok(Self {
			view,
			_context: context,
			_storage: storage,
			_parent: parent,
			status,
			progress: Default::default(),
			visible: std::cell::Cell::new(false),
			visibility_applied: std::cell::Cell::new(false),
			#[cfg(target_os = "macos")]
			clip,
		})
	}
	pub fn set_bounds(&self, full: Bounds, clip: Bounds) -> Result<(), &'static str> {
		#[cfg(target_os = "windows")]
		{
			self.view
				.set_bounds(wry::Rect {
					position: wry::dpi::PhysicalPosition::new(full.x, full.y).into(),
					size: wry::dpi::PhysicalSize::new(full.width, full.height).into(),
				})
				.map_err(|_| "video-provider-load-failed")?;
			windows_clip(&self.view, full, clip)?;
		}
		#[cfg(target_os = "macos")]
		macos::bounds(&self.view, &self.clip, full, clip)?;
		Ok(())
	}
	pub fn set_visible(&self, visible: bool) -> Result<(), &'static str> {
		let ready = self.status().state != PlaybackState::Loading;
		let changed = self.visible.replace(visible) != visible;
		// Reapply once after API readiness if the command preceded document initialization.
		if changed || (ready && !self.visibility_applied.get()) {
			self.view
				.evaluate_script(if visible {
					"window.sereinVisibility?.(true)"
				} else {
					"window.sereinVisibility?.(false)"
				})
				.map_err(|_| "video-provider-load-failed")?;
			self.visibility_applied.set(ready);
		}
		if changed {
			self.view
				.set_visible(visible)
				.map_err(|_| "video-provider-load-failed")?;
		}
		Ok(())
	}

	pub fn status(&self) -> PlaybackStatus {
		self.status.lock().map(|s| *s).unwrap_or_default()
	}
	pub fn poll(&self) -> Result<bool, &'static str> {
		let status = self.status();
		let mut progress = self.progress.borrow_mut();
		if !self.visible.get() {
			progress.suspend(Instant::now());
		}
		progress.check(status, Instant::now())?;
		Ok(true)
	}
}
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_clip(view: &wry::WebView, full: Bounds, clip: Bounds) -> Result<(), &'static str> {
	use windows::Win32::{
		Foundation::HWND,
		Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn},
	};
	use wry::WebViewExtWindows;
	// The region belongs to the OS after SetWindowRgn succeeds. Coordinates are local to the full page.
	unsafe {
		let region = CreateRectRgn(
			clip.x - full.x,
			clip.y - full.y,
			clip.x - full.x + clip.width as i32,
			clip.y - full.y + clip.height as i32,
		);
		if region.is_invalid() {
			return Err("video-provider-load-failed");
		}
		if SetWindowRgn(HWND(view.hwnd().0), Some(region), true) == 0 {
			let _ = DeleteObject(region.into());
			return Err("video-provider-load-failed");
		}
	}
	Ok(())
}
#[cfg(target_os = "macos")]
#[path = "embed_player_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
impl Drop for ProviderPlayer {
	fn drop(&mut self) {
		self.clip.removeFromSuperview();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn player_events_are_bounded_and_page_load_does_not_establish_playback() {
		assert!(status_message(r#"{"state":"playing","position":1.0,"error":0}"#).is_some());
		for raw in [
			r#"{"state":"playing","position":-1}"#,
			r#"{"state":"navigation","position":1}"#,
			r#"{"state":"playing","position":9999999}"#,
		] {
			assert!(status_message(raw).is_none());
		}
		let denied = status_message(r#"{"state":"failed","position":0,"error":150}"#).unwrap();
		assert_eq!(denied.error, Some("video-provider-embedding-disabled"));
		let mut progress = Progress::default();
		let now = progress.opened;
		assert!(
			progress
				.check(
					PlaybackStatus {
						state: PlaybackState::Ready,
						..Default::default()
					},
					now + LOAD_TIMEOUT
				)
				.is_err()
		);
		let mut progress = Progress::default();
		let now = progress.opened;
		assert!(
			progress
				.check(
					PlaybackStatus {
						state: PlaybackState::Paused,
						..Default::default()
					},
					now + LOAD_TIMEOUT
				)
				.is_ok()
		);
	}

	#[test]
	fn provider_navigation_stays_on_the_selected_public_video() {
		let video = YouTubeVideo::new("fT6TIXnmv-s", 12).unwrap();
		assert!(allowed_navigation(&video.embed_url(), &video));
		assert!(wrapper_url("https://serein-player.localhost"));
		for raw in [
			"https://www.youtube-nocookie.com/",
			"https://www.youtube.com/signin",
			"https://www.youtube-nocookie.com.evil.test/embed/fT6TIXnmv-s",
			"file:///tmp/video.html",
			"https://discord.com/login",
			"about:blank",
		] {
			assert!(!allowed_navigation(raw, &video), "{raw}");
		}
		assert!(!allowed_navigation(
			&YouTubeVideo::new("synthet1c01", 12).unwrap().embed_url(),
			&video
		));
	}
}
