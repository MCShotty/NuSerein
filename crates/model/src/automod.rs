//! Typed AutoMod rules. Server evaluation remains authoritative; no local regex is executed.
use crate::Id;

pub const MAX_RULES: usize = 32;
pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Metadata {
	pub keywords: Vec<String>,
	pub regex: Vec<String>,
	pub presets: Vec<u8>,
	pub allow: Vec<String>,
	pub mentions: u8,
	pub raid_protection: bool,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Effect {
	pub kind: u8,
	pub channel: Option<Id>,
	pub duration: u32,
	pub message: String,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
	pub name: String,
	pub event: u8,
	pub trigger: u8,
	pub metadata: Metadata,
	pub effects: Vec<Effect>,
	pub enabled: bool,
	pub exempt_roles: Vec<Id>,
	pub exempt_channels: Vec<Id>,
}
impl Default for Settings {
	fn default() -> Self {
		Self {
			name: String::new(),
			event: 1,
			trigger: 1,
			metadata: Metadata::default(),
			effects: vec![Effect {
				kind: 1,
				..Effect::default()
			}],
			enabled: false,
			exempt_roles: Vec::new(),
			exempt_channels: Vec::new(),
		}
	}
}
#[derive(Clone, PartialEq, Eq)]
pub struct Rule {
	pub id: Id,
	pub guild: Id,
	pub creator: Id,
	pub settings: Settings,
}
#[derive(Clone)]
pub struct Snapshot {
	pub guild: Id,
	pub rules: Vec<Rule>,
}
#[derive(Clone)]
pub enum Action {
	Load,
	Create {
		settings: Box<Settings>,
		reason: String,
	},
	Edit {
		id: Id,
		before: Box<Settings>,
		after: Box<Settings>,
		reason: String,
	},
	Delete {
		id: Id,
		reason: String,
	},
}
pub fn valid_reason(reason: &str) -> bool {
	reason.chars().count() <= 512 && reason.len() <= 2048 && !reason.chars().any(char::is_control)
}
fn strings(values: &[String], count: usize, length: usize) -> bool {
	values.len() <= count
		&& values.iter().all(|text| {
			!text.is_empty()
				&& text.chars().count() <= length
				&& text.len() <= length * 4
				&& !text.chars().any(char::is_control)
		})
}
fn ids(values: &[Id], count: usize) -> bool {
	values.len() <= count
		&& values
			.iter()
			.enumerate()
			.all(|(index, id)| id.0 != 0 && !values[..index].contains(id))
}
fn ids_confirm(current: &[Id], before: &[Id], after: &[Id]) -> bool {
	before
		.iter()
		.filter(|id| !after.contains(id))
		.all(|id| !current.contains(id))
		&& after
			.iter()
			.filter(|id| !before.contains(id))
			.all(|id| current.contains(id))
}
impl Settings {
	pub fn reapply(&self, before: &Self, after: &Self) -> Option<Self> {
		if self.trigger != before.trigger
			|| self.event != before.event
			|| before.trigger != after.trigger
			|| before.event != after.event
		{
			return None;
		}
		let mut value = self.clone();
		macro_rules! field {
			($field:ident) => {
				if before.$field != after.$field {
					value.$field.clone_from(&after.$field);
				}
			};
		}
		field!(name);
		field!(enabled);
		macro_rules! metadata {
			($field:ident) => {
				if before.metadata.$field != after.metadata.$field {
					value.metadata.$field.clone_from(&after.metadata.$field);
				}
			};
		}
		metadata!(keywords);
		metadata!(regex);
		metadata!(presets);
		metadata!(allow);
		metadata!(mentions);
		metadata!(raid_protection);
		for previous in &before.effects {
			if !after
				.effects
				.iter()
				.any(|effect| effect.kind == previous.kind)
			{
				value.effects.retain(|effect| effect.kind != previous.kind);
			}
		}
		for next in &after.effects {
			let previous = before
				.effects
				.iter()
				.find(|effect| effect.kind == next.kind);
			if previous == Some(next) {
				continue;
			}
			if let (Some(previous), Some(current)) = (
				previous,
				value
					.effects
					.iter_mut()
					.find(|effect| effect.kind == next.kind),
			) {
				if previous.channel != next.channel {
					current.channel = next.channel;
				}
				if previous.duration != next.duration {
					current.duration = next.duration;
				}
				if previous.message != next.message {
					current.message.clone_from(&next.message);
				}
			} else {
				value.effects.retain(|effect| effect.kind != next.kind);
				value.effects.push(next.clone());
			}
		}
		fn ids(current: &mut Vec<Id>, before: &[Id], after: &[Id]) {
			current.retain(|id| !before.contains(id) || after.contains(id));
			for id in after.iter().filter(|id| !before.contains(id)) {
				if !current.contains(id) {
					current.push(*id);
				}
			}
		}
		ids(
			&mut value.exempt_roles,
			&before.exempt_roles,
			&after.exempt_roles,
		);
		ids(
			&mut value.exempt_channels,
			&before.exempt_channels,
			&after.exempt_channels,
		);
		value.valid().then_some(value)
	}
	/// Confirm only fields the user changed; unrelated concurrent edits must survive.
	pub fn confirms(&self, before: &Self, after: &Self) -> bool {
		macro_rules! kept {
			($field:ident) => {
				before.$field == after.$field || self.$field == after.$field
			};
		}
		self.trigger == after.trigger
			&& self.event == after.event
			&& kept!(name)
			&& kept!(enabled)
			&& {
				let current = &self.metadata;
				let old = &before.metadata;
				let new = &after.metadata;
				macro_rules! changed {
					($field:ident) => {
						old.$field == new.$field || current.$field == new.$field
					};
				}
				changed!(keywords)
					&& changed!(regex)
					&& changed!(presets)
					&& changed!(allow)
					&& changed!(mentions)
					&& changed!(raid_protection)
			} && self.effects_confirm(before, after)
			&& ids_confirm(
				&self.exempt_roles,
				&before.exempt_roles,
				&after.exempt_roles,
			) && ids_confirm(
			&self.exempt_channels,
			&before.exempt_channels,
			&after.exempt_channels,
		)
	}
	fn effects_confirm(&self, before: &Self, after: &Self) -> bool {
		before.effects.iter().chain(&after.effects).all(|effect| {
			let old = before.effects.iter().find(|row| row.kind == effect.kind);
			let new = after.effects.iter().find(|row| row.kind == effect.kind);
			let current = self.effects.iter().find(|row| row.kind == effect.kind);
			match (old, new, current) {
				(old, new, _) if old == new => true,
				(Some(_), None, None) => true,
				(None, Some(new), Some(current)) => new == current,
				(Some(old), Some(new), Some(current)) => {
					(old.channel == new.channel || current.channel == new.channel)
						&& (old.duration == new.duration || current.duration == new.duration)
						&& (old.message == new.message || current.message == new.message)
				}
				_ => false,
			}
		})
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.name.capacity()
			+ self.metadata.keywords.capacity() * size_of::<String>()
			+ self.metadata.regex.capacity() * size_of::<String>()
			+ self.metadata.allow.capacity() * size_of::<String>()
			+ self.metadata.presets.capacity()
			+ self
				.metadata
				.keywords
				.iter()
				.chain(&self.metadata.regex)
				.chain(&self.metadata.allow)
				.map(String::capacity)
				.sum::<usize>()
			+ self.effects.capacity() * size_of::<Effect>()
			+ self
				.effects
				.iter()
				.map(|effect| effect.message.capacity())
				.sum::<usize>()
			+ (self.exempt_roles.capacity() + self.exempt_channels.capacity()) * size_of::<Id>()
	}
	pub fn valid(&self) -> bool {
		let m = &self.metadata;
		(1..=100).contains(&self.name.chars().count())
			&& !self.name.chars().any(char::is_control)
			&& matches!(self.trigger, 1 | 3..=6)
			&& self.event == if self.trigger == 6 { 2 } else { 1 }
			&& self.bytes() <= MAX_BYTES / 2
			&& strings(&m.keywords, 1000, 60)
			&& strings(&m.regex, 10, 260)
			&& strings(&m.allow, if self.trigger == 4 { 1000 } else { 100 }, 60)
			&& m.presets.len() <= 3
			&& m.presets
				.iter()
				.enumerate()
				.all(|(index, kind)| (1..=3).contains(kind) && !m.presets[..index].contains(kind))
			&& m.mentions <= 50
			&& (matches!(self.trigger, 1 | 6) || m.keywords.is_empty() && m.regex.is_empty())
			&& (self.trigger == 4 || m.presets.is_empty())
			&& (matches!(self.trigger, 1 | 4 | 6) || m.allow.is_empty())
			&& (self.trigger == 5 || m.mentions == 0 && !m.raid_protection)
			&& (1..=4).contains(&self.effects.len())
			&& self.effects.iter().enumerate().all(|(index, effect)| {
				!self.effects[..index]
					.iter()
					.any(|row| row.kind == effect.kind)
					&& effect.message.chars().count() <= 150
					&& !effect.message.chars().any(char::is_control)
					&& match effect.kind {
						1 => self.trigger != 6 && effect.channel.is_none() && effect.duration == 0,
						2 => {
							effect.channel.is_some_and(|id| id.0 != 0)
								&& effect.message.is_empty()
								&& effect.duration == 0
						}
						3 => {
							matches!(self.trigger, 1 | 5)
								&& (1..=2_419_200).contains(&effect.duration)
								&& effect.channel.is_none()
								&& effect.message.is_empty()
						}
						4 => {
							self.trigger == 6
								&& effect.channel.is_none()
								&& effect.duration == 0 && effect.message.is_empty()
						}
						_ => false,
					}
			}) && ids(&self.exempt_roles, 20)
			&& ids(&self.exempt_channels, 50)
	}
}
impl Rule {
	pub fn valid(&self) -> bool {
		self.id.0 != 0 && self.guild.0 != 0 && self.creator.0 != 0 && self.settings.valid()
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.settings.bytes()
	}
}
impl Snapshot {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.rules.capacity() * size_of::<Rule>()
			+ self.rules.iter().map(Rule::bytes).sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.rules.len() <= MAX_RULES
			&& self.bytes() <= MAX_BYTES
			&& self.rules.iter().enumerate().all(|(i, rule)| {
				rule.guild == self.guild
					&& rule.valid() && !self.rules[..i].iter().any(|row| row.id == rule.id)
			})
	}
}
impl Action {
	pub fn write(&self) -> bool {
		!matches!(self, Self::Load)
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Load => true,
			Self::Create { settings, reason } => {
				settings.valid() && valid_reason(reason) && reason.capacity() <= 4096
			}
			Self::Edit {
				id,
				before,
				after,
				reason,
			} => {
				id.0 != 0
					&& before.valid()
					&& after.valid()
					&& before.trigger == after.trigger
					&& before.event == after.event
					&& valid_reason(reason)
					&& reason.capacity() <= 4096
			}
			Self::Delete { id, reason } => {
				id.0 != 0 && valid_reason(reason) && reason.capacity() <= 4096
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reapplying_drafts_preserves_unrelated_concurrent_edits() {
		let before = Settings {
			name: "Synthetic".into(),
			exempt_roles: vec![Id(10)],
			..Default::default()
		};
		let mut after = before.clone();
		after.metadata.keywords = vec!["reviewed keyword".into()];
		after.exempt_roles.clear();
		let mut current = before.clone();
		current.name = "Renamed elsewhere".into();
		current.metadata.allow = vec!["concurrent allowance".into()];
		current.exempt_roles.push(Id(20));
		let result = current.reapply(&before, &after).unwrap();
		assert_eq!(result.name, "Renamed elsewhere");
		assert_eq!(result.metadata.allow, ["concurrent allowance"]);
		assert_eq!(result.exempt_roles, [Id(20)]);
		assert!(result.confirms(&before, &after));
		assert!(!before.confirms(&before, &after));
	}
	#[test]
	fn rules_validate_trigger_effect_combinations_and_budgets() {
		let mut rule = Settings {
			name: "Synthetic".into(),
			..Default::default()
		};
		assert!(rule.valid());
		rule.effects.push(Effect {
			kind: 3,
			duration: 60,
			..Default::default()
		});
		assert!(rule.valid());
		rule.trigger = 3;
		assert!(!rule.valid());
		rule.effects.pop();
		assert!(rule.valid());
		rule.trigger = 6;
		rule.event = 2;
		rule.effects[0].kind = 4;
		assert!(rule.valid());
		rule.metadata.regex = vec!["x".repeat(261)];
		assert!(!rule.valid());
		rule.metadata.regex.clear();
		rule.exempt_roles = vec![Id(1); 2];
		assert!(!rule.valid());
		assert!(!valid_reason("line\ninjection"));
	}
}
