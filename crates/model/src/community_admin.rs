//! Session-only community configuration. The participant onboarding model is independent.
use crate::Id;

pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_PROMPTS: usize = 50;
pub const MAX_OPTIONS: usize = 50;
pub const MAX_ASSIGNMENTS: usize = 100;
pub const MAX_DEFAULT_CHANNELS: usize = 500;
pub const MAX_FIELDS: usize = 5;
pub const MAX_ACTIONS: usize = 5;
pub const MAX_RESOURCES: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
	Onboarding,
	Screening,
	Guide,
}
impl Section {
	pub fn path(self) -> &'static str {
		match self {
			Self::Onboarding => "onboarding",
			Self::Screening => "member-verification",
			Self::Guide => "new-member-welcome",
		}
	}
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Emoji {
	pub id: Option<Id>,
	pub name: Option<String>,
	pub animated: bool,
}
impl Emoji {
	pub fn valid(&self) -> bool {
		self.id.is_none_or(|id| id.0 != 0) && self.name.as_ref().is_none_or(|s| text(s, 128, true))
	}
	fn bytes(&self) -> usize {
		self.name.as_ref().map_or(0, String::capacity)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionRow {
	/// New rows use millisecond timestamp IDs, matching the public client serializer.
	pub id: Id,
	pub title: String,
	pub description: Option<String>,
	pub emoji: Emoji,
	pub channels: Vec<Id>,
	pub roles: Vec<Id>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
	pub id: Id,
	pub kind: u8,
	pub title: String,
	pub single_select: bool,
	pub required: bool,
	pub in_onboarding: bool,
	pub options: Vec<OptionRow>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Onboarding {
	pub enabled: bool,
	pub mode: u8,
	pub below_requirements: bool,
	pub default_channels: Vec<Id>,
	pub prompts: Vec<Prompt>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
	/// Index in the fetched form; retained through reordering to preserve opaque field metadata.
	pub source: Option<usize>,
	pub kind: String,
	pub label: String,
	pub description: Option<String>,
	pub placeholder: Option<String>,
	pub required: bool,
	pub values: Vec<String>,
	pub choices: Vec<String>,
}
impl Field {
	pub fn supported(&self) -> bool {
		matches!(
			self.kind.as_str(),
			"TERMS" | "TEXT_INPUT" | "PARAGRAPH" | "MULTIPLE_CHOICE"
		)
	}
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screening {
	pub enabled: bool,
	pub manual_approval: bool,
	pub version: Option<String>,
	pub description: Option<String>,
	pub fields: Vec<Field>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideChannel {
	pub channel: Id,
	pub action: u8,
	pub title: String,
	pub description: String,
	pub emoji: Emoji,
	pub icon: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guide {
	pub enabled: bool,
	pub authors: Vec<Id>,
	pub message: String,
	pub actions: Vec<GuideChannel>,
	pub resources: Vec<GuideChannel>,
	pub header: Option<String>,
	pub header_available: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
	Onboarding(Onboarding),
	Screening(Screening),
	Guide(Guide),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
	pub guild: Id,
	pub data: Data,
	pub roles: Vec<Role>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
	pub id: Id,
	pub name: String,
	pub bits: u128,
	pub position: i32,
	pub managed: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PendingApplications {
	#[default]
	Approve,
	Reject,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
	Load(Section),
	Artwork {
		/// None selects the welcome sign; Some selects a first-step/resource icon.
		channel: Option<Id>,
		resource: bool,
		before: Option<String>,
		image: crate::Patch<String>,
	},
	Edit {
		before: Box<Data>,
		after: Box<Data>,
		pending: PendingApplications,
	},
}
fn text(value: &str, max: usize, multiline: bool) -> bool {
	value.len() <= max * 4
		&& value.chars().count() <= max
		&& !value
			.chars()
			.any(|c| c.is_control() && !(multiline && c == '\n'))
}
fn opt(value: &Option<String>, max: usize) -> bool {
	value.as_ref().is_none_or(|s| text(s, max, true))
}
fn ids(values: &[Id], max: usize) -> bool {
	values.len() <= max
		&& values
			.iter()
			.enumerate()
			.all(|(i, id)| id.0 != 0 && !values[..i].contains(id))
}
fn strings(values: &[String], max: usize, chars: usize) -> bool {
	values.len() <= max && values.iter().all(|s| text(s, chars, true))
}
impl Onboarding {
	pub fn valid(&self) -> bool {
		ids(&self.default_channels, MAX_DEFAULT_CHANNELS)
			&& self.prompts.len() <= MAX_PROMPTS
			&& self.prompts.iter().enumerate().all(|(i, p)| {
				p.id.0 != 0
					&& !self.prompts[..i].iter().any(|o| o.id == p.id)
					&& text(&p.title, 100, false)
					&& p.options.len() <= MAX_OPTIONS
					&& p.options.iter().enumerate().all(|(i, o)| {
						o.id.0 != 0
							&& !p.options[..i].iter().any(|v| v.id == o.id)
							&& text(&o.title, 50, false)
							&& opt(&o.description, 100)
							&& o.emoji.valid() && ids(&o.channels, MAX_ASSIGNMENTS)
							&& ids(&o.roles, MAX_ASSIGNMENTS)
					})
			})
	}
	pub fn editable(&self) -> bool {
		self.valid()
			&& self.prompts.iter().all(|p| {
				p.kind > 1
					|| (!p.title.trim().is_empty()
						&& !p.options.is_empty()
						&& p.options.iter().all(|o| !o.title.trim().is_empty()))
			})
	}
}
impl Screening {
	pub fn valid(&self) -> bool {
		opt(&self.version, 64)
			&& opt(&self.description, 300)
			&& self.fields.len() <= MAX_FIELDS
			&& self.fields.iter().all(|f| {
				text(&f.kind, 64, false)
					&& text(&f.label, 300, true)
					&& opt(&f.description, 300)
					&& opt(&f.placeholder, 300)
					&& strings(&f.values, 16, 300)
					&& strings(&f.choices, 8, 150)
			})
	}
	pub fn editable(&self) -> bool {
		self.valid()
			&& (!self.enabled || !self.fields.is_empty())
			&& self.fields.iter().all(|f| {
				!f.label.trim().is_empty()
					&& (!f.supported()
						|| match f.kind.as_str() {
							"TERMS" => {
								!f.values.is_empty()
									&& f.values.iter().all(|s| !s.trim().is_empty())
							}
							"MULTIPLE_CHOICE" => {
								self.manual_approval
									&& !f.choices.is_empty() && f
									.choices
									.iter()
									.all(|s| !s.trim().is_empty())
							}
							_ => self.manual_approval,
						})
			})
	}
}
impl Guide {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.authors.capacity() * size_of::<Id>()
			+ self.header.as_ref().map_or(0, String::capacity)
			+ self.message.capacity()
			+ (self.actions.capacity() + self.resources.capacity()) * size_of::<GuideChannel>()
			+ self
				.actions
				.iter()
				.chain(&self.resources)
				.map(|r| {
					r.title.capacity()
						+ r.description.capacity()
						+ r.emoji.bytes() + r.icon.as_ref().map_or(0, String::capacity)
				})
				.sum::<usize>()
	}

	pub fn valid(&self) -> bool {
		ids(&self.authors, 10)
			&& self
				.header
				.as_ref()
				.is_none_or(|hash| crate::valid_avatar_hash(hash))
			&& text(&self.message, 300, true)
			&& self.actions.len() <= MAX_ACTIONS
			&& self.resources.len() <= MAX_RESOURCES
			&& [&self.actions, &self.resources].into_iter().all(|rows| {
				rows.iter().enumerate().all(|(i, r)| {
					r.channel.0 != 0
						&& !rows[..i].iter().any(|o| o.channel == r.channel)
						&& text(&r.title, 60, false)
						&& text(&r.description, 200, true)
						&& r.emoji.valid() && r
						.icon
						.as_ref()
						.is_none_or(|s| crate::valid_avatar_hash(s))
				})
			})
	}
	pub fn editable(&self) -> bool {
		self.valid()
			&& (!self.enabled
				|| (self.actions.len() >= 3
					&& self.message.trim().chars().count() >= 7
					&& !self.authors.is_empty()))
			&& self
				.actions
				.iter()
				.chain(&self.resources)
				.all(|r| r.action <= 1 && !r.title.trim().is_empty())
	}
}
impl Data {
	/// Confirm only fields touched by this draft; new prompt IDs are assigned by Discord.
	pub fn confirms(before: &Self, after: &Self, actual: &Self) -> bool {
		match (before, after, actual) {
			(Self::Onboarding(old), Self::Onboarding(new), Self::Onboarding(actual)) => {
				(old.enabled == new.enabled || new.enabled == actual.enabled)
					&& (old.mode == new.mode || new.mode == actual.mode)
					&& (old.default_channels == new.default_channels
						|| new.default_channels == actual.default_channels)
					&& (old.prompts == new.prompts
						|| (new.prompts.len() == actual.prompts.len()
							&& new
								.prompts
								.iter()
								.zip(&actual.prompts)
								.all(|(new, actual)| {
									let existing = old.prompts.iter().find(|p| p.id == new.id);
									(existing.is_none() || new.id == actual.id)
										&& new.kind == actual.kind && new.title == actual.title
										&& new.single_select == actual.single_select
										&& new.required == actual.required
										&& new.in_onboarding == actual.in_onboarding
										&& new.options.len() == actual.options.len()
										&& new.options.iter().zip(&actual.options).all(
											|(new, actual)| {
												(existing.is_none_or(|p| {
													!p.options.iter().any(|o| o.id == new.id)
												}) || new.id == actual.id) && new.title
													== actual.title && new.description
													== actual.description && new.emoji == actual.emoji
													&& new.channels == actual.channels && new.roles
													== actual.roles
											},
										)
								})))
			}
			(Self::Screening(old), Self::Screening(new), Self::Screening(actual)) => {
				let fields = |fields: &[Field]| {
					fields
						.iter()
						.cloned()
						.map(|mut f| {
							f.source = None;
							f
						})
						.collect::<Vec<_>>()
				};
				(old.enabled == new.enabled || new.enabled == actual.enabled)
					&& (old.description == new.description || new.description == actual.description)
					&& (fields(&old.fields) == fields(&new.fields)
						|| fields(&new.fields) == fields(&actual.fields))
			}
			(Self::Guide(old), Self::Guide(new), Self::Guide(actual)) => {
				let rows = |rows: &[GuideChannel]| {
					rows.iter()
						.cloned()
						.map(|mut r| {
							r.icon = None;
							r
						})
						.collect::<Vec<_>>()
				};
				(old.enabled == new.enabled || new.enabled == actual.enabled)
					&& (old.authors == new.authors || new.authors == actual.authors)
					&& (old.message == new.message || new.message == actual.message)
					&& (rows(&old.actions) == rows(&new.actions)
						|| rows(&new.actions) == rows(&actual.actions))
					&& (rows(&old.resources) == rows(&new.resources)
						|| rows(&new.resources) == rows(&actual.resources))
			}
			_ => false,
		}
	}
	pub fn section(&self) -> Section {
		match self {
			Self::Onboarding(_) => Section::Onboarding,
			Self::Screening(_) => Section::Screening,
			Self::Guide(_) => Section::Guide,
		}
	}
	pub fn valid(&self) -> bool {
		self.bytes() <= MAX_BYTES
			&& match self {
				Self::Onboarding(v) => v.valid(),
				Self::Screening(v) => v.valid(),
				Self::Guide(v) => v.valid(),
			}
	}
	pub fn editable(&self) -> bool {
		self.valid()
			&& match self {
				Self::Onboarding(v) => v.editable(),
				Self::Screening(v) => v.editable(),
				Self::Guide(v) => v.editable(),
			}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Onboarding(v) => {
					v.default_channels.capacity() * size_of::<Id>()
						+ v.prompts.capacity() * size_of::<Prompt>()
						+ v.prompts
							.iter()
							.map(|p| {
								p.title.capacity()
									+ p.options.capacity() * size_of::<OptionRow>()
									+ p.options
										.iter()
										.map(|o| {
											o.title.capacity()
												+ o.description.as_ref().map_or(0, String::capacity)
												+ o.emoji.bytes() + (o.channels.capacity()
												+ o.roles.capacity()) * size_of::<Id>(
											)
										})
										.sum::<usize>()
							})
							.sum::<usize>()
				}
				Self::Screening(v) => {
					v.version.as_ref().map_or(0, String::capacity)
						+ v.description.as_ref().map_or(0, String::capacity)
						+ v.fields.capacity() * size_of::<Field>()
						+ v.fields
							.iter()
							.map(|f| {
								f.kind.capacity()
									+ f.label.capacity() + f
									.description
									.as_ref()
									.map_or(0, String::capacity)
									+ f.placeholder.as_ref().map_or(0, String::capacity)
									+ (f.values.capacity() + f.choices.capacity())
										* size_of::<String>() + f
									.values
									.iter()
									.chain(&f.choices)
									.map(String::capacity)
									.sum::<usize>()
							})
							.sum::<usize>()
				}
				Self::Guide(v) => v.bytes() - size_of::<Guide>(),
			}
	}
}
impl Snapshot {
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.data.valid()
			&& self.roles.len() <= crate::permissions::MAX_ROLES
			&& self.roles.iter().enumerate().all(|(i, r)| {
				r.id.0 != 0
					&& text(&r.name, 100, false)
					&& (0..=4096).contains(&r.position)
					&& !self.roles[..i].iter().any(|o| o.id == r.id)
			}) && self.bytes() <= MAX_BYTES
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.data.bytes()
			+ self.roles.capacity() * size_of::<Role>()
			+ self.roles.iter().map(|r| r.name.capacity()).sum::<usize>()
	}
}
impl Action {
	pub fn confirms(&self, snapshot: &Snapshot) -> bool {
		if self.section() != snapshot.data.section() {
			return false;
		}
		match self {
			Self::Load(_) => true,
			Self::Edit { before, after, .. } => Data::confirms(before, after, &snapshot.data),
			Self::Artwork {
				channel,
				resource,
				before,
				image,
			} => {
				let Data::Guide(guide) = &snapshot.data else {
					return false;
				};
				let hash = if let Some(channel) = channel {
					(if *resource {
						&guide.resources
					} else {
						&guide.actions
					})
					.iter()
					.find(|r| r.channel == *channel)
					.map(|r| &r.icon)
				} else {
					Some(&guide.header)
				};
				hash.is_some_and(|hash| match image {
					crate::Patch::Null => hash.is_none(),
					crate::Patch::Value(_) => hash.is_some() && hash != before,
					crate::Patch::Absent => false,
				})
			}
		}
	}
	pub fn write(&self) -> bool {
		!matches!(self, Self::Load(_))
	}
	pub fn section(&self) -> Section {
		match self {
			Self::Load(s) => *s,
			Self::Artwork { .. } => Section::Guide,
			Self::Edit { after, .. } => after.section(),
		}
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Load(_) => true,
			Self::Artwork {
				channel,
				before,
				image,
				..
			} => {
				channel.is_none_or(|id| id.0 != 0)
					&& before
						.as_ref()
						.is_none_or(|hash| crate::valid_avatar_hash(hash))
					&& match image {
						crate::Patch::Null => before.is_some(),
						crate::Patch::Value(uri) => {
							uri.capacity() <= crate::server_admin::MAX_IMAGE_URI
								&& uri.starts_with("data:image/png;base64,")
						}
						crate::Patch::Absent => false,
					}
			}
			Self::Edit { before, after, .. } => {
				before.section() == after.section()
					&& before.valid()
					&& after.editable()
					&& before != after
					&& safe_edit(before, after)
			}
		}
	}
	pub fn bytes(&self) -> usize {
		match self {
			Self::Load(_) => 0,
			Self::Artwork { image, before, .. } => {
				before.as_ref().map_or(0, String::capacity)
					+ match image {
						crate::Patch::Value(uri) => uri.capacity(),
						_ => 0,
					}
			}
			Self::Edit { before, after, .. } => before.bytes() + after.bytes(),
		}
	}
}
fn safe_edit(before: &Data, after: &Data) -> bool {
	match (before, after) {
		(Data::Onboarding(old), Data::Onboarding(new)) => {
			let mut ids = std::collections::BTreeSet::new();
			old.below_requirements == new.below_requirements
				&& (new.mode <= 1 || (new.mode == old.mode && new.enabled == old.enabled))
				&& new.prompts.iter().all(|p| {
					(p.kind <= 1 || old.prompts.iter().any(|o| o == p))
						&& ids.insert(p.id)
						&& p.options.iter().all(|o| ids.insert(o.id))
				})
		}
		(Data::Screening(old), Data::Screening(new)) => {
			let mut sources = std::collections::BTreeSet::new();
			old.version == new.version
				&& old.manual_approval == new.manual_approval
				&& new.fields.iter().all(|f| {
					if let Some(source) = f.source {
						sources.insert(source)
							&& old
								.fields
								.iter()
								.find(|o| o.source == Some(source))
								.is_some_and(|o| o.kind == f.kind && (f.supported() || o == f))
					} else {
						f.supported()
					}
				})
		}
		(Data::Guide(old), Data::Guide(new)) => {
			old.header == new.header
				&& old.header_available == new.header_available
				&& [
					(&old.actions, &new.actions),
					(&old.resources, &new.resources),
				]
				.into_iter()
				.all(|(old, new)| {
					new.iter().all(|r| {
						old.iter()
							.find(|o| o.channel == r.channel)
							.map_or(r.icon.is_none(), |o| o.icon == r.icon)
					})
				})
		}
		_ => false,
	}
}

impl Default for OptionRow {
	fn default() -> Self {
		Self {
			id: Id(0),
			title: String::new(),
			description: None,
			emoji: Emoji::default(),
			channels: vec![],
			roles: vec![],
		}
	}
}
impl Default for Prompt {
	fn default() -> Self {
		Self {
			id: Id(0),
			kind: 0,
			title: String::new(),
			single_select: false,
			required: false,
			in_onboarding: true,
			options: vec![],
		}
	}
}
impl Default for GuideChannel {
	fn default() -> Self {
		Self {
			channel: Id(0),
			action: 0,
			title: String::new(),
			description: String::new(),
			emoji: Emoji::default(),
			icon: None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn screening_preserves_unknown_fields_but_does_not_enable_an_empty_gate() {
		let mut s = Screening {
			enabled: true,
			..Default::default()
		};
		assert!(!s.editable());
		s.fields.push(Field {
			kind: "FUTURE".into(),
			label: "Unknown".into(),
			..Default::default()
		});
		assert!(s.editable());
		s.fields[0].values = vec!["x".repeat(301)];
		assert!(!s.valid());
	}
	#[test]
	fn guide_needs_three_actions_and_bounded_authors() {
		let mut g = Guide {
			enabled: true,
			..Default::default()
		};
		assert!(!g.editable());
		g.message = "Welcome".into();
		g.authors = vec![Id(1)];
		g.actions = (1..=3)
			.map(|id| GuideChannel {
				channel: Id(id),
				title: "Read this".into(),
				..Default::default()
			})
			.collect();
		assert!(g.editable());
		g.authors.push(Id(1));
		assert!(!g.valid());
	}
}
