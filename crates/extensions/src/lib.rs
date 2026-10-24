//! Versioned, bounded local extensions. No Discord or native platform access is exposed.
mod validation;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod runtime;
pub use runtime::invoke;
mod rich_presence;
pub use rich_presence::*;
mod voice_messages;
pub use voice_messages::*;
mod discovery;
pub use discovery::*;
mod conversation_activity;
pub use conversation_activity::*;
mod message_content;
pub use message_content::*;
mod forum_data;
pub use forum_data::*;
mod channel_metadata;
pub use channel_metadata::*;
mod member_details;
pub use member_details::*;
mod app;
pub use app::*;
mod extended;
pub use extended::*;

pub const API_VERSION: u32 = 1;
pub const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MODULE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BACKGROUND_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PREVIEW_BYTES: usize = 256 * 1024;
pub const MAX_CATALOG_BYTES: usize = 1024 * 1024;
pub const MAX_IO_BYTES: usize = 256 * 1024;
pub const MAX_EVENT_CONTENT_BYTES: usize = 16 * 1024;
pub const MAX_STORAGE_BYTES: usize = 1024 * 1024;
pub const MAX_PLUGINS: usize = 8;
pub const MAX_PANEL_ELEMENTS: usize = 64;
pub const MAX_CAPABILITIES: usize = 64;
/// Minimum delay between completed host-scheduled appearance ticks.
pub const TICK_MIN_INTERVAL_MS: u64 = 250;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Extension exceeds its resource limit")]
	Limit,
	#[error("Invalid extension document")]
	Invalid,
	#[error("Unsupported extension API version")]
	Version,
	#[error("Extension capability was not granted")]
	Capability,
	#[error("Invalid or unsupported WebAssembly module")]
	Module,
	#[error("Extension execution could not start; check its Wasm exports and runtime requirements")]
	Execution,
	#[error("Extension exhausted its execution fuel; reduce handler work or requested data")]
	Fuel,
	#[error(
		"Extension memory or table allocation failed; reduce allocations within the sandbox limits"
	)]
	Memory,
	#[error("Extension exhausted its call stack; reduce recursion and stack allocations")]
	Stack,
	#[error(
		"Extension handler trapped; check for panics, invalid memory access or arithmetic errors"
	)]
	Trap,
	#[error("Extension input is invalid; check the action and input field schema")]
	Input,
	#[error(
		"Extension input exceeds its limits; reduce requested data, form values or saved storage"
	)]
	InputLimit,
	#[error("Extension response exceeds its limits; reduce panel elements, text or saved storage")]
	OutputLimit,
	#[error("Extension returned no response; check SDK input decoding and output serialization")]
	Handler,
	#[error("Extension returned an invalid response; check the output JSON schema and ABI buffer")]
	Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
	Plugin,
	Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
	ApiProxy,
	RichPresence,
	VoiceMessages,
	RelationshipControl,
	AccountControl,
	AudioSettings,
	VoiceConnect,
	CameraControl,

	MessageSend,
	MessageManage,
	ReactionsControl,
	ReadStateControl,
	ThreadsControl,
	ChannelControl,
	ServerControl,
	RoleControl,
	ModerationControl,
	MediaControl,
	ActionFeedback,
	DataQueries,
	MessagingSettings,
	GuildFolders,

	MessageContent,
	ForumData,
	ConversationActivity,
	ChannelMetadata,
	MemberDetails,
	SelectedMessage,
	Composer,
	Storage,
	DeletedMessages,
	ImageSharing,
	Appearance,
	MessageEvents,
	AppContext,
	ChannelDirectory,
	Timeline,
	Members,
	Presence,
	VoiceState,
	ReadState,
	LocalSettings,
	NotificationSettings,
	Navigation,
	LocalNotices,
	ClipboardWrite,
	VoiceControl,
	AppEvents,
	AccountProfile,
	GuildDirectory,
	ChannelDetails,
	DataEvents,
	MessageDetails,
	Relationships,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
	Message,
	Composer,
	Panel,
	Activation,
	MessageEvent,
	AppEvent,
	/// Host-scheduled appearance update. Each call is a fresh Wasm invocation.
	Tick,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
	pub id: String,
	pub label: String,
	pub surface: Surface,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
	pub api_version: u32,
	pub id: String,
	pub name: String,
	pub version: String,
	pub author: String,
	pub license: String,
	pub source: String,
	pub kind: ExtensionKind,
	#[serde(default)]
	pub capabilities: Vec<Capability>,
	#[serde(default)]
	pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub background: Option<Background>,
	#[serde(default)]
	pub colors: BTreeMap<String, String>,
	#[serde(default)]
	pub backdrop: Option<[String; 2]>,
}

/// Image settings for the package's single embedded static background.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Background {
	pub opacity: u8,
	pub fit: BackgroundFit,
	pub target: BackgroundTarget,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub sections: Option<SectionOpacity>,
}

impl Default for Background {
	fn default() -> Self {
		Self {
			opacity: 25,
			fit: BackgroundFit::Cover,
			target: BackgroundTarget::Window,
			sections: None,
		}
	}
}

/// Surface coverage over one continuous window image; text and controls stay opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SectionOpacity {
	pub top_bar: u8,
	pub server_list: u8,
	pub channel_list: u8,
	pub message_list: u8,
	pub member_list: u8,
	pub composer: u8,
}
impl Default for SectionOpacity {
	fn default() -> Self {
		Self {
			top_bar: 85,
			server_list: 85,
			channel_list: 85,
			message_list: 75,
			member_list: 85,
			composer: 90,
		}
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundFit {
	#[default]
	Cover,
	Contain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTarget {
	#[default]
	Window,
	Chat,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
	#[serde(default)]
	pub light: ThemePalette,
	#[serde(default)]
	pub dark: ThemePalette,
	#[serde(default)]
	pub style: ThemeStyle,
}

/// Native control metrics in logical pixels. Omitted fields inherit the active theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeStyle {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub transparency_blur: Option<bool>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub transparency: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub blur: Option<u8>,
	/// Legacy switch: transparency now always covers every surface. Older themes that still
	/// set it keep parsing; the value is ignored and never written back.
	#[serde(skip_serializing)]
	pub transparent_all: Option<bool>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub body_size: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub heading_size: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub button_size: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub small_size: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub monospace_size: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub item_spacing: Option<[u8; 2]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub button_padding: Option<[u8; 2]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub control_height: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub widget_radius: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub window_radius: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub menu_radius: Option<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub background_image: Vec<u8>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub cover_image: Vec<u8>,
	pub manifest: Manifest,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub theme: Option<Theme>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub wasm: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
	pub api_version: u32,
	pub entries: Vec<CatalogEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
	#[serde(default)]
	pub description: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub preview: Option<Preview>,
	pub manifest: Manifest,
	pub release_url: String,
	pub sha256: String,
	pub download_bytes: u64,
	pub source_commit: String,
}

/// A creator-supplied shop image, pinned independently from executable packages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
	pub url: String,
	pub sha256: String,
	pub download_bytes: u64,
}

impl Preview {
	pub fn validate(&self) -> Result<(), Error> {
		if !valid_https_url(&self.url)
			|| self.sha256.len() != 64
			|| !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
			|| self.download_bytes == 0
			|| self.download_bytes > MAX_PREVIEW_BYTES as u64
		{
			return Err(Error::Invalid);
		}
		Ok(())
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
	pub action: String,
	#[serde(default)]
	pub selected_message: Option<String>,
	#[serde(default)]
	pub composer: Option<String>,
	#[serde(default)]
	pub storage: Option<String>,
	#[serde(default)]
	pub values: BTreeMap<String, String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub message_event: Option<Box<MessageEvent>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub app: Option<Box<AppSnapshot>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub app_event: Option<AppEventKind>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub action_result: Option<ActionResult>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub queries: Option<Box<QuerySnapshot>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub messaging_settings: Option<Box<MessagingSettingsSnapshot>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub guild_folders: Option<Box<GuildFoldersSnapshot>>,
	/// Milliseconds elapsed since this plugin was enabled for the current session.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tick_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageEventKind {
	Create,
	Update,
	Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageEvent {
	pub kind: MessageEventKind,
	pub channel_id: String,
	pub message_id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub author_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub content: Option<String>,
}

impl MessageEvent {
	pub fn validate(&self) -> Result<(), Error> {
		for id in [&self.channel_id, &self.message_id]
			.into_iter()
			.chain(self.author_id.iter())
		{
			app::entity_id(id)?;
		}
		if self
			.content
			.as_ref()
			.is_some_and(|content| content.len() > MAX_EVENT_CONTENT_BYTES)
		{
			return Err(Error::Limit);
		}
		match self.kind {
			MessageEventKind::Create if self.author_id.is_none() || self.content.is_none() => {
				Err(Error::Invalid)
			}
			MessageEventKind::Delete if self.author_id.is_some() || self.content.is_some() => {
				Err(Error::Invalid)
			}
			_ => Ok(()),
		}
	}
}

/// Connection-scoped REST API routing. No credentials or account data are exposed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApiProxyConfig {
	#[default]
	Direct,
	Automatic,
	Url {
		url: String,
	},
}
impl<'de> Deserialize<'de> for ApiProxyConfig {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
		enum Wire {
			Direct {},
			Automatic {},
			Url { url: String },
		}
		Ok(match Wire::deserialize(deserializer)? {
			Wire::Direct {} => Self::Direct,
			Wire::Automatic {} => Self::Automatic,
			Wire::Url { url } => Self::Url { url },
		})
	}
}
impl ApiProxyConfig {
	pub fn validate(&self) -> Result<(), Error> {
		if let Self::Url { url } = self {
			if url.contains('@')
				|| url.contains('\\')
				|| url.len() > 2048
				|| url
					.chars()
					.any(|c| c.is_control() || c.is_ascii_whitespace())
			{
				return Err(Error::Invalid);
			}
			let authority = url
				.split_once("://")
				.map(|(_, tail)| tail)
				.ok_or(Error::Invalid)?;
			if authority
				.strip_suffix('/')
				.unwrap_or(authority)
				.contains('/')
			{
				return Err(Error::Invalid);
			}
			let parsed = url::Url::parse(url).map_err(|_| Error::Invalid)?;
			if !matches!(parsed.scheme(), "http" | "https")
				|| parsed.host_str().is_none()
				|| !parsed.username().is_empty()
				|| parsed.password().is_some()
				|| parsed.path() != "/"
				|| parsed.query().is_some()
				|| parsed.fragment().is_some()
			{
				return Err(Error::Invalid);
			}
		}
		Ok(())
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub api_proxy: Option<ApiProxyConfig>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub rich_presence: Option<RichPresenceUpdate>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub voice_messages: Option<VoiceMessagesConfig>,
	#[serde(default)]
	pub image_sharing: bool,
	#[serde(default)]
	pub preserve_deleted_messages: bool,
	#[serde(default)]
	pub appearance: Option<Theme>,
	#[serde(default)]
	pub replacement: Option<String>,
	#[serde(default)]
	pub panel: Vec<Element>,
	#[serde(default)]
	pub storage: Option<String>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub effects: Vec<HostEffect>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Element {
	ActivityPreview {
		presence: Box<CustomRichPresence>,
	},
	Text {
		text: String,
	},
	Heading {
		text: String,
	},
	Separator,
	Row {
		children: Vec<Element>,
	},
	Button {
		id: String,
		label: String,
	},
	TextInput {
		id: String,
		label: String,
		value: String,
	},
	Checkbox {
		id: String,
		label: String,
		checked: bool,
	},
	Select {
		id: String,
		label: String,
		options: Vec<String>,
		value: String,
	},
	Slider {
		id: String,
		label: String,
		min: i32,
		max: i32,
		value: i32,
	},
}

pub fn valid_id(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 64
		&& value
			.bytes()
			.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		&& value.as_bytes()[0].is_ascii_alphanumeric()
		&& !matches!(
			value,
			"con"
				| "prn" | "aux"
				| "nul" | "com1"
				| "com2" | "com3"
				| "com4" | "com5"
				| "com6" | "com7"
				| "com8" | "com9"
				| "lpt1" | "lpt2"
				| "lpt3" | "lpt4"
				| "lpt5" | "lpt6"
				| "lpt7" | "lpt8"
				| "lpt9"
		)
}

/// Metadata links never carry credentials and are opened only through an explicit host action.
pub fn valid_https_url(value: &str) -> bool {
	value.len() <= 2048
		&& url::Url::parse(value).is_ok_and(|url| {
			url.scheme() == "https"
				&& url.host_str().is_some()
				&& url.username().is_empty()
				&& url.password().is_none()
		})
}

pub fn parse_color(value: &str) -> Result<[u8; 4], Error> {
	let bytes = value.as_bytes();
	if !matches!(bytes.len(), 7 | 9)
		|| bytes[0] != b'#'
		|| !bytes[1..].iter().all(u8::is_ascii_hexdigit)
	{
		return Err(Error::Invalid);
	}
	let mut rgba = [0, 0, 0, 255];
	for (i, pair) in bytes[1..].as_chunks::<2>().0.iter().enumerate() {
		let digit = |b: u8| {
			if b.is_ascii_digit() {
				b - b'0'
			} else {
				b.to_ascii_lowercase() - b'a' + 10
			}
		};
		rgba[i] = digit(pair[0]) * 16 + digit(pair[1]);
	}
	Ok(rgba)
}

impl Theme {
	/// Apply only explicitly supplied values, preserving the underlying theme's other tokens.
	pub fn overlay(&mut self, other: &Self) {
		for (base, overlay) in [
			(&mut self.light, &other.light),
			(&mut self.dark, &other.dark),
		] {
			base.colors.extend(overlay.colors.clone());
			base.background = overlay.background.or(base.background);
			if overlay.backdrop.is_some() {
				base.backdrop.clone_from(&overlay.backdrop);
			}
		}
		self.style.body_size = other.style.body_size.or(self.style.body_size);
		self.style.transparency_blur = other
			.style
			.transparency_blur
			.or(self.style.transparency_blur);
		self.style.transparency = other.style.transparency.or(self.style.transparency);
		self.style.blur = other.style.blur.or(self.style.blur);
		self.style.heading_size = other.style.heading_size.or(self.style.heading_size);
		self.style.button_size = other.style.button_size.or(self.style.button_size);
		self.style.small_size = other.style.small_size.or(self.style.small_size);
		self.style.monospace_size = other.style.monospace_size.or(self.style.monospace_size);
		self.style.item_spacing = other.style.item_spacing.or(self.style.item_spacing);
		self.style.button_padding = other.style.button_padding.or(self.style.button_padding);
		self.style.control_height = other.style.control_height.or(self.style.control_height);
		self.style.widget_radius = other.style.widget_radius.or(self.style.widget_radius);
		self.style.window_radius = other.style.window_radius.or(self.style.window_radius);
		self.style.menu_radius = other.style.menu_radius.or(self.style.menu_radius);
	}

	pub fn validate(&self) -> Result<(), Error> {
		let style = self.style;
		if [style.transparency, style.blur]
			.into_iter()
			.flatten()
			.any(|value| value > 100)
			|| ![
				style.body_size,
				style.button_size,
				style.small_size,
				style.monospace_size,
			]
			.into_iter()
			.flatten()
			.all(|size| (10..=28).contains(&size))
			|| style
				.heading_size
				.is_some_and(|size| !(12..=40).contains(&size))
			|| style
				.control_height
				.is_some_and(|size| !(24..=56).contains(&size))
			|| style
				.item_spacing
				.is_some_and(|sizes| sizes.into_iter().any(|size| size > 24))
			|| style
				.button_padding
				.is_some_and(|sizes| sizes.into_iter().any(|size| size > 24))
			|| [style.widget_radius, style.window_radius, style.menu_radius]
				.into_iter()
				.flatten()
				.any(|size| size > 24)
		{
			return Err(Error::Invalid);
		}
		const NAMES: &[&str] = &[
			"base",
			"sidebar",
			"chat",
			"raised",
			"hover",
			"selected",
			"border",
			"text_strong",
			"text",
			"muted",
			"link",
			"accent",
			"accent_text",
			"positive",
			"warning",
			"danger",
			"mention_bg",
			"mention_text",
		];
		for palette in [&self.light, &self.dark] {
			if palette.background.is_some_and(|background| {
				background.opacity > 100
					|| background.sections.is_some_and(|s| {
						[
							s.top_bar,
							s.server_list,
							s.channel_list,
							s.message_list,
							s.member_list,
							s.composer,
						]
						.into_iter()
						.any(|opacity| opacity > 100)
					})
			}) {
				return Err(Error::Invalid);
			}
			if palette.colors.len() > NAMES.len() {
				return Err(Error::Limit);
			}
			for (name, color) in &palette.colors {
				if !NAMES.contains(&name.as_str()) {
					return Err(Error::Invalid);
				}
				parse_color(color)?;
			}
			if let Some(stops) = &palette.backdrop {
				for color in stops {
					parse_color(color)?;
				}
			}
		}
		Ok(())
	}
}

impl Package {
	pub fn validate(&self) -> Result<(), Error> {
		self.manifest.validate()?;
		if self.background_image.len() > MAX_BACKGROUND_BYTES
			|| self.cover_image.len() > MAX_BACKGROUND_BYTES
		{
			return Err(Error::Limit);
		}
		match self.manifest.kind {
			ExtensionKind::Theme if self.wasm.is_empty() => {
				self.theme.as_ref().ok_or(Error::Invalid)?.validate()
			}
			ExtensionKind::Plugin
				if self.theme.is_none()
					&& self.background_image.is_empty()
					&& self.cover_image.is_empty() =>
			{
				if self.wasm.len() > MAX_MODULE_BYTES {
					return Err(Error::Limit);
				}
				runtime::validate_module(&self.wasm)
			}
			_ => Err(Error::Invalid),
		}
	}
}

pub fn parse_package(bytes: &[u8]) -> Result<Package, Error> {
	if bytes.len() > MAX_PACKAGE_BYTES {
		return Err(Error::Limit);
	}
	let mut package: Package = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
	// Creator theme IDs are case-insensitive; storage still uses validated lowercase IDs.
	// Keep plugin identities unchanged, including their action/capability references.
	if package.manifest.kind == ExtensionKind::Theme {
		package.manifest.id.make_ascii_lowercase();
	}
	package.validate()?;
	Ok(package)
}

pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog, Error> {
	if bytes.len() > MAX_CATALOG_BYTES {
		return Err(Error::Limit);
	}
	let catalog: Catalog = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
	if catalog.api_version != API_VERSION {
		return Err(Error::Version);
	}
	if catalog.entries.len() > 256 {
		return Err(Error::Limit);
	}
	let mut ids = BTreeSet::new();
	for entry in &catalog.entries {
		entry.manifest.validate()?;
		if !valid_https_url(&entry.manifest.source) {
			return Err(Error::Invalid);
		}
		if entry.description.len() > 1024
			|| entry.description.chars().count() > 256
			|| entry.description.chars().any(char::is_control)
		{
			return Err(Error::Invalid);
		}
		if let Some(preview) = &entry.preview {
			preview.validate()?;
		}
		if !ids.insert(&entry.manifest.id)
			|| !valid_https_url(&entry.release_url)
			|| entry.sha256.len() != 64
			|| !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::Invalid);
		}
		if entry.download_bytes == 0
			|| entry.download_bytes > MAX_PACKAGE_BYTES as u64
			|| !matches!(entry.source_commit.len(), 40 | 64)
			|| !entry.source_commit.bytes().all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::Invalid);
		}
	}
	Ok(catalog)
}
