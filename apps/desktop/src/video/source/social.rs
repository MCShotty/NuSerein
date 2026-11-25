//! Explicit Play only. Resolve public post metadata without a webview, cookies or Discord headers.
use super::WEB_PAGE;
use html5ever::tokenizer::{
	BufferQueue, StartTag, TagToken, Token, TokenSink, TokenSinkResult, Tokenizer, states::RawKind,
};
use std::{cell::RefCell, time::Duration};
use url::Url;

const MAX_METADATA: usize = 192 * 1024;
fn reddit(url: &Url) -> bool {
	matches!(
		url.host_str(),
		Some("v.redd.it" | "redd.it" | "reddit.com" | "www.reddit.com" | "old.reddit.com")
	)
}
fn media(page: &Url, raw: &str) -> Option<Url> {
	if raw.len() > 2048 || raw.contains('\\') || raw.chars().any(char::is_control) {
		return None;
	}
	let url = page.join(raw).ok()?;
	let host = url.host_str()?;
	let same_provider = if reddit(page) {
		host == "v.redd.it"
	} else {
		matches!(
			host,
			"oginstagram.com" | "www.oginstagram.com" | "cdninstagram.com" | "fbcdn.net"
		) || host.ends_with(".cdninstagram.com")
			|| host.ends_with(".fbcdn.net")
	};
	if !same_provider {
		return None;
	}
	ui::embedded_video_url(&model::EmbedMedia {
		url: Some(url.into()),
		..Default::default()
	})
}
fn reddit_post(page: &Url) -> Option<&str> {
	if !reddit(page) {
		return None;
	}
	let mut parts = page.path_segments()?;
	if page.host_str() == Some("redd.it") {
		return parts.next();
	}
	match parts.next()? {
		"mediaembed" | "comments" => parts.next(),
		"r" => {
			parts.next()?;
			(parts.next()? == "comments")
				.then(|| parts.next())
				.flatten()
		}
		_ => None,
	}
}
fn listing(page: &Url, post: &str, bytes: &[u8]) -> Option<Url> {
	#[derive(serde::Deserialize)]
	struct Listing {
		data: Children,
	}
	#[derive(serde::Deserialize)]
	struct Children {
		children: [Child; 1],
	}
	#[derive(serde::Deserialize)]
	struct Child {
		kind: String,
		data: Post,
	}
	#[derive(serde::Deserialize)]
	struct Post {
		id: String,
		secure_media: Option<Media>,
		media: Option<Media>,
	}
	#[derive(serde::Deserialize)]
	struct Media {
		reddit_video: Option<Video>,
	}
	#[derive(serde::Deserialize)]
	struct Video {
		fallback_url: String,
	}
	let Listing {
		data: Children { children: [child] },
	} = serde_json::from_slice(bytes).ok()?;
	if child.kind != "t3" || child.data.id != post {
		return None;
	}
	[child.data.secure_media, child.data.media]
		.into_iter()
		.flatten()
		.filter_map(|m| m.reddit_video)
		.find_map(|v| media(page, &v.fallback_url))
}
struct Metadata<'a> {
	page: &'a Url,
	target: RefCell<Option<Url>>,
}
impl TokenSink for Metadata<'_> {
	type Handle = ();
	fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
		if let TagToken(tag) = token
			&& tag.kind == StartTag
		{
			match tag.name.as_ref() {
				"script" => return TokenSinkResult::RawData(RawKind::ScriptData),
				"style" | "noscript" | "iframe" => {
					return TokenSinkResult::RawData(RawKind::Rawtext);
				}
				"title" | "textarea" => return TokenSinkResult::RawData(RawKind::Rcdata),
				"meta" if self.target.borrow().is_none() && !tag.had_duplicate_attributes => {
					let attr = |name: &str| {
						tag.attrs
							.iter()
							.find(|a| a.name.local.as_ref() == name)
							.map(|a| a.value.as_ref())
					};
					if matches!(
						attr("property").or_else(|| attr("name")),
						Some(
							"og:video"
								| "og:video:url" | "og:video:secure_url"
								| "twitter:player:stream"
						)
					) && let Some(target) = attr("content").and_then(|raw| media(self.page, raw))
					{
						self.target.replace(Some(target));
					}
				}
				_ => (),
			}
		}
		TokenSinkResult::Continue
	}
}
fn metadata(page: &Url, bytes: &[u8]) -> Option<Url> {
	let input = BufferQueue::default();
	input.push_back(String::from_utf8_lossy(bytes).as_ref().into());
	let tok = Tokenizer::new(
		Metadata {
			page,
			target: RefCell::new(None),
		},
		Default::default(),
	);
	let _ = tok.feed(&input);
	tok.end();
	tok.sink.target.into_inner()
}
pub(super) async fn resolve(mut page: Url) -> Result<Url, &'static str> {
	let client = reqwest::Client::builder()
		.no_gzip()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(8))
		.user_agent(concat!("NuSerein/", env!("CARGO_PKG_VERSION")))
		.build()
		.map_err(|_| WEB_PAGE)?;
	for _ in 0..3 {
		let post = reddit_post(&page);
		let endpoint = match post {
			Some(id) => {
				let mut api =
					Url::parse("https://www.reddit.com/api/info.json").map_err(|_| WEB_PAGE)?;
				api.query_pairs_mut()
					.append_pair("id", &format!("t3_{id}"))
					.append_pair("raw_json", "1");
				api
			}
			None => page.clone(),
		};
		let mut res = client
			.get(endpoint)
			.header(reqwest::header::ACCEPT_ENCODING, "identity")
			.send()
			.await
			.map_err(|_| WEB_PAGE)?;
		if res.status().is_redirection() && post.is_none() {
			let raw = res
				.headers()
				.get(reqwest::header::LOCATION)
				.and_then(|h| h.to_str().ok())
				.ok_or(WEB_PAGE)?;
			if let Some(url) = media(&page, raw) {
				return Ok(url);
			}
			let next = page
				.join(raw)
				.ok()
				.and_then(|u| ui::social_video_page(u.as_str()))
				.filter(|u| reddit(u) == reddit(&page))
				.ok_or(WEB_PAGE)?;
			page = next;
			continue;
		}
		if !res.status().is_success() {
			return Err(WEB_PAGE);
		}
		let is_json = res
			.headers()
			.get(reqwest::header::CONTENT_TYPE)
			.and_then(|h| h.to_str().ok())
			.and_then(|s| s.split(';').next())
			.is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"));
		if (post.is_some() && !is_json) || (post.is_none() && !super::web_page(res.headers())) {
			return Err(WEB_PAGE);
		}
		let mut bytes = Vec::new();
		while let Some(chunk) = res.chunk().await.map_err(|_| WEB_PAGE)? {
			let space = MAX_METADATA - bytes.len();
			if chunk.len() > space && post.is_some() {
				return Err(WEB_PAGE);
			}
			bytes.extend_from_slice(&chunk[..space.min(chunk.len())]);
			if bytes.len() == MAX_METADATA && post.is_none() {
				break;
			}
		}
		return match post {
			Some(id) => listing(&page, id, &bytes),
			None => metadata(&page, &bytes),
		}
		.ok_or(WEB_PAGE);
	}
	Err(WEB_PAGE)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reel_uses_public_metadata_not_script_or_arbitrary_media() {
		let page = ui::social_video_page("https://oginstagram.com/reel/fixture/?").unwrap();
		let raw = br#"<script>'<meta property="og:video" content="/offload/fake/1">'</script><meta property="og:video" content="https://example.invalid/track.mp4"><meta content='/offload/fixture/1?s=a%2Fb&amp;n=2' property='og:video:secure_url'>"#;
		assert_eq!(
			metadata(&page, raw).unwrap().as_str(),
			"https://oginstagram.com/offload/fixture/1?s=a%2Fb&n=2"
		);
		assert!(media(&page, "https://u:p@scontent.cdninstagram.com/a.mp4").is_none());
		assert!(ui::social_video_page("https://oginstagram.com.example/reel/fixture").is_none());
	}
	#[test]
	fn reddit_listing_must_match_the_selected_post_and_video_origin() {
		let page = ui::social_video_page("https://www.reddit.com/r/example/comments/abc/my_clip/")
			.unwrap();
		for raw in [
			page.as_str(),
			"https://redd.it/abc?tracking=unused",
			"https://www.reddit.com/comments/abc/title/",
		] {
			assert_eq!(
				reddit_post(&ui::social_video_page(raw).unwrap()),
				Some("abc")
			);
		}
		assert!(reddit_post(&ui::social_video_page("https://v.redd.it/abc").unwrap()).is_none());
		assert!(ui::social_video_page("https://redd.it/abc/other").is_none());
		let raw = br#"{"data":{"children":[{"kind":"t3","data":{"id":"abc","secure_media":{"reddit_video":{"fallback_url":"https://v.redd.it/clip/DASH_720.mp4?sig=a%2Fb"}}}}]}}"#;
		assert_eq!(
			listing(&page, "abc", raw).unwrap().query(),
			Some("sig=a%2Fb")
		);
		assert!(listing(&page, "xyz", raw).is_none());
		assert!(media(&page, "https://example.invalid/DASH_720.mp4").is_none());
	}
}
