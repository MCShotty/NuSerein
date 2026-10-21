//! Audited clipping container for an inline WKWebView; both views stay on the main thread.
#![allow(unsafe_code)]
use super::Bounds;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
use wry::WebViewExtMacOS;

pub(super) fn attach(view: &wry::WebView, html: &str) -> Result<Retained<NSView>, &'static str> {
	let mtm = MainThreadMarker::new().ok_or("video-provider-load-failed")?;
	let web = view.webview();
	// SAFETY: Wry attached this retained child to the application's main-thread NSView.
	let parent = unsafe { web.superview() }.ok_or("video-provider-load-failed")?;
	let base = NSURL::URLWithString(&NSString::from_str(super::APP_BASE))
		.ok_or("video-provider-load-failed")?;
	// SAFETY: the retained WKWebView, owned HTML and fixed base URL are live on
	// the AppKit main thread. Reparent only after navigation was accepted.
	let _navigation = unsafe { web.loadHTMLString_baseURL(&NSString::from_str(html), Some(&base)) }
		.ok_or("video-provider-load-failed")?;
	let clip = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
	clip.setClipsToBounds(true);
	web.removeFromSuperview();
	parent.addSubview(&clip);
	clip.addSubview(&web);
	Ok(clip)
}
pub(super) fn bounds(
	view: &wry::WebView,
	clip_view: &NSView,
	full: Bounds,
	clip: Bounds,
) -> Result<(), &'static str> {
	let _mtm = MainThreadMarker::new().ok_or("video-provider-load-failed")?;
	let window = clip_view.window().ok_or("video-provider-load-failed")?;
	let scale = window.backingScaleFactor();
	// SAFETY: attach installed this container in the application's main-thread NSView.
	let parent = unsafe { clip_view.superview() }.ok_or("video-provider-load-failed")?;
	let top = f64::from(clip.y) / scale;
	let height = f64::from(clip.height) / scale;
	let y = if parent.isFlipped() {
		top
	} else {
		parent.bounds().size.height - top - height
	};
	clip_view.setFrame(NSRect::new(
		NSPoint::new(f64::from(clip.x) / scale, y),
		NSSize::new(f64::from(clip.width) / scale, height),
	));
	view.set_bounds(wry::Rect {
		position: wry::dpi::PhysicalPosition::new(full.x - clip.x, full.y - clip.y).into(),
		size: wry::dpi::PhysicalSize::new(full.width, full.height).into(),
	})
	.map_err(|_| "video-provider-load-failed")
}
