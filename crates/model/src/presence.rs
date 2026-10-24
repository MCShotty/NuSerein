//! Presence validation and display metadata.
use super::{Id, MAX_ACTIVITY_TIMESTAMP, MAX_RICH_ACTIVITIES};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresenceStatus {
	#[default]
	Online,
	Idle,
	DoNotDisturb,
	Invisible,
}

impl PresenceStatus {
	pub const ALL: [Self; 4] = [
		Self::Online,
		Self::Idle,
		Self::DoNotDisturb,
		Self::Invisible,
	];
	pub fn wire(self) -> &'static str {
		match self {
			Self::Online => "online",
			Self::Idle => "idle",
			Self::DoNotDisturb => "dnd",
			Self::Invisible => "invisible",
		}
	}
	pub fn label(self) -> &'static str {
		match self {
			Self::Online => "Online",
			Self::Idle => "Idle",
			Self::DoNotDisturb => "Do Not Disturb",
			Self::Invisible => "Invisible",
		}
	}
	pub fn parse(wire: &str) -> Option<Self> {
		match wire {
			"online" => Some(Self::Online),
			"idle" => Some(Self::Idle),
			"dnd" => Some(Self::DoNotDisturb),
			"invisible" => Some(Self::Invisible),
			_ => None,
		}
	}
}

/// This account's chosen status. Discord settings are authoritative. A local row is only the fallback when that read fails.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnPresence {
	pub status: PresenceStatus,
	pub custom_status: String,
	/// Custom-status clear deadline, unix milliseconds. The gateway presence opcode does not carry it.
	pub expires_at_ms: Option<u64>,
}

impl OwnPresence {
	pub fn valid(&self) -> bool {
		self.custom_status.is_empty() || valid_presence_text(&self.custom_status)
	}
}

/// A service image reference, never permission to fetch an arbitrary external URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityImage {
	Asset { application: Id, asset: Id },
	Proxy(String),
	Spotify(String),
	Application(Id),
}

impl ActivityImage {
	pub fn valid(&self) -> bool {
		match self {
			Self::Asset { application, asset } => application.0 != 0 && asset.0 != 0,
			Self::Application(id) => id.0 != 0,
			Self::Spotify(id) => id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()),
			Self::Proxy(path) => {
				!path.is_empty()
					&& path.len() <= 1024
					&& !path.starts_with('/')
					&& !path
						.chars()
						.any(|c| c.is_control() || c.is_whitespace() || c == '\\')
					&& !path.split('/').any(|part| matches!(part, "." | ".."))
					&& !path.as_bytes().windows(3).any(|part| {
						part.eq_ignore_ascii_case(b"%2e")
							|| part.eq_ignore_ascii_case(b"%2f")
							|| part.eq_ignore_ascii_case(b"%5c")
					})
			}
		}
	}
	pub fn heap_bytes(&self) -> usize {
		match self {
			Self::Proxy(path) | Self::Spotify(path) => path.capacity(),
			_ => 0,
		}
	}
	pub fn key(&self) -> String {
		match self {
			Self::Asset { application, asset } => format!("activity-{application}-{asset}"),
			Self::Proxy(path) => format!("embed:https://media.discordapp.net/{path}"),
			Self::Application(id) => format!("app-icon-{id}"),
			Self::Spotify(id) => format!("spotify-{id}"),
		}
	}
}

/// Bounded activity metadata. Secrets and actions are never retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichActivity {
	pub kind: u8,
	pub name: String,
	pub details: Option<String>,
	pub state: Option<String>,
	pub image: Option<ActivityImage>,
	pub small_image: Option<ActivityImage>,
	/// Unix milliseconds, as supplied by the activity producer.
	pub started_at: Option<u64>,
	/// Activity end in Unix milliseconds; may be present without a start for a countdown.
	pub ends_at: Option<u64>,
}

impl RichActivity {
	pub fn valid(&self) -> bool {
		matches!(self.kind, 0..=3 | 5)
			&& valid_presence_text(&self.name)
			&& self.details.as_deref().is_none_or(valid_presence_text)
			&& self.state.as_deref().is_none_or(valid_presence_text)
			&& self.image.as_ref().is_none_or(ActivityImage::valid)
			&& self.small_image.as_ref().is_none_or(ActivityImage::valid)
			&& self
				.started_at
				.is_none_or(|at| at <= MAX_ACTIVITY_TIMESTAMP)
			&& self.ends_at.is_none_or(|end| {
				end <= MAX_ACTIVITY_TIMESTAMP && self.started_at.is_none_or(|start| end > start)
			})
	}
	pub fn heap_bytes(&self) -> usize {
		self.name.capacity()
			+ self.details.as_ref().map_or(0, String::capacity)
			+ self.state.as_ref().map_or(0, String::capacity)
			+ self.image.as_ref().map_or(0, ActivityImage::heap_bytes)
			+ self
				.small_image
				.as_ref()
				.map_or(0, ActivityImage::heap_bytes)
	}
	pub fn summary(&self) -> String {
		let verb = match self.kind {
			0 => "Playing",
			1 => "Streaming",
			2 => "Listening to",
			3 => "Watching",
			5 => "Competing in",
			_ => return self.name.clone(),
		};
		format!("{verb} {}", self.name)
	}
}

pub(super) fn valid_presence_text(text: &str) -> bool {
	!text.is_empty()
		&& text.len() <= 512
		&& text.chars().count() <= 128
		&& text.trim() == text
		&& !text.chars().any(char::is_control)
}

/// Fixed-size client session flags for an already-loaded user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientPlatforms {
	pub desktop: bool,
	pub mobile: bool,
	pub web: bool,
	pub vr: bool,
}

impl ClientPlatforms {
	pub fn any(self) -> bool {
		self.desktop || self.mobile || self.web || self.vr
	}
}

/// Complete, bounded presence values for an already-loaded user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberPresence {
	pub user: Id,
	pub status: Option<String>,
	pub custom_status: Option<String>,
	pub activities: Vec<RichActivity>,
	pub clients: ClientPlatforms,
}

impl MemberPresence {
	pub fn valid(&self) -> bool {
		self.user.0 != 0
			&& self
				.status
				.as_deref()
				.is_none_or(|status| matches!(status, "online" | "idle" | "dnd" | "offline"))
			&& self
				.custom_status
				.as_deref()
				.is_none_or(valid_presence_text)
			&& self.activities.len() <= MAX_RICH_ACTIVITIES
			&& self.activities.iter().all(RichActivity::valid)
	}
	pub fn heap_bytes(&self) -> usize {
		self.status.as_ref().map_or(0, String::capacity)
			+ self.custom_status.as_ref().map_or(0, String::capacity)
			+ self.activities.capacity() * size_of::<RichActivity>()
			+ self
				.activities
				.iter()
				.map(RichActivity::heap_bytes)
				.sum::<usize>()
	}
}
