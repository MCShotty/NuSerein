//! Bounded official scheduled-event/Stage DTOs. RSVP subscriptions are unofficial.
use model::{Id, community::*};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub fn timestamp(value: &str) -> Result<i64, &'static str> {
	if value.len() > 64 {
		return Err("event timestamp exceeds limit");
	}
	OffsetDateTime::parse(value, &Rfc3339)
		.map(|t| t.unix_timestamp())
		.map_err(|_| "invalid event timestamp")
}
pub fn iso(value: i64) -> Result<String, &'static str> {
	OffsetDateTime::from_unix_timestamp(value)
		.map_err(|_| "event time outside range")?
		.format(&Rfc3339)
		.map_err(|_| "event time outside range")
}
#[derive(Deserialize)]
struct RecurrenceDto {
	start: String,
	#[serde(default)]
	end: Option<String>,
	frequency: u8,
	interval: u8,
	#[serde(default)]
	by_weekday: Option<Vec<u8>>,
	#[serde(default)]
	by_n_weekday: Option<Vec<NWeekday>>,
	#[serde(default)]
	by_month: Option<Vec<u8>>,
	#[serde(default)]
	by_month_day: Option<Vec<u8>>,
	#[serde(default)]
	by_year_day: Option<Vec<u16>>,
	#[serde(default)]
	count: Option<u32>,
}
impl RecurrenceDto {
	fn model(self) -> Result<Recurrence, &'static str> {
		let r = Recurrence {
			start: timestamp(&self.start)?,
			end: self.end.as_deref().map(timestamp).transpose()?,
			frequency: self.frequency,
			interval: self.interval,
			by_weekday: self.by_weekday.unwrap_or_default(),
			by_n_weekday: self.by_n_weekday.unwrap_or_default(),
			by_month: self.by_month.unwrap_or_default(),
			by_month_day: self.by_month_day.unwrap_or_default(),
			by_year_day: self.by_year_day.unwrap_or_default(),
			count: self.count,
		};
		r.valid()
			.then_some(r)
			.ok_or("event recurrence exceeds limits")
	}
}
#[derive(Deserialize)]
struct Metadata {
	location: Option<String>,
}
#[derive(Deserialize)]
struct EventDto {
	#[serde(default)]
	guild_scheduled_event_exceptions: Vec<ExceptionDto>,
	id: Id,
	guild_id: Id,
	#[serde(default)]
	creator_id: Option<Id>,
	#[serde(default)]
	channel_id: Option<Id>,
	name: String,
	#[serde(default)]
	description: Option<String>,
	scheduled_start_time: String,
	#[serde(default)]
	scheduled_end_time: Option<String>,
	privacy_level: u8,
	status: u8,
	entity_type: u8,
	#[serde(default)]
	entity_metadata: Option<Metadata>,
	#[serde(default)]
	user_count: Option<u32>,
	#[serde(default)]
	recurrence_rule: Option<RecurrenceDto>,
}
impl EventDto {
	fn model(self, guild: Id) -> Result<ScheduledEvent, &'static str> {
		if self.guild_id != guild || self.privacy_level != 2 {
			return Err("event scope mismatch");
		}
		let event = ScheduledEvent {
			id: self.id,
			guild,
			creator: self.creator_id,
			status: self.status,
			exceptions: self
				.guild_scheduled_event_exceptions
				.into_iter()
				.map(|e| e.model(self.id))
				.collect::<Result<Vec<_>, _>>()?,
			user_count: self.user_count,
			draft: EventDraft {
				name: self.name,
				description: self.description.unwrap_or_default(),
				channel: self.channel_id,
				entity_type: self.entity_type,
				location: self
					.entity_metadata
					.and_then(|m| m.location)
					.unwrap_or_default(),
				start: timestamp(&self.scheduled_start_time)?,
				end: self
					.scheduled_end_time
					.as_deref()
					.map(timestamp)
					.transpose()?,
				recurrence: self.recurrence_rule.map(RecurrenceDto::model).transpose()?,
			},
		};
		event
			.valid()
			.then_some(event)
			.ok_or("invalid scheduled event")
	}
}
pub fn event(bytes: &[u8], guild: Id) -> Result<ScheduledEvent, &'static str> {
	if bytes.len() > 64 * 1024 {
		return Err("event response exceeds limit");
	}
	crate::decode::<EventDto>(bytes)
		.map_err(|_| "invalid event response")?
		.model(guild)
}
pub fn events(bytes: &[u8], guild: Id) -> Result<EventPage, &'static str> {
	if bytes.len() > MAX_WIRE {
		return Err("events response exceeds limit");
	}
	let wire = crate::decode::<Vec<EventDto>>(bytes).map_err(|_| "invalid events response")?;
	if wire.len() > MAX_EVENTS {
		return Err("too many scheduled events");
	}
	let events = wire
		.into_iter()
		.map(|e| e.model(guild))
		.collect::<Result<Vec<_>, _>>()?;
	let page = EventPage {
		guild,
		events,
		subscriptions: None,
		subscriptions_complete: false,
	};
	page.valid()
		.then_some(page)
		.ok_or("events exceed retained budget")
}
#[derive(Deserialize)]
struct ExceptionDto {
	event_id: Id,
	event_exception_id: Id,
	is_canceled: bool,
	#[serde(default)]
	scheduled_start_time: Option<String>,
	#[serde(default)]
	scheduled_end_time: Option<String>,
}
impl ExceptionDto {
	fn model(self, event: Id) -> Result<EventException, &'static str> {
		if self.event_id != event {
			return Err("occurrence event scope mismatch");
		}
		let e = EventException {
			event,
			id: self.event_exception_id,
			canceled: self.is_canceled,
			start: self
				.scheduled_start_time
				.as_deref()
				.map(timestamp)
				.transpose()?,
			end: self
				.scheduled_end_time
				.as_deref()
				.map(timestamp)
				.transpose()?,
		};
		e.valid().then_some(e).ok_or("invalid occurrence exception")
	}
}
pub fn event_exception(bytes: &[u8], event: Id) -> Result<EventException, &'static str> {
	if bytes.len() > 8192 {
		return Err("occurrence exceeds wire budget");
	}
	crate::decode::<ExceptionDto>(bytes)
		.map_err(|_| "invalid occurrence exception")?
		.model(event)
}
#[derive(Deserialize)]
struct SubscriptionDto {
	guild_scheduled_event_id: Id,
	#[serde(default)]
	guild_scheduled_event_exception_id: Option<Id>,
	user_id: Id,
	response: u8,
}
pub fn subscriptions(bytes: &[u8], user: Id) -> Result<Vec<Subscription>, &'static str> {
	if bytes.len() > 128 * 1024 {
		return Err("subscriptions exceed wire budget");
	}
	let values =
		crate::decode::<Vec<SubscriptionDto>>(bytes).map_err(|_| "invalid subscriptions")?;
	if values.len() > MAX_EVENTS * 4
		|| values
			.iter()
			.any(|s| s.user_id != user || s.guild_scheduled_event_id.0 == 0 || s.response > 1)
	{
		return Err("subscriptions exceed scope or budget");
	}
	Ok(values
		.into_iter()
		.map(|s| Subscription {
			event: s.guild_scheduled_event_id,
			occurrence: s.guild_scheduled_event_exception_id,
			interested: s.response == 1,
			user: s.user_id,
		})
		.collect())
}
pub fn subscription(bytes: &[u8], user: Id, event: Id) -> Result<(), &'static str> {
	subscription_response(bytes, user, event, None)
}
pub fn subscription_response(
	bytes: &[u8],
	user: Id,
	event: Id,
	occurrence: Option<Id>,
) -> Result<(), &'static str> {
	if bytes.len() > 4096 {
		return Err("subscription exceeds wire budget");
	}
	let value = crate::decode::<SubscriptionDto>(bytes).map_err(|_| "invalid subscription")?;
	(value.user_id == user
		&& value.guild_scheduled_event_id == event
		&& value.guild_scheduled_event_exception_id == occurrence
		&& value.response == 1)
		.then_some(())
		.ok_or("subscription scope mismatch")
}
#[derive(Deserialize)]
struct StageDto {
	id: Id,
	guild_id: Id,
	channel_id: Id,
	topic: String,
	privacy_level: u8,
	#[serde(default)]
	guild_scheduled_event_id: Option<Id>,
}
pub fn stage(bytes: &[u8], guild: Id, channel: Id) -> Result<StageInstance, &'static str> {
	if bytes.len() > 8192 {
		return Err("stage exceeds wire budget");
	}
	let dto = crate::decode::<StageDto>(bytes).map_err(|_| "invalid stage")?;
	if dto.guild_id != guild || dto.channel_id != channel || dto.privacy_level != 2 {
		return Err("stage scope mismatch");
	}
	let stage = StageInstance {
		id: dto.id,
		guild,
		channel,
		topic: dto.topic,
		event: dto.guild_scheduled_event_id,
	};
	stage
		.valid()
		.then_some(stage)
		.ok_or("stage exceeds retained budget")
}
fn recurrence_body(r: &Recurrence) -> Result<Value, &'static str> {
	if !r.writable() {
		return Err("unsupported recurrence write");
	}
	let optional = |v: &[u8]| if v.is_empty() { Value::Null } else { json!(v) };
	Ok(
		json!({"start":iso(r.start)?,"frequency":r.frequency,"interval":r.interval,"by_weekday":optional(&r.by_weekday),"by_n_weekday":if r.by_n_weekday.is_empty(){Value::Null}else{json!(r.by_n_weekday)},"by_month":optional(&r.by_month),"by_month_day":optional(&r.by_month_day)}),
	)
}
/// Sparse edits retain fields unknown to this client and service-only recurrence data.
pub fn event_body(after: &EventDraft, before: Option<&EventDraft>) -> Result<Value, &'static str> {
	if !after.valid() {
		return Err("invalid event draft");
	}
	let mut body = serde_json::Map::new();
	macro_rules! field {
		($name:literal,$field:ident,$value:expr) => {
			if before.is_none_or(|b| b.$field != after.$field) {
				body.insert($name.into(), $value);
			}
		};
	}
	field!("name", name, json!(after.name));
	field!(
		"description",
		description,
		if after.description.is_empty() {
			Value::Null
		} else {
			json!(after.description)
		}
	);
	field!("scheduled_start_time", start, json!(iso(after.start)?));
	field!(
		"scheduled_end_time",
		end,
		after
			.end
			.map(iso)
			.transpose()?
			.map_or(Value::Null, Value::String)
	);
	if before.is_none() {
		body.insert("privacy_level".into(), json!(2));
	}
	if before.is_none_or(|b| {
		b.entity_type != after.entity_type
			|| b.channel != after.channel
			|| b.location != after.location
	}) {
		body.insert("entity_type".into(), json!(after.entity_type));
		body.insert(
			"channel_id".into(),
			json!(after.channel.map(|c| c.to_string())),
		);
		body.insert(
			"entity_metadata".into(),
			if after.entity_type == 3 {
				json!({"location":after.location})
			} else {
				Value::Null
			},
		);
	}
	if before.is_none_or(|b| b.recurrence != after.recurrence) {
		body.insert(
			"recurrence_rule".into(),
			after
				.recurrence
				.as_ref()
				.map(recurrence_body)
				.transpose()?
				.unwrap_or(Value::Null),
		);
	}
	Ok(Value::Object(body))
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn timezone_offsets_refer_to_the_same_instant_and_invalid_times_fail() {
		assert_eq!(
			timestamp("2026-10-17T10:00:00+02:00"),
			timestamp("2026-10-17T08:00:00Z")
		);
		for bad in [
			"2026-02-30T10:00:00Z",
			"2026-10-17T10:00:00",
			"2026-10-17T25:00:00Z",
		] {
			assert!(timestamp(bad).is_err());
		}
	}
	#[test]
	fn sparse_edits_preserve_unsupported_recurrence_and_reject_cross_guild() {
		let bytes=br#"{"id":"9","guild_id":"1","channel_id":null,"name":"Test","scheduled_start_time":"2027-01-01T10:00:00Z","scheduled_end_time":"2027-01-01T11:00:00Z","privacy_level":2,"status":1,"entity_type":3,"entity_metadata":{"location":"Outside"},"recurrence_rule":{"start":"2027-01-01T10:00:00Z","frequency":2,"interval":1,"by_weekday":[4],"count":3}}"#;
		let old = event(bytes, Id(1)).unwrap();
		let mut new = old.draft.clone();
		new.name = "Changed".into();
		let body = event_body(&new, Some(&old.draft)).unwrap();
		assert_eq!(body, json!({"name":"Changed"}));
		assert!(event_body(&old.draft, None).is_err());
		assert!(event(bytes, Id(2)).is_err());
	}
	#[test]
	fn budgets_duplicates_and_subscription_identity_are_enforced() {
		assert!(events(&vec![b' '; MAX_WIRE + 1], Id(1)).is_err());
		assert!(
			subscriptions(
				br#"[{"guild_scheduled_event_id":"2","user_id":"3","response":1}]"#,
				Id(4)
			)
			.is_err()
		);
		assert!(
			stage(
				br#"{"id":"1","guild_id":"2","channel_id":"3","topic":"Topic","privacy_level":2}"#,
				Id(2),
				Id(4)
			)
			.is_err()
		);
	}
	#[test]
	fn occurrence_and_stage_reads_validate_identity_and_never_infer_missing_suppression() {
		let id = occurrence_id(timestamp("2030-01-01T08:00:00Z").unwrap()).unwrap();
		let bytes=json!({"event_id":"30","event_exception_id":id.to_string(),"is_canceled":true,"scheduled_start_time":"2030-01-01T11:00:00+02:00","scheduled_end_time":null}).to_string();
		let exception = event_exception(bytes.as_bytes(), Id(30)).unwrap();
		assert!(exception.canceled);
		assert_eq!(
			exception.start,
			Some(timestamp("2030-01-01T09:00:00Z").unwrap())
		);
		assert!(event_exception(bytes.as_bytes(), Id(31)).is_err());
		let subscribed=json!({"guild_scheduled_event_id":"30","guild_scheduled_event_exception_id":id.to_string(),"user_id":"1","response":1}).to_string();
		assert!(subscription_response(subscribed.as_bytes(), Id(1), Id(30), Some(id)).is_ok());
		assert!(subscription(subscribed.as_bytes(), Id(1), Id(30)).is_err());
		for (extra, expected) in [
			(json!({}), StageParticipation::Unknown),
			(json!({"suppress":false}), StageParticipation::Speaker),
			(
				json!({"suppress":false,"request_to_speak_timestamp":"2030-01-01T09:00:00Z"}),
				StageParticipation::Invited,
			),
			(
				json!({"suppress":false,"request_to_speak_timestamp":"invalid"}),
				StageParticipation::Unknown,
			),
		] {
			let mut voice = json!({"user_id":"1","guild_id":"10","channel_id":"21"});
			voice
				.as_object_mut()
				.unwrap()
				.extend(extra.as_object().unwrap().clone());
			let bytes = voice.to_string();
			assert_eq!(
				stage_voice(bytes.as_bytes(), Id(10), Id(21), Id(1))
					.unwrap()
					.participation,
				expected
			);
			assert!(stage_voice(bytes.as_bytes(), Id(10), Id(21), Id(2)).is_err());
		}
	}
}

/// This read never retains session IDs and never grants microphone consent.
pub fn stage_voice(
	bytes: &[u8],
	guild: Id,
	channel: Id,
	user: Id,
) -> Result<StageVoiceSnapshot, &'static str> {
	if bytes.len() > 8192 {
		return Err("voice state exceeds wire budget");
	}
	let dto =
		crate::decode::<crate::VoiceStateDto>(bytes).map_err(|_| "invalid stage voice state")?;
	if dto.user_id != user
		|| dto.channel_id != Some(channel)
		|| dto.guild_id.is_some_and(|g| g != guild)
	{
		return Err("stage voice scope mismatch");
	}
	let participation = dto
		.suppress
		.filter(|_| {
			dto.request_to_speak_timestamp
				.as_deref()
				.is_none_or(|value| timestamp(value).is_ok())
		})
		.map_or(StageParticipation::Unknown, |s| {
			StageParticipation::from_wire(s, dto.request_to_speak_timestamp.is_some())
		});
	Ok(StageVoiceSnapshot {
		user,
		channel,
		participation,
	})
}
