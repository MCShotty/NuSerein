//! Session-only scheduled events and Stage instances. Official resource schemas;
//! normal-account RSVP routes are unofficial. No media authorization is inferred here.
use crate::Id;
use serde::{Deserialize, Serialize};

pub const MAX_EVENTS: usize = 100;
pub const MAX_BYTES: usize = 768 * 1024;
pub const MAX_WIRE: usize = 1024 * 1024;

fn text(value: &str, min: usize, max: usize) -> bool {
	value.len() <= max * 4 && (min..=max).contains(&value.chars().count()) && !value.contains('\0')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recurrence {
	pub start: i64,
	pub end: Option<i64>,
	pub frequency: u8,
	pub interval: u8,
	pub by_weekday: Vec<u8>,
	pub by_n_weekday: Vec<NWeekday>,
	pub by_month: Vec<u8>,
	pub by_month_day: Vec<u8>,
	pub by_year_day: Vec<u16>,
	pub count: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct NWeekday {
	pub n: u8,
	pub day: u8,
}
impl Recurrence {
	pub fn valid(&self) -> bool {
		(1..=253_402_300_799).contains(&self.start)
			&& self
				.end
				.is_none_or(|end| end > self.start && end <= 253_402_300_799)
			&& self.frequency <= 3
			&& (1..=2).contains(&self.interval)
			&& self.by_weekday.len() <= 7
			&& self.by_weekday.iter().all(|d| *d <= 6)
			&& self.by_n_weekday.len() <= 1
			&& self
				.by_n_weekday
				.iter()
				.all(|d| (1..=5).contains(&d.n) && d.day <= 6)
			&& self.by_month.len() <= 1
			&& self.by_month.iter().all(|m| (1..=12).contains(m))
			&& self.by_month_day.len() <= 1
			&& self.by_month_day.iter().all(|d| (1..=31).contains(d))
			&& self.by_year_day.len() <= 366
			&& self.by_year_day.iter().all(|d| (1..=366).contains(d))
	}
	/// Current official client-settable subset. Read-only server fields are never echoed.
	pub fn writable(&self) -> bool {
		if !self.valid()
			|| self.end.is_some()
			|| self.count.is_some()
			|| !self.by_year_day.is_empty()
		{
			return false;
		}
		match self.frequency {
			0 => {
				self.interval == 1
					&& self.by_month.len() == 1
					&& self.by_month_day.len() == 1
					&& self.by_weekday.is_empty()
					&& self.by_n_weekday.is_empty()
			}
			1 => {
				self.interval == 1
					&& self.by_n_weekday.len() == 1
					&& self.by_weekday.is_empty()
					&& self.by_month.is_empty()
					&& self.by_month_day.is_empty()
			}
			2 => {
				self.by_weekday.len() == 1
					&& self.by_n_weekday.is_empty()
					&& self.by_month.is_empty()
					&& self.by_month_day.is_empty()
			}
			3 => {
				self.interval == 1
					&& self.by_n_weekday.is_empty()
					&& self.by_month.is_empty()
					&& self.by_month_day.is_empty()
					&& matches!(
						self.by_weekday.as_slice(),
						[] | [0, 1, 2, 3, 4]
							| [1, 2, 3, 4, 5] | [6, 0, 1, 2, 3]
							| [4, 5] | [5, 6] | [6, 0]
					)
			}
			_ => false,
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.by_weekday.capacity()
			+ self.by_n_weekday.capacity() * size_of::<NWeekday>()
			+ self.by_month.capacity()
			+ self.by_month_day.capacity()
			+ self.by_year_day.capacity() * 2
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventDraft {
	pub name: String,
	pub description: String,
	pub channel: Option<Id>,
	pub entity_type: u8,
	pub location: String,
	pub start: i64,
	pub end: Option<i64>,
	pub recurrence: Option<Recurrence>,
}
impl EventDraft {
	pub fn valid(&self) -> bool {
		text(&self.name, 1, 100)
			&& text(&self.description, 0, 1000)
			&& (1..=253_402_300_799).contains(&self.start)
			&& self
				.end
				.is_none_or(|end| end > self.start && end <= 253_402_300_799)
			&& match self.entity_type {
				1 | 2 => self.channel.is_some_and(|c| c.0 != 0) && self.location.is_empty(),
				3 => self.channel.is_none() && self.end.is_some() && text(&self.location, 1, 100),
				_ => false,
			} && self
			.recurrence
			.as_ref()
			.is_none_or(|r| r.valid() && r.start > 0)
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.name.capacity()
			+ self.description.capacity()
			+ self.location.capacity()
			+ self.recurrence.as_ref().map_or(0, Recurrence::bytes)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledEvent {
	pub id: Id,
	pub guild: Id,
	pub creator: Option<Id>,
	pub status: u8,
	pub draft: EventDraft,
	pub user_count: Option<u32>,
	pub exceptions: Vec<EventException>,
}
impl ScheduledEvent {
	pub fn valid(&self) -> bool {
		self.id.0 != 0
			&& self.guild.0 != 0
			&& (1..=4).contains(&self.status)
			&& self.draft.valid()
			&& self.exceptions.len() <= MAX_EXCEPTIONS
			&& self.exceptions.iter().enumerate().all(|(i, e)| {
				e.valid() && e.event == self.id && self.exceptions[..i].iter().all(|p| p.id != e.id)
			})
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.draft.bytes()
			+ self.exceptions.capacity() * size_of::<EventException>()
	}
	pub fn link(&self) -> String {
		format!("https://discord.com/events/{}/{}", self.guild, self.id)
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
	pub event: Id,
	pub occurrence: Option<Id>,
	pub interested: bool,
	pub user: Id,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPage {
	pub guild: Id,
	pub events: Vec<ScheduledEvent>,
	/// None means the service did not disclose normal-account subscription data.
	pub subscriptions: Option<Vec<Subscription>>,
	/// A confirmed single RSVP is retained even when the complete account list is unavailable.
	pub subscriptions_complete: bool,
}
impl EventPage {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.events.capacity() * size_of::<ScheduledEvent>()
			+ self.events.iter().map(ScheduledEvent::bytes).sum::<usize>()
			+ self
				.subscriptions
				.as_ref()
				.map_or(0, |s| s.capacity() * size_of::<Subscription>())
	}
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.events.len() <= MAX_EVENTS
			&& (!self.subscriptions_complete || self.subscriptions.is_some())
			&& self.bytes() <= MAX_BYTES
			&& self.events.iter().enumerate().all(|(i, e)| {
				e.guild == self.guild && e.valid() && self.events[..i].iter().all(|p| p.id != e.id)
			}) && self.subscriptions.as_ref().is_none_or(|s| {
			s.len() <= MAX_EVENTS * 4
				&& s.iter().enumerate().all(|(i, row)| {
					row.event.0 != 0
						&& row.user.0 != 0 && row.occurrence.is_none_or(|id| id.0 != 0)
						&& s[..i]
							.iter()
							.all(|p| p.event != row.event || p.occurrence != row.occurrence)
				})
		})
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageInstance {
	pub id: Id,
	pub guild: Id,
	pub channel: Id,
	pub topic: String,
	pub event: Option<Id>,
}
impl StageInstance {
	pub fn valid(&self) -> bool {
		self.id.0 != 0 && self.guild.0 != 0 && self.channel.0 != 0 && text(&self.topic, 1, 120)
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.topic.capacity()
	}
}
/// Separate from server mute. An invitation never authorizes capture until accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StageParticipation {
	#[default]
	Unknown,
	Audience {
		requested: bool,
	},
	Invited,
	Speaker,
}
impl StageParticipation {
	pub fn from_wire(suppress: bool, requested: bool) -> Self {
		match (suppress, requested) {
			(true, requested) => Self::Audience { requested },
			(false, true) => Self::Invited,
			(false, false) => Self::Speaker,
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageVoiceChange {
	RequestToSpeak(bool),
	AcceptInvitation,
	Audience,
	BecomeSpeaker,
	Invite(Id),
	MoveToAudience(Id),
}
#[derive(Clone, Debug, PartialEq, Eq)]
// One pending action, already boxed by Command, is bounded to 64 KiB by valid()/bytes().
#[allow(clippy::large_enum_variant)]
pub enum Action {
	ListEvents,
	EditOccurrence {
		before: ScheduledEvent,
		original: i64,
		canceled: bool,
		start: Option<i64>,
		end: Option<i64>,
	},
	RestoreOccurrence {
		before: ScheduledEvent,
		exception: Id,
	},
	RsvpOccurrence {
		event: Id,
		occurrence: Id,
		interested: bool,
	},
	ResetOccurrenceRsvp {
		event: Id,
		occurrence: Id,
	},
	GetEvent(Id),
	Create(EventDraft),
	Edit {
		before: ScheduledEvent,
		after: EventDraft,
	},
	Status {
		event: Id,
		status: u8,
	},
	Delete(Id),
	Rsvp {
		event: Id,
		interested: bool,
	},
	GetStage {
		channel: Id,
		call_request: Option<u64>,
	},
	CreateStage {
		channel: Id,
		topic: String,
		event: Option<Id>,
	},
	EditStage {
		channel: Id,
		topic: String,
	},
	EndStage(Id),
	StageVoice {
		channel: Id,
		call_request: u64,
		change: StageVoiceChange,
	},
}
impl Action {
	pub fn write(&self) -> bool {
		!matches!(
			self,
			Self::ListEvents | Self::GetEvent(_) | Self::GetStage { .. }
		)
	}
	pub fn valid(&self) -> bool {
		if self.bytes() > 64 * 1024 {
			return false;
		}
		match self {
			Self::ListEvents => true,
			Self::EditOccurrence {
				before,
				original,
				start,
				end,
				..
			} => {
				before.valid()
					&& before
						.draft
						.recurrence
						.as_ref()
						.is_some_and(|r| r.matches_occurrence(*original))
					&& occurrence_id(*original).is_some()
					&& start.is_none_or(|s| s > 0)
					&& end.is_none_or(|e| e > start.unwrap_or(*original))
			}
			Self::RestoreOccurrence { before, exception } => {
				before.valid() && before.exceptions.iter().any(|e| e.id == *exception)
			}
			Self::ResetOccurrenceRsvp { event, occurrence }
			| Self::RsvpOccurrence {
				event, occurrence, ..
			} => event.0 != 0 && occurrence.0 != 0,
			Self::GetEvent(id) | Self::Delete(id) | Self::EndStage(id) => id.0 != 0,
			Self::GetStage {
				channel,
				call_request,
			} => channel.0 != 0 && call_request.is_none_or(|r| r != 0),
			Self::Create(d) => d.valid() && d.recurrence.as_ref().is_none_or(Recurrence::writable),
			Self::Edit { before, after } => {
				before.valid()
					&& after.valid()
					&& (before.draft.recurrence == after.recurrence
						|| after.recurrence.as_ref().is_none_or(Recurrence::writable))
			}
			Self::Status { event, status } => event.0 != 0 && (2..=4).contains(status),
			Self::Rsvp { event, .. } => event.0 != 0,
			Self::CreateStage {
				channel,
				topic,
				event,
			} => channel.0 != 0 && text(topic, 1, 120) && event.is_none_or(|e| e.0 != 0),
			Self::EditStage { channel, topic } => channel.0 != 0 && text(topic, 1, 120),
			Self::StageVoice {
				channel,
				call_request,
				change,
			} => {
				channel.0 != 0
					&& *call_request != 0
					&& match change {
						StageVoiceChange::Invite(id) | StageVoiceChange::MoveToAudience(id) => {
							id.0 != 0
						}
						_ => true,
					}
			}
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Create(d) => d.bytes(),
				Self::EditOccurrence { before, .. } | Self::RestoreOccurrence { before, .. } => {
					before.bytes()
				}
				Self::Edit { before, after } => before.bytes() + after.bytes(),
				Self::CreateStage { topic, .. } | Self::EditStage { topic, .. } => topic.capacity(),
				_ => 0,
			}
	}
	pub fn stage_channel(&self) -> Option<Id> {
		match self {
			Self::EndStage(c) => Some(*c),
			Self::GetStage { channel, .. } => Some(*channel),
			Self::CreateStage { channel, .. }
			| Self::EditStage { channel, .. }
			| Self::StageVoice { channel, .. } => Some(*channel),
			_ => None,
		}
	}
}
#[derive(Clone, Debug)]
pub struct StageVoiceSnapshot {
	pub user: Id,
	pub channel: Id,
	pub participation: StageParticipation,
}
#[derive(Clone, Debug)]
// The single community result lane accounts for the complete retained payload with bytes().
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
	Events(EventPage),
	Exception {
		event: Id,
		exception: EventException,
	},
	ExceptionRestored {
		event: Id,
		exception: Id,
	},
	Event(ScheduledEvent),
	Deleted(Id),
	Rsvp {
		event: Id,
		interested: bool,
		occurrence: Option<Id>,
	},
	OccurrenceRsvpReset {
		event: Id,
		occurrence: Id,
	},
	Stage {
		channel: Id,
		instance: Option<StageInstance>,
		voice: Option<StageVoiceSnapshot>,
	},
	StageVoice {
		channel: Id,
	},
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ match self {
				Self::Events(p) => p.bytes(),
				Self::Event(e) => e.bytes(),
				Self::Stage { instance, .. } => instance.as_ref().map_or(0, StageInstance::bytes),
				_ => 0,
			}
	}
}

/// Strict canonical event links. No arbitrary URL is opened or fetched by this parser.
pub fn event_link(value: &str) -> Option<(Id, Id)> {
	let path = value.strip_prefix("https://discord.com/events/")?;
	let mut parts = path.split('/');
	let guild = parts.next()?.parse::<u64>().ok()?;
	let event = parts.next()?.parse::<u64>().ok()?;
	(guild != 0 && event != 0 && parts.next().is_none()).then_some((Id(guild), Id(event)))
}

pub const MAX_EXCEPTIONS: usize = 64;
pub const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
pub fn occurrence_id(original: i64) -> Option<Id> {
	let ms = original.checked_mul(1000)?.checked_sub(DISCORD_EPOCH_MS)?;
	let value = u64::try_from(ms).ok()?;
	value
		.checked_shl(22)
		.filter(|id| *id != 0 && (*id >> 22) == value)
		.map(Id)
}
pub fn occurrence_time(id: Id) -> Option<i64> {
	i64::try_from(id.0 >> 22)
		.ok()?
		.checked_add(DISCORD_EPOCH_MS)
		.map(|ms| ms / 1000)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventException {
	pub event: Id,
	pub id: Id,
	pub canceled: bool,
	pub start: Option<i64>,
	pub end: Option<i64>,
}
impl EventException {
	pub fn valid(&self) -> bool {
		self.event.0 != 0
			&& self.id.0 != 0
			&& self
				.start
				.is_none_or(|s| (1..=253_402_300_799).contains(&s))
			&& self.end.is_none_or(|e| (1..=253_402_300_799).contains(&e))
			&& self.start.zip(self.end).is_none_or(|(s, e)| e > s)
	}
}
/// Gregorian calendar date from a UTC day index. Arithmetic stays within the RFC3339 range.
fn calendar(seconds: i64) -> (u8, u8, u8) {
	let days = seconds.div_euclid(86400);
	let z = days + 719468;
	let era = z.div_euclid(146097);
	let doe = z - era * 146097;
	let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let day = doy - (153 * mp + 2) / 5 + 1;
	let month = mp + if mp < 10 { 3 } else { -9 };
	(month as u8, day as u8, (days + 3).rem_euclid(7) as u8)
}
impl Recurrence {
	pub fn matches_occurrence(&self, at: i64) -> bool {
		if !self.writable()
			|| at < self.start
			|| at > 253_402_300_799
			|| at.rem_euclid(86400) != self.start.rem_euclid(86400)
		{
			return false;
		}
		let (month, day, weekday) = calendar(at);
		match self.frequency {
			0 => self.by_month == [month] && self.by_month_day == [day],
			1 => {
				self.by_n_weekday
					== [NWeekday {
						n: (day - 1) / 7 + 1,
						day: weekday,
					}]
			}
			2 => {
				self.by_weekday == [weekday]
					&& ((at.div_euclid(86400) - self.start.div_euclid(86400)
						+ (self.start.div_euclid(86400) + 3).rem_euclid(7))
						/ 7)
					.rem_euclid(i64::from(self.interval))
						== 0
			}
			3 => self.by_weekday.is_empty() || self.by_weekday.contains(&weekday),
			_ => false,
		}
	}
	/// At most sixteen results and ten years of UTC days; never an unbounded RRULE expansion.
	pub fn upcoming(&self, after: i64, limit: usize) -> Vec<i64> {
		let limit = limit.min(16);
		let mut results = Vec::with_capacity(limit);
		if limit == 0 || after > 253_402_300_799 || !self.writable() {
			return results;
		}
		let first = after.max(self.start).div_euclid(86400) * 86400 + self.start.rem_euclid(86400);
		for day in 0..3660 {
			let Some(at) = first.checked_add(day * 86400) else {
				break;
			};
			if at >= after && self.matches_occurrence(at) {
				results.push(at);
				if results.len() == limit {
					break;
				}
			}
		}
		results
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn rule(start: i64, frequency: u8) -> Recurrence {
		Recurrence {
			start,
			end: None,
			frequency,
			interval: 1,
			by_weekday: vec![],
			by_n_weekday: vec![],
			by_month: vec![],
			by_month_day: vec![],
			by_year_day: vec![],
			count: None,
		}
	}
	#[test]
	fn recurrence_calendar_leap_days_fifth_weekdays_and_fortnights_are_utc_and_bounded() {
		let mut yearly = rule(1_835_395_200, 0); // 2028-02-29 UTC
		yearly.by_month = vec![2];
		yearly.by_month_day = vec![29];
		assert_eq!(
			yearly.upcoming(yearly.start, 2),
			vec![1_835_395_200, 1_961_625_600]
		);
		assert!(!yearly.matches_occurrence(1_866_931_200)); // 2029-02-28
		let mut monthly = rule(1_895_875_200, 1); // fifth Tuesday, 2030-01-29
		monthly.by_n_weekday = vec![NWeekday { n: 5, day: 1 }];
		assert_eq!(
			monthly.upcoming(monthly.start, 2),
			vec![1_895_875_200, 1_903_737_600]
		); // April, not February's last Tuesday
		let mut fortnight = rule(1_893_456_000, 2); // Tuesday 2030-01-01
		fortnight.interval = 2;
		fortnight.by_weekday = vec![1];
		assert!(fortnight.matches_occurrence(1_894_665_600));
		assert!(!fortnight.matches_occurrence(1_894_060_800));
		assert!(!fortnight.matches_occurrence(fortnight.start + 1));
		let daily = rule(1_806_105_600 + 3600, 3); // 01:00 UTC across European DST
		assert_eq!(
			daily.upcoming(daily.start, 3),
			vec![daily.start, daily.start + 86400, daily.start + 172800]
		);
		assert_eq!(daily.upcoming(daily.start, usize::MAX).len(), 16);
		assert!(daily.upcoming(i64::MAX, 16).is_empty());
		assert!(daily.upcoming(daily.start, 0).is_empty());
		let mut service_rule = daily;
		service_rule.count = Some(10);
		assert!(service_rule.valid());
		assert!(!service_rule.writable());
		assert!(service_rule.upcoming(service_rule.start, 8).is_empty());
	}
	#[test]
	fn occurrence_snowflakes_and_retained_budgets_reject_out_of_scope_values() {
		let at = 1_893_456_000;
		let id = occurrence_id(at).unwrap();
		assert_eq!(occurrence_time(id), Some(at));
		assert!(occurrence_id(1).is_none());
		assert!(occurrence_id(i64::MAX).is_none());
		let mut r = rule(i64::MAX, 3);
		assert!(!r.valid());
		r.start = at;
		assert!(r.valid());
		let exception = EventException {
			event: Id(1),
			id,
			canceled: false,
			start: Some(at + 3600),
			end: Some(at + 1800),
		};
		assert!(!exception.valid());
		let huge = EventPage {
			guild: Id(1),
			events: Vec::with_capacity(MAX_EVENTS * 100),
			subscriptions: None,
			subscriptions_complete: false,
		};
		assert!(!huge.valid());
	}
	#[test]
	fn invitations_are_not_speaker_authorization() {
		assert_eq!(
			StageParticipation::from_wire(false, true),
			StageParticipation::Invited
		);
		assert_eq!(
			StageParticipation::from_wire(true, true),
			StageParticipation::Audience { requested: true }
		);
	}
	#[test]
	fn event_links_are_canonical() {
		assert_eq!(
			event_link("https://discord.com/events/1/2"),
			Some((Id(1), Id(2)))
		);
		for url in [
			"https://discord.com/events/0/2",
			"https://discord.com.evil/events/1/2",
			"https://discord.com/events/1/2?x=3",
			"https://discord.com/events/1/2/3",
		] {
			assert!(event_link(url).is_none());
		}
	}
}
