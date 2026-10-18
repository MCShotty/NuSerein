//! Official onboarding schema; member verification and Server Guide are unofficial user routes.
//! Raw JSON is short-lived here so an edit can retain fields unknown to this client.
use crate::DecodeError;
use model::{Id, community_admin::*};
use serde_json::{Map, Value, json};

pub const MAX_WIRE: usize = 512 * 1024;
fn fail<T>() -> Result<T, DecodeError> {
	Err(DecodeError)
}
fn arr<'a>(v: &'a Value, k: &str, max: usize) -> Result<&'a [Value], DecodeError> {
	match v.get(k) {
		None | Some(Value::Null) => Ok(&[]),
		Some(Value::Array(a)) if a.len() <= max => Ok(a),
		_ => fail(),
	}
}
fn string(v: &Value, k: &str, max: usize) -> Result<String, DecodeError> {
	let s = match v.get(k) {
		None | Some(Value::Null) => "",
		Some(Value::String(s)) => s,
		_ => return fail(),
	};
	if s.chars().count() > max || s.len() > max * 4 {
		return fail();
	}
	Ok(s.to_owned())
}
fn optional(v: &Value, k: &str, max: usize) -> Result<Option<String>, DecodeError> {
	if v.get(k).is_none_or(Value::is_null) {
		Ok(None)
	} else {
		string(v, k, max).map(Some)
	}
}
fn boolean(v: &Value, k: &str) -> Result<bool, DecodeError> {
	match v.get(k) {
		None | Some(Value::Null) => Ok(false),
		Some(Value::Bool(b)) => Ok(*b),
		_ => fail(),
	}
}
fn number(v: &Value, k: &str) -> Result<u8, DecodeError> {
	match v.get(k) {
		None | Some(Value::Null) => Ok(0),
		Some(v) => v
			.as_u64()
			.and_then(|n| u8::try_from(n).ok())
			.ok_or(DecodeError),
	}
}
fn id(v: &Value) -> Result<Id, DecodeError> {
	v.as_str()
		.and_then(|s| s.parse().ok())
		.filter(|id: &Id| id.0 != 0)
		.ok_or(DecodeError)
}
fn ids(v: &Value, k: &str, max: usize) -> Result<Vec<Id>, DecodeError> {
	arr(v, k, max)?.iter().map(id).collect()
}
fn texts(v: &Value, k: &str, max: usize, chars: usize) -> Result<Vec<String>, DecodeError> {
	arr(v, k, max)?
		.iter()
		.map(|v| {
			v.as_str()
				.filter(|s| s.chars().count() <= chars)
				.map(str::to_owned)
				.ok_or(DecodeError)
		})
		.collect()
}
fn emoji(v: &Value) -> Result<Emoji, DecodeError> {
	let Some(e) = v.get("emoji").filter(|v| !v.is_null()) else {
		return Ok(Emoji::default());
	};
	if !e.is_object() {
		return fail();
	}
	Ok(Emoji {
		id: e.get("id").filter(|v| !v.is_null()).map(id).transpose()?,
		name: optional(e, "name", 128)?,
		animated: boolean(e, "animated")?,
	})
}
pub fn raw(bytes: &[u8]) -> Result<Value, DecodeError> {
	if bytes.len() > MAX_WIRE {
		return fail();
	}
	let value: Value = serde_json::from_slice(bytes).map_err(|_| DecodeError)?;
	if !value.is_object() {
		return fail();
	}
	Ok(value)
}
pub fn parse(
	guild: Id,
	section: Section,
	value: &Value,
	enabled: bool,
	manual: bool,
) -> Result<Snapshot, DecodeError> {
	if !value.is_object()
		|| value
			.get("guild_id")
			.is_some_and(|v| id(v).ok() != Some(guild))
	{
		return fail();
	}
	let data = match section {
		Section::Onboarding => Data::Onboarding(Onboarding {
			enabled: boolean(value, "enabled")?,
			mode: number(value, "mode")?,
			below_requirements: boolean(value, "below_requirements")?,
			default_channels: ids(value, "default_channel_ids", MAX_DEFAULT_CHANNELS)?,
			prompts: arr(value, "prompts", MAX_PROMPTS)?
				.iter()
				.map(|p| {
					Ok(Prompt {
						id: id(&p["id"])?,
						kind: number(p, "type")?,
						title: string(p, "title", 100)?,
						single_select: boolean(p, "single_select")?,
						required: boolean(p, "required")?,
						in_onboarding: boolean(p, "in_onboarding")?,
						options: arr(p, "options", MAX_OPTIONS)?
							.iter()
							.map(|o| {
								Ok(OptionRow {
									id: id(&o["id"])?,
									title: string(o, "title", 50)?,
									description: optional(o, "description", 100)?,
									emoji: emoji(o)?,
									channels: ids(o, "channel_ids", MAX_ASSIGNMENTS)?,
									roles: ids(o, "role_ids", MAX_ASSIGNMENTS)?,
								})
							})
							.collect::<Result<_, DecodeError>>()?,
					})
				})
				.collect::<Result<_, DecodeError>>()?,
		}),
		Section::Screening => Data::Screening(Screening {
			enabled,
			manual_approval: manual,
			version: optional(value, "version", 64)?,
			description: optional(value, "description", 300)?,
			fields: arr(value, "form_fields", MAX_FIELDS)?
				.iter()
				.enumerate()
				.map(|(index, f)| {
					Ok(Field {
						source: Some(index),
						kind: string(f, "field_type", 64)?,
						label: string(f, "label", 300)?,
						description: optional(f, "description", 300)?,
						placeholder: optional(f, "placeholder", 300)?,
						required: boolean(f, "required")?,
						values: texts(f, "values", 16, 300)?,
						choices: texts(f, "choices", 8, 150)?,
					})
				})
				.collect::<Result<_, DecodeError>>()?,
		}),
		Section::Guide => {
			let channels = |key, max| {
				arr(value, key, max)?
					.iter()
					.map(|r| {
						Ok(GuideChannel {
							channel: id(&r["channel_id"])?,
							action: number(r, "action_type")?,
							title: if r.get("title").is_none() {
								string(r, "name", 60)?
							} else {
								string(r, "title", 60)?
							},
							description: string(r, "description", 200)?,
							emoji: emoji(r)?,
							icon: optional(r, "icon", 128)?,
						})
					})
					.collect::<Result<Vec<_>, DecodeError>>()
			};
			Data::Guide(Guide {
				enabled: boolean(value, "enabled")?,
				authors: ids(&value["welcome_message"], "author_ids", 10)?,
				message: string(&value["welcome_message"], "message", 300)?,
				actions: channels("new_member_actions", MAX_ACTIONS)?,
				resources: channels("resource_channels", MAX_RESOURCES)?,
				..Default::default()
			})
		}
	};
	let result = Snapshot {
		guild,
		data,
		roles: vec![],
	};
	if !result.valid() {
		return fail();
	}
	Ok(result)
}
fn e(e: &Emoji) -> Value {
	json!({"id":e.id,"name":e.name,"animated":e.animated})
}
fn known(data: &Data) -> Value {
	match data {
		Data::Onboarding(o) => {
			json!({"enabled":o.enabled,"mode":o.mode,"default_channel_ids":o.default_channels,"prompts":o.prompts.iter().map(|p|json!({"id":p.id,"type":p.kind,"title":p.title,"single_select":p.single_select,"required":p.required,"in_onboarding":p.in_onboarding,"options":p.options.iter().map(|o|json!({"id":o.id,"title":o.title,"description":o.description,"emoji":e(&o.emoji),"channel_ids":o.channels,"role_ids":o.roles})).collect::<Vec<_>>() })).collect::<Vec<_>>() })
		}
		Data::Screening(s) => {
			json!({"enabled":s.enabled,"description":s.description,"form_fields":s.fields.iter().map(|f|json!({"field_type":f.kind,"label":f.label,"description":f.description,"placeholder":f.placeholder,"required":f.required,"values":f.values,"choices":f.choices})).collect::<Vec<_>>() })
		}
		Data::Guide(g) => {
			let rows = |rows: &[GuideChannel], actions: bool| {
				rows.iter().map(|r|{let mut v=json!({"channel_id":r.channel,"title":r.title,"description":r.description,"emoji":e(&r.emoji)});if actions {v["action_type"]=json!(r.action);}v}).collect::<Vec<_>>()
			};
			json!({"enabled":g.enabled,"welcome_message":{"author_ids":g.authors,"message":g.message},"new_member_actions":rows(&g.actions,true),"resource_channels":rows(&g.resources,false)})
		}
	}
}
fn merge_object(raw: &Value, before: &Value, after: &Value) -> Value {
	let mut merged = raw.as_object().cloned().unwrap_or_default();
	for (key, value) in after.as_object().into_iter().flatten() {
		if before.get(key) != Some(value) {
			merged.insert(key.clone(), value.clone());
		}
	}
	Value::Object(merged)
}
fn merge_rows(raw: &Value, before: &Value, after: &Value, key: &str, options: bool) -> Value {
	Value::Array(
		after
			.as_array()
			.into_iter()
			.flatten()
			.map(|row| {
				let old = before
					.as_array()
					.into_iter()
					.flatten()
					.find(|v| v.get(key) == row.get(key));
				let original = raw
					.as_array()
					.into_iter()
					.flatten()
					.find(|v| v.get(key) == row.get(key));
				let mut merged = merge_object(
					original.unwrap_or(&Value::Null),
					old.unwrap_or(&Value::Null),
					row,
				);
				if options {
					merged["options"] = merge_rows(
						&original.unwrap_or(&Value::Null)["options"],
						&old.unwrap_or(&Value::Null)["options"],
						&row["options"],
						"id",
						false,
					);
				}
				merged
			})
			.collect(),
	)
}
fn option_emoji(body: &mut Value) {
	if let Some(prompts) = body.get_mut("prompts").and_then(Value::as_array_mut) {
		for prompt in prompts {
			if let Some(options) = prompt.get_mut("options").and_then(Value::as_array_mut) {
				for option in options {
					if let Some(object) = option.as_object_mut() {
						let emoji = object.remove("emoji").unwrap_or(Value::Null);
						object.insert("emoji_id".into(), emoji["id"].clone());
						object.insert("emoji_name".into(), emoji["name"].clone());
						object.insert(
							"emoji_animated".into(),
							json!(emoji["animated"].as_bool().unwrap_or(false)),
						);
					}
				}
			}
		}
	}
}
/// Builds a sparse write from a fresh raw response. Conflicting touched fields never overwrite
/// another administrator's edits; untouched fields and opaque nested fields remain intact.
pub fn edit(
	guild: Id,
	latest_raw: &Value,
	before: &Data,
	after: &Data,
	pending: PendingApplications,
	enabled: bool,
	manual: bool,
) -> Result<Value, DecodeError> {
	if before.section() != after.section() || !before.valid() || !after.editable() {
		return fail();
	}
	let latest = parse(guild, before.section(), latest_raw, enabled, manual)?;
	let before_known = known(before);
	let after_known = known(after);
	let latest_known = known(&latest.data);
	let mut body = Map::new();
	for (key, after_value) in after_known.as_object().ok_or(DecodeError)? {
		let before_value = &before_known[key];
		if before_value == after_value {
			continue;
		}
		if &latest_known[key] != before_value {
			return fail();
		}
		let value = match key.as_str() {
			"prompts" => merge_rows(&latest_raw[key], before_value, after_value, "id", true),
			"new_member_actions" | "resource_channels" => merge_rows(
				&latest_raw[key],
				before_value,
				after_value,
				"channel_id",
				false,
			),
			"form_fields" => {
				let Data::Screening(before) = before else {
					return fail();
				};
				let Data::Screening(after) = after else {
					return fail();
				};
				Value::Array(
					after
						.fields
						.iter()
						.enumerate()
						.map(|(i, f)| {
							let old = f.source.and_then(|source| {
								before.fields.iter().position(|f| f.source == Some(source))
							});
							let original = f
								.source
								.and_then(|source| latest_raw[key].get(source))
								.unwrap_or(&Value::Null);
							let mut row = merge_object(
								original,
								old.map(|i| &before_value[i]).unwrap_or(&Value::Null),
								&after_value[i],
							);
							row.as_object_mut().unwrap().remove("response");
							row
						})
						.collect(),
				)
			}
			"welcome_message" => merge_object(&latest_raw[key], before_value, after_value),
			_ => after_value.clone(),
		};
		body.insert(key.clone(), value);
	}
	if let (Data::Screening(before), Data::Screening(after)) = (before, after)
		&& before.enabled
		&& !after.enabled
	{
		body.insert(
			"bulk_action".into(),
			json!(match pending {
				PendingApplications::Approve => "APPROVED",
				PendingApplications::Reject => "REJECTED",
			}),
		);
	}
	let mut body = Value::Object(body);
	option_emoji(&mut body);
	if serde_json::to_vec(&body).map_err(|_| DecodeError)?.len() > MAX_WIRE {
		return fail();
	}
	Ok(body)
}
/// Read-only fields and Discord-assigned new IDs need not match a submitted draft.
pub fn confirms(before: &Data, after: &Data, actual: &Data) -> bool {
	Data::confirms(before, after, actual)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn new_onboarding_rows_retain_client_generated_ids() {
		let raw = json!({"guild_id":"1","prompts":[]});
		let before = parse(Id(1), Section::Onboarding, &raw, false, false)
			.unwrap()
			.data;
		let mut after = before.clone();
		let Data::Onboarding(config) = &mut after else {
			panic!()
		};
		config.prompts.push(Prompt {
			id: Id(1_792_276_800_000),
			title: "Choose a channel".into(),
			options: vec![OptionRow {
				id: Id(1_792_276_800_001),
				title: "General".into(),
				channels: vec![Id(2)],
				..Default::default()
			}],
			..Default::default()
		});
		let body = edit(
			Id(1),
			&raw,
			&before,
			&after,
			PendingApplications::Approve,
			false,
			false,
		)
		.unwrap();
		assert_eq!(body["prompts"][0]["id"], "1792276800000");
		assert_eq!(body["prompts"][0]["options"][0]["id"], "1792276800001");
	}
	#[test]
	fn guide_collection_edits_use_title_and_preserve_icon_and_unknown_fields() {
		let raw = json!({"guild_id":"1","new_member_actions":[{"channel_id":"2","action_type":0,"title":"Read","description":"","icon":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","future":{"keep":true}}]});
		let before = parse(Id(1), Section::Guide, &raw, false, false)
			.unwrap()
			.data;
		let mut after = before.clone();
		let Data::Guide(config) = &mut after else {
			panic!()
		};
		config.actions[0].title = "Read the guide".into();
		let body = edit(
			Id(1),
			&raw,
			&before,
			&after,
			PendingApplications::Approve,
			false,
			false,
		)
		.unwrap();
		assert_eq!(body["new_member_actions"][0]["title"], "Read the guide");
		assert!(body["new_member_actions"][0].get("name").is_none());
		assert_eq!(
			body["new_member_actions"][0]["icon"],
			raw["new_member_actions"][0]["icon"]
		);
		assert_eq!(body["new_member_actions"][0]["future"]["keep"], true);
	}
	#[test]
	fn sparse_edit_retains_opaque_option_fields_and_uses_official_emoji_fields() {
		let raw = json!({"guild_id":"1","enabled":false,"mode":0,"prompts":[{"id":"10","type":0,"title":"Choose","extra":"prompt","options":[{"id":"11","title":"Hi","emoji":{"id":null,"name":"😀"},"unknown_option":{"keep":true},"channel_ids":["2"],"role_ids":[]}]}]});
		let before = parse(Id(1), Section::Onboarding, &raw, false, false)
			.unwrap()
			.data;
		let mut after = before.clone();
		let Data::Onboarding(o) = &mut after else {
			panic!()
		};
		o.prompts[0].options[0].title = "Hello".into();
		let body = edit(
			Id(1),
			&raw,
			&before,
			&after,
			PendingApplications::Approve,
			false,
			false,
		)
		.unwrap();
		assert_eq!(body["prompts"][0]["extra"], "prompt");
		assert_eq!(
			body["prompts"][0]["options"][0]["unknown_option"]["keep"],
			true
		);
		assert!(body.get("enabled").is_none());
		assert!(body["prompts"][0]["options"][0].get("emoji").is_none());
		assert_eq!(body["prompts"][0]["options"][0]["emoji_name"], "😀");
	}
	#[test]
	fn concurrent_touched_collection_is_rejected_and_untouched_changes_survive() {
		let raw = json!({"guild_id":"1","enabled":false,"mode":0,"prompts":[]});
		let before = parse(Id(1), Section::Onboarding, &raw, false, false)
			.unwrap()
			.data;
		let mut after = before.clone();
		let Data::Onboarding(o) = &mut after else {
			panic!()
		};
		o.mode = 1;
		let mut fresh = raw.clone();
		fresh["enabled"] = json!(true);
		assert_eq!(
			edit(
				Id(1),
				&fresh,
				&before,
				&after,
				PendingApplications::Approve,
				false,
				false
			)
			.unwrap(),
			json!({"mode":1})
		);
		fresh["mode"] = json!(1);
		assert!(
			edit(
				Id(1),
				&fresh,
				&before,
				&after,
				PendingApplications::Approve,
				false,
				false
			)
			.is_err()
		);
	}
	#[test]
	fn screening_reorder_keeps_automation_with_its_source_field() {
		let raw = json!({"form_fields":[{"field_type":"TERMS","label":"A","values":["A"],"automations":["opaque-A"]},{"field_type":"TERMS","label":"B","values":["B"],"automations":["opaque-B"]}]});
		let before = parse(Id(1), Section::Screening, &raw, true, false)
			.unwrap()
			.data;
		let mut after = before.clone();
		let Data::Screening(s) = &mut after else {
			panic!()
		};
		s.fields.swap(0, 1);
		let body = edit(
			Id(1),
			&raw,
			&before,
			&after,
			PendingApplications::Approve,
			true,
			false,
		)
		.unwrap();
		assert_eq!(body["form_fields"][0]["automations"][0], "opaque-B");
	}
	#[test]
	fn bounds_and_guild_identity_are_strict() {
		assert!(
			parse(
				Id(1),
				Section::Guide,
				&json!({"guild_id":"2"}),
				false,
				false
			)
			.is_err()
		);
		assert!(
			parse(
				Id(1),
				Section::Onboarding,
				&json!({"default_channel_ids":vec!["1";501]}),
				false,
				false
			)
			.is_err()
		);
		assert!(raw(&vec![b' '; MAX_WIRE + 1]).is_err());
	}
}
