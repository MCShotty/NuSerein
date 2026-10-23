//! Unofficial normal-account application and access metadata. All parsing is bounded.
use crate::{DecodeError, Timestamp, UserDto, permissions::List};
use model::{Id, server_access as m};
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const MAX_WIRE: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
struct Field {
	field_type: String,
	label: String,
	#[serde(default)]
	values: Option<List<String, 16>>,
	#[serde(default)]
	choices: Option<List<String, 8>>,
	#[serde(default)]
	response: Value,
}
#[derive(Deserialize)]
struct Request {
	id: Id,
	join_request_id: Option<Id>,
	guild_id: Id,
	user_id: Id,
	user: Option<UserDto>,
	application_status: String,
	created_at: Timestamp,
	reviewed_at: Option<Timestamp>,
	actioned_by_user: Option<UserDto>,
	rejection_reason: Option<String>,
	form_responses: Option<List<Field, 5>>,
	interview_channel_id: Option<Id>,
}
fn status(value: &str) -> Result<m::Status, DecodeError> {
	match value {
		"SUBMITTED" => Ok(m::Status::Pending),
		"APPROVED" => Ok(m::Status::Approved),
		"REJECTED" => Ok(m::Status::Rejected),
		_ => Err(DecodeError),
	}
}
impl Request {
	fn checked(self, guild: Id) -> Result<m::Application, DecodeError> {
		if self.guild_id != guild || self.join_request_id.is_some_and(|id| id != self.id) {
			return Err(DecodeError);
		}
		let answers = self
			.form_responses
			.map_or_else(Vec::new, |v| v.0)
			.into_iter()
			.map(|field| {
				let answer = match field.response {
					Value::Null => "Not supplied".into(),
					Value::String(v) => v,
					Value::Bool(v) => {
						if v {
							"Accepted".into()
						} else {
							"Not accepted".into()
						}
					}
					Value::Number(v) => {
						let index = v
							.as_u64()
							.and_then(|v| usize::try_from(v).ok())
							.ok_or(DecodeError)?;
						let choices = field.choices.or(field.values);
						if let Some(choices) = choices {
							choices.0.get(index).cloned().ok_or(DecodeError)?
						} else {
							format!("Choice {}", index.checked_add(1).ok_or(DecodeError)?)
						}
					}
					_ => return Err(DecodeError),
				};
				if field.field_type.len() > 64
					|| !field
						.field_type
						.bytes()
						.all(|b| b.is_ascii_uppercase() || b == b'_')
				{
					return Err(DecodeError);
				}
				Ok(m::Answer {
					label: field.label,
					answer,
				})
			})
			.collect::<Result<Vec<_>, DecodeError>>()?;
		let result = m::Application {
			id: self.id,
			guild,
			user_id: self.user_id,
			user: self.user.map(UserDto::into_model),
			status: status(&self.application_status)?,
			created_at: self.created_at.0,
			reviewed_at: self.reviewed_at.map(|v| v.0),
			moderator: self.actioned_by_user.map(UserDto::into_model),
			reason: self.rejection_reason,
			answers,
			interview: self.interview_channel_id,
		};
		result.valid().then_some(result).ok_or(DecodeError)
	}
}
pub fn application(bytes: &[u8], guild: Id, id: Id) -> Result<m::Application, DecodeError> {
	if bytes.len() > 64 * 1024 {
		return Err(DecodeError);
	}
	let v = crate::decode::<Request>(bytes)?.checked(guild)?;
	(v.id == id).then_some(v).ok_or(DecodeError)
}
pub fn applications(
	bytes: &[u8],
	guild: Id,
	filter: m::Status,
) -> Result<m::Applications, DecodeError> {
	if bytes.len() > MAX_WIRE {
		return Err(DecodeError);
	}
	#[derive(Deserialize)]
	struct Page {
		guild_join_requests: List<Request, 100>,
		total: Option<u64>,
	}
	let page: Page = crate::decode(bytes)?;
	let items = page
		.guild_join_requests
		.0
		.into_iter()
		.map(|v| v.checked(guild))
		.collect::<Result<Vec<_>, _>>()?;
	let next = if items.len() == 100 {
		items.last().map(|v| v.id)
	} else {
		None
	};
	let result = m::Applications {
		guild,
		status: filter,
		items,
		total: page.total,
		next,
	};
	result.valid().then_some(result).ok_or(DecodeError)
}
#[derive(Deserialize)]
struct Incident {
	invites_disabled_until: Option<Timestamp>,
	dms_disabled_until: Option<Timestamp>,
}
/// Optional cosmetic/session metadata must not make otherwise valid READY fail.
pub fn incident_patch<'de, D: serde::Deserializer<'de>>(
	d: D,
) -> Result<model::Patch<m::Incident>, D::Error> {
	let raw = <&'de serde_json::value::RawValue>::deserialize(d)?;
	if raw.get() == "null" {
		return Ok(model::Patch::Null);
	}
	if raw.get().len() > 4096 {
		return Ok(model::Patch::Absent);
	}
	let value = serde_json::from_str::<Incident>(raw.get())
		.ok()
		.map(|v| m::Incident {
			invites_until: v.invites_disabled_until.map(|v| v.0),
			dms_until: v.dms_disabled_until.map(|v| v.0),
		})
		.filter(m::Incident::valid);
	Ok(value.map_or(model::Patch::Absent, model::Patch::Value))
}
pub fn incident_update(bytes: &[u8]) -> Result<Option<(Id, Option<m::Incident>)>, DecodeError> {
	#[derive(Deserialize)]
	struct Update {
		id: Id,
		#[serde(default, deserialize_with = "incident_patch")]
		incidents_data: model::Patch<m::Incident>,
	}
	let value: Update = crate::decode(bytes)?;
	if value.id.0 == 0 {
		return Err(DecodeError);
	}
	Ok(match value.incidents_data {
		model::Patch::Absent => None,
		model::Patch::Null => Some((value.id, None)),
		model::Patch::Value(incident) => Some((value.id, Some(incident))),
	})
}
pub fn application_update(bytes: &[u8], deleted: bool) -> Result<Option<(Id, Id)>, DecodeError> {
	#[derive(Deserialize)]
	struct Key {
		id: Id,
		join_request_id: Option<Id>,
		application_status: String,
	}
	#[derive(Deserialize)]
	struct Update {
		guild_id: Id,
		request: Key,
	}
	#[derive(Deserialize)]
	struct Deleted {
		guild_id: Id,
		id: Id,
	}
	if bytes.len() > 64 * 1024 {
		return Err(DecodeError);
	}
	let pair = if deleted {
		let v: Deleted = crate::decode(bytes)?;
		(v.guild_id, v.id)
	} else {
		let v: Update = crate::decode(bytes)?;
		if v.request.application_status == "STARTED" {
			return Ok(None);
		}
		status(&v.request.application_status)?;
		if v.request
			.join_request_id
			.is_some_and(|id| id != v.request.id)
		{
			return Err(DecodeError);
		}
		(v.guild_id, v.request.id)
	};
	if pair.0.0 == 0 || pair.1.0 == 0 {
		return Err(DecodeError);
	}
	Ok(Some(pair))
}
#[derive(Deserialize)]
struct Guild {
	id: Id,
	owner_id: Id,
	features: List<String, 256>,
	rules_channel_id: Option<Id>,
	public_updates_channel_id: Option<Id>,
	safety_alerts_channel_id: Option<Id>,
	mfa_level: u8,
	verification_level: u8,
	explicit_content_filter: u8,
	#[serde(default)]
	incidents_data: model::Patch<Incident>,
}
pub fn settings(bytes: &[u8], guild: Id) -> Result<m::Settings, DecodeError> {
	if bytes.len() > MAX_WIRE {
		return Err(DecodeError);
	}
	let dto: Guild = crate::decode(bytes)?;
	if dto.id != guild {
		return Err(DecodeError);
	}
	let features = dto.features.0;
	let configuration = m::Configuration {
		community: features.iter().any(|v| v == "COMMUNITY"),
		rules: dto.rules_channel_id,
		updates: dto.public_updates_channel_id,
		alerts: dto.safety_alerts_channel_id,
	};
	let result = m::Settings {
		guild,
		owner: dto.owner_id,
		features,
		configuration,
		mfa_level: dto.mfa_level,
		verification_level: dto.verification_level,
		content_filter: dto.explicit_content_filter,
		incident: match dto.incidents_data {
			model::Patch::Absent => None,
			model::Patch::Null => Some(m::Incident::default()),
			model::Patch::Value(v) => Some(m::Incident {
				invites_until: v.invites_disabled_until.map(|v| v.0),
				dms_until: v.dms_disabled_until.map(|v| v.0),
			}),
		},
	};
	result.valid().then_some(result).ok_or(DecodeError)
}
fn date(value: Option<i128>) -> Result<Value, DecodeError> {
	value
		.map(|at| {
			time::OffsetDateTime::from_unix_timestamp_nanos(at)
				.map_err(|_| DecodeError)?
				.format(&time::format_description::well_known::Rfc3339)
				.map(Value::String)
				.map_err(|_| DecodeError)
		})
		.transpose()
		.map(|v| v.unwrap_or(Value::Null))
}
pub fn incident_body(incident: m::Incident) -> Result<Value, DecodeError> {
	if !incident.valid() {
		return Err(DecodeError);
	}
	Ok(
		json!({"invites_disabled_until":date(incident.invites_until)?,"dms_disabled_until":date(incident.dms_until)?}),
	)
}
pub fn configuration_body(
	current: &m::Settings,
	before: m::Configuration,
	after: m::Configuration,
) -> Result<Value, DecodeError> {
	if !current.valid() || !after.valid() || current.configuration != before {
		return Err(DecodeError);
	}
	let mut map = Map::new();
	for (key, old, new) in [
		("rules_channel_id", before.rules, after.rules),
		("public_updates_channel_id", before.updates, after.updates),
		("safety_alerts_channel_id", before.alerts, after.alerts),
	] {
		if old != new {
			map.insert(key.into(), json!(new));
		}
	}
	if before.community != after.community {
		if after.community && (current.verification_level < 1 || current.content_filter != 2) {
			return Err(DecodeError);
		}
		let mut features = current.features.clone();
		features.retain(|v| v != "COMMUNITY");
		if after.community {
			features.push("COMMUNITY".into());
		}
		map.insert("features".into(), json!(features));
	}
	Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
	use super::*;
	fn request() -> Value {
		json!({"id":"10","join_request_id":"10","guild_id":"2","user_id":"3","application_status":"SUBMITTED","created_at":"2026-01-01T00:00:00Z","user":{"id":"3","username":"applicant","discriminator":"0","avatar":null},"form_responses":[{"field_type":"MULTIPLE_CHOICE","label":"Interest","choices":["Art","Music"],"response":1}],"interview_channel_id":null})
	}
	#[test]
	fn applications_use_canonical_request_identity_and_reject_cross_guild_nested_answers() {
		let value = request();
		let parsed = application(value.to_string().as_bytes(), Id(2), Id(10)).unwrap();
		assert_eq!(parsed.answers[0].answer, "Music");
		assert!(application(value.to_string().as_bytes(), Id(2), Id(3)).is_err());
		let mut invalid = value.clone();
		invalid["join_request_id"] = json!("11");
		assert!(application(invalid.to_string().as_bytes(), Id(2), Id(10)).is_err());
		invalid = value.clone();
		invalid["guild_id"] = json!("4");
		assert!(application(invalid.to_string().as_bytes(), Id(2), Id(10)).is_err());
		invalid = value;
		invalid["form_responses"][0]["response"] = json!({"nested":[1,2]});
		assert!(application(invalid.to_string().as_bytes(), Id(2), Id(10)).is_err());
	}
	#[test]
	fn application_pages_reject_wrong_status_duplicates_and_oversized_lists() {
		let page = json!({"guild_join_requests":[request()],"total":1});
		assert!(applications(page.to_string().as_bytes(), Id(2), m::Status::Pending).is_ok());
		assert!(applications(page.to_string().as_bytes(), Id(2), m::Status::Approved).is_err());
		for n in [2, 101] {
			let page = json!({"guild_join_requests":vec![request();n]});
			assert!(applications(page.to_string().as_bytes(), Id(2), m::Status::Pending).is_err());
		}
	}
	#[test]
	fn incident_updates_distinguish_missing_null_invalid_and_service_values() {
		assert!(incident_update(br#"{"id":"2"}"#).unwrap().is_none());
		assert_eq!(
			incident_update(br#"{"id":"2","incidents_data":null}"#).unwrap(),
			Some((Id(2), None))
		);
		assert!(
			incident_update(br#"{"id":"2","incidents_data":{"invites_disabled_until":"invalid"}}"#)
				.unwrap()
				.is_none()
		);
		let value=incident_update(br#"{"id":"2","incidents_data":{"invites_disabled_until":"2027-01-01T00:00:00Z","dms_disabled_until":null}}"#).unwrap().unwrap();
		assert_eq!(value.0, Id(2));
		assert!(value.1.unwrap().invites_until.is_some());
		let metadata = json!({"id":"2","owner_id":"1","features":[],"mfa_level":0,"verification_level":0,"explicit_content_filter":0,"incidents_data":null});
		assert_eq!(
			settings(metadata.to_string().as_bytes(), Id(2))
				.unwrap()
				.incident,
			Some(m::Incident::default())
		);
	}
	#[test]
	fn application_lifecycle_uses_request_ids_and_ignores_unsubmitted_forms() {
		let value = json!({"guild_id":"2","request":request(),"status":"SUBMITTED"});
		assert_eq!(
			application_update(value.to_string().as_bytes(), false).unwrap(),
			Some((Id(2), Id(10)))
		);
		assert_eq!(
			application_update(br#"{"guild_id":"2","id":"10","user_id":"3"}"#, true).unwrap(),
			Some((Id(2), Id(10)))
		);
		let mut value = value;
		value["request"]["application_status"] = json!("STARTED");
		assert_eq!(
			application_update(value.to_string().as_bytes(), false).unwrap(),
			None
		);
	}
	#[test]
	fn configuration_changes_are_sparse_and_preserve_future_features() {
		let value = json!({"id":"2","owner_id":"1","features":["FUTURE"],"mfa_level":0,"verification_level":1,"explicit_content_filter":2,"incidents_data":{"invites_disabled_until":null,"dms_disabled_until":null}});
		let current = settings(value.to_string().as_bytes(), Id(2)).unwrap();
		let after = m::Configuration {
			community: true,
			rules: Some(Id(3)),
			updates: Some(Id(4)),
			alerts: None,
		};
		let body = configuration_body(&current, current.configuration, after).unwrap();
		assert_eq!(
			body,
			json!({"features":["FUTURE","COMMUNITY"],"rules_channel_id":"3","public_updates_channel_id":"4"})
		);
		assert_eq!(
			incident_body(m::Incident::default()).unwrap(),
			json!({"invites_disabled_until":null,"dms_disabled_until":null})
		);
	}
}
