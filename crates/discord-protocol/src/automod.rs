//! Documented AutoMod wire shapes, bounded before collection growth.
use crate::{DecodeError, permissions::List};
use model::{Id, automod as m};
use serde::Deserialize;
#[derive(Default, Deserialize)]
struct Metadata {
	#[serde(default)]
	keyword_filter: List<String, 1000>,
	#[serde(default)]
	regex_patterns: List<String, 10>,
	#[serde(default)]
	presets: List<u8, 3>,
	#[serde(default)]
	allow_list: List<String, 1000>,
	#[serde(default)]
	mention_total_limit: u8,
	#[serde(default)]
	mention_raid_protection_enabled: bool,
}
#[derive(Default, Deserialize)]
struct EffectData {
	channel_id: Option<Id>,
	#[serde(default)]
	duration_seconds: u32,
	#[serde(default)]
	custom_message: Option<String>,
}
#[derive(Deserialize)]
struct Effect {
	#[serde(rename = "type")]
	kind: u8,
	#[serde(default)]
	metadata: EffectData,
}
#[derive(Deserialize)]
struct Rule {
	id: Id,
	guild_id: Id,
	creator_id: Id,
	name: String,
	event_type: u8,
	trigger_type: u8,
	#[serde(default)]
	trigger_metadata: Metadata,
	actions: List<Effect, 4>,
	enabled: bool,
	#[serde(default)]
	exempt_roles: List<Id, 20>,
	#[serde(default)]
	exempt_channels: List<Id, 50>,
}
impl Rule {
	fn checked(self, guild: Id) -> Result<m::Rule, DecodeError> {
		let metadata = self.trigger_metadata;
		let rule = m::Rule {
			id: self.id,
			guild: self.guild_id,
			creator: self.creator_id,
			settings: m::Settings {
				name: self.name,
				event: self.event_type,
				trigger: self.trigger_type,
				enabled: self.enabled,
				metadata: m::Metadata {
					keywords: metadata.keyword_filter.0,
					regex: metadata.regex_patterns.0,
					presets: metadata.presets.0,
					allow: metadata.allow_list.0,
					mentions: metadata.mention_total_limit,
					raid_protection: metadata.mention_raid_protection_enabled,
				},
				effects: self
					.actions
					.0
					.into_iter()
					.map(|effect| m::Effect {
						kind: effect.kind,
						channel: effect.metadata.channel_id,
						duration: effect.metadata.duration_seconds,
						message: effect.metadata.custom_message.unwrap_or_default(),
					})
					.collect(),
				exempt_roles: self.exempt_roles.0,
				exempt_channels: self.exempt_channels.0,
			},
		};
		(rule.guild == guild && rule.valid())
			.then_some(rule)
			.ok_or(DecodeError)
	}
}
pub fn rules(bytes: &[u8], guild: Id) -> Result<m::Snapshot, DecodeError> {
	let rows = crate::decode::<List<Rule, { m::MAX_RULES }>>(bytes)?.0;
	let snapshot = m::Snapshot {
		guild,
		rules: rows
			.into_iter()
			.map(|row| row.checked(guild))
			.collect::<Result<_, _>>()?,
	};
	snapshot.valid().then_some(snapshot).ok_or(DecodeError)
}
pub fn rule(bytes: &[u8], guild: Id) -> Result<m::Rule, DecodeError> {
	crate::decode::<Rule>(bytes)?.checked(guild)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn rule_pages_enforce_scope_counts_metadata_and_action_types() {
		let base = serde_json::json!({"id":"10","guild_id":"2","creator_id":"1","name":"Synthetic","trigger_type":1,"event_type":1,"trigger_metadata":{"keyword_filter":["sample"]},"actions":[{"type":1}],"enabled":false});
		assert!(rule(&serde_json::to_vec(&base).unwrap(), Id(2)).is_ok());
		assert!(rule(&serde_json::to_vec(&base).unwrap(), Id(3)).is_err());
		assert!(rules(&serde_json::to_vec(&vec![base.clone(); 2]).unwrap(), Id(2)).is_err());
		assert!(
			rules(
				&serde_json::to_vec(&vec![base.clone(); m::MAX_RULES + 1]).unwrap(),
				Id(2)
			)
			.is_err()
		);
		let mut oversized = base.clone();
		oversized["trigger_metadata"]["keyword_filter"] = serde_json::json!(vec!["x"; 1001]);
		assert!(rule(&serde_json::to_vec(&oversized).unwrap(), Id(2)).is_err());
		for (kind, event, effect) in [(3, 1, 1), (4, 1, 1), (5, 1, 3), (6, 2, 4)] {
			let mut row = base.clone();
			row["trigger_type"] = serde_json::json!(kind);
			row["event_type"] = serde_json::json!(event);
			row["trigger_metadata"] = serde_json::json!({});
			row["actions"] = serde_json::json!([{"type":effect,"metadata":if effect==3 {serde_json::json!({"duration_seconds":60})} else {serde_json::json!({})}}]);
			assert!(rule(&serde_json::to_vec(&row).unwrap(), Id(2)).is_ok());
		}
	}
}
