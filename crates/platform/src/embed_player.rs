//! One explicitly opened public provider player, isolated from Discord authentication.
use model::YouTubeVideo;
use std::{
	sync::Arc,
	time::{Duration, Instant},
};

/// Identify this application to the official embedded player; never forward a message URL,
/// Discord authorization, cookies, user ID or channel ID as its referrer.
pub(super) const REFERRER: &str = "https://github.com/ViceVerse-cz/Serein/";
pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "linux")]
#[path = "embed_player_linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::ProviderPlayer;

/// Navigation is pinned to the selected player. Provider links, sign-in and popups cannot
/// turn this surface into a general browser. Subresource requests belong to the web engine.
pub(super) fn allowed_navigation(raw: &str, target: &YouTubeVideo) -> bool {
	let Ok(url) = url::Url::parse(raw) else {
		return false;
	};
	let expected = target.embed_url();
	let expected = url::Url::parse(&expected).expect("typed player URL");
	url.scheme() == "https"
		&& url.host_str() == expected.host_str()
		&& url.port_or_known_default() == Some(443)
		&& url.username().is_empty()
		&& url.password().is_none()
		&& url.path() == expected.path()
		&& url.query() == expected.query()
		&& url.fragment().is_none()
}

#[cfg(not(target_os = "linux"))]
pub struct ProviderPlayer {
	view: wry::WebView,
	_context: wry::WebContext,
	_storage: tempfile::TempDir,
	_parent: Arc<winit::window::Window>,
	loaded: Arc<std::sync::atomic::AtomicBool>,
	opened: Instant,
}
#[cfg(not(target_os = "linux"))]
impl ProviderPlayer {
	pub fn open(
		parent: Arc<winit::window::Window>,
		target: &YouTubeVideo,
		wake: impl Fn() + Send + Sync + 'static,
	) -> Result<Self, &'static str> {
		let loaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
		let finished = loaded.clone();
		let selected = target.clone();
		let mut headers = wry::http::HeaderMap::new();
		headers.insert(
			wry::http::header::REFERER,
			wry::http::HeaderValue::from_static(REFERRER),
		);
		let storage = tempfile::Builder::new()
			.prefix("serein-video-")
			.tempdir()
			.map_err(|_| "video-provider-load-failed")?;
		let mut context = wry::WebContext::new(Some(storage.path().to_owned()));
		let view = wry::WebViewBuilder::new_with_web_context(&mut context)
			.with_url_and_headers(target.embed_url(), headers)
			.with_incognito(true)
			.with_visible(false)
			.with_devtools(false)
			.with_permission_handler(|_| wry::PermissionResponse::Deny)
			.with_navigation_handler(move |url| allowed_navigation(&url, &selected))
			.with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
			.with_download_started_handler(|_, _| false)
			.with_on_page_load_handler(move |event, _| {
				if matches!(event, wry::PageLoadEvent::Finished) {
					finished.store(true, std::sync::atomic::Ordering::Release);
					wake();
				}
			})
			.build_as_child(parent.as_ref())
			.map_err(|_| "video-provider-load-failed")?;
		Ok(Self {
			view,
			_context: context,
			_storage: storage,
			_parent: parent,
			loaded,
			opened: Instant::now(),
		})
	}
	pub fn set_bounds(&self, x: i32, y: i32, width: u32, height: u32) -> Result<(), &'static str> {
		self.view
			.set_bounds(wry::Rect {
				position: wry::dpi::PhysicalPosition::new(x, y).into(),
				size: wry::dpi::PhysicalSize::new(width, height).into(),
			})
			.map_err(|_| "video-provider-load-failed")?;
		self.view
			.set_visible(width > 0 && height > 0)
			.map_err(|_| "video-provider-load-failed")
	}
	/// `false` means the native window was closed. Loading failures are separately visible.
	pub fn poll(&self) -> Result<bool, &'static str> {
		if !self.loaded.load(std::sync::atomic::Ordering::Acquire)
			&& self.opened.elapsed() >= LOAD_TIMEOUT
		{
			Err("video-provider-load-failed")
		} else {
			Ok(true)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn provider_navigation_stays_on_the_selected_public_video() {
		let video = YouTubeVideo::new("fT6TIXnmv-s", 12).unwrap();
		assert!(allowed_navigation(&video.embed_url(), &video));
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
		assert!(!allowed_navigation(
			&YouTubeVideo::new("fT6TIXnmv-s", 0).unwrap().embed_url(),
			&video
		));
	}
}
