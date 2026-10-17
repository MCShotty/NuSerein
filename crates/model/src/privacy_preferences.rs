//! Service-managed privacy/content preferences; defaults remain explicitly unspecified.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileVisibility {
	#[default]
	Default,
	Friends,
	FriendsAndSmallServers,
	FriendsAndAllServers,
	Unknown(u32),
}
impl ProfileVisibility {
	pub fn from_wire(value: u32) -> Self {
		match value {
			0 => Self::Default,
			1 => Self::Friends,
			2 => Self::FriendsAndSmallServers,
			3 => Self::FriendsAndAllServers,
			value => Self::Unknown(value),
		}
	}
	pub fn wire(self) -> u32 {
		match self {
			Self::Default => 0,
			Self::Friends => 1,
			Self::FriendsAndSmallServers => 2,
			Self::FriendsAndAllServers => 3,
			Self::Unknown(value) => value,
		}
	}
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Redaction {
	#[default]
	Default,
	Show,
	Blur,
	Block,
	Unknown(u32),
}
impl Redaction {
	pub fn from_wire(value: u32) -> Self {
		match value {
			0 => Self::Default,
			1 => Self::Show,
			2 => Self::Blur,
			3 => Self::Block,
			value => Self::Unknown(value),
		}
	}
	pub fn wire(self) -> u32 {
		match self {
			Self::Default => 0,
			Self::Show => 1,
			Self::Blur => 2,
			Self::Block => 3,
			Self::Unknown(value) => value,
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
	Sensitive,
	Gore,
	SelfHarm,
}
impl ContentKind {
	pub fn field(self) -> u64 {
		match self {
			Self::Sensitive => 29,
			Self::Gore => 32,
			Self::SelfHarm => 35,
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
	Servers,
	Friends,
	Others,
}
impl Audience {
	pub fn index(self) -> usize {
		match self {
			Self::Servers => 0,
			Self::Friends => 1,
			Self::Others => 2,
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
	Profanity,
	SexualContent,
	Slurs,
}
impl Keyword {
	pub fn index(self) -> usize {
		match self {
			Self::Profanity => 0,
			Self::SexualContent => 1,
			Self::Slurs => 2,
		}
	}
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
	pub profile_visibility: ProfileVisibility,
	/// Servers, friend DMs, non-friend DMs, in that order. Unknown/default values
	/// must not be represented as a fabricated service decision.
	pub sensitive: [Redaction; 3],
	pub gore: [Redaction; 3],
	pub self_harm: [Redaction; 3],
	pub keywords: [Option<bool>; 3],
	pub conversation_warnings: Option<bool>,
	pub show_local_time: Option<bool>,
	pub detect_platform_accounts: Option<bool>,
	pub non_spam_training: Option<bool>,
}
impl Settings {
	pub fn content(&self, kind: ContentKind) -> &[Redaction; 3] {
		match kind {
			ContentKind::Sensitive => &self.sensitive,
			ContentKind::Gore => &self.gore,
			ContentKind::SelfHarm => &self.self_harm,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
	ProfileVisibility(ProfileVisibility),
	Content {
		kind: ContentKind,
		audience: Audience,
		level: Redaction,
	},
	Keyword {
		keyword: Keyword,
		enabled: bool,
	},
	ConversationWarnings(bool),
	ShowLocalTime(bool),
	DetectPlatformAccounts(bool),
	NonSpamTraining(bool),
}
impl Change {
	pub fn valid(self) -> bool {
		match self {
			Self::ProfileVisibility(value) => matches!(
				value,
				ProfileVisibility::Friends
					| ProfileVisibility::FriendsAndSmallServers
					| ProfileVisibility::FriendsAndAllServers
			),
			Self::Content { level, .. } => {
				matches!(level, Redaction::Show | Redaction::Blur | Redaction::Block)
			}
			_ => true,
		}
	}
	pub fn apply(self, settings: &mut Settings) {
		match self {
			Self::ProfileVisibility(value) => settings.profile_visibility = value,
			Self::Content {
				kind,
				audience,
				level,
			} => {
				let values = match kind {
					ContentKind::Sensitive => &mut settings.sensitive,
					ContentKind::Gore => &mut settings.gore,
					ContentKind::SelfHarm => &mut settings.self_harm,
				};
				values[audience.index()] = level;
			}
			Self::Keyword { keyword, enabled } => {
				settings.keywords[keyword.index()] = Some(enabled)
			}
			Self::ConversationWarnings(value) => settings.conversation_warnings = Some(value),
			Self::ShowLocalTime(value) => settings.show_local_time = Some(value),
			Self::DetectPlatformAccounts(value) => settings.detect_platform_accounts = Some(value),
			Self::NonSpamTraining(value) => settings.non_spam_training = Some(value),
		}
	}
	pub fn field(self) -> (bool, u64) {
		match self {
			Self::Content { kind, .. } => (true, kind.field()),
			Self::Keyword { .. } => (true, 30),
			Self::ProfileVisibility(_) => (false, 32),
			Self::ConversationWarnings(_) => (false, 23),
			Self::ShowLocalTime(_) => (false, 31),
			Self::DetectPlatformAccounts(_) => (false, 8),
			Self::NonSpamTraining(_) => (false, 19),
		}
	}
}
