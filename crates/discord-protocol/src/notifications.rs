//! Unofficial normal-user settings/session payloads; unknown guild preferences disable OS alerts.
use model::Id;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct MuteConfig {
	#[serde(default)]
	pub end_time: Option<crate::Timestamp>,
}
impl MuteConfig {
	pub fn until(&self) -> Option<i64> {
		self.end_time
			.as_ref()
			.and_then(|at| i64::try_from(at.0 / 1_000_000_000).ok())
	}
}
#[derive(Deserialize)]
#[serde(from = "OverrideWire")]
pub struct Override {
	pub flags: Option<u64>,
	pub collapsed: Option<bool>,
	pub channel_id: Id,
	pub mute_config: Option<MuteConfig>,
	pub muted: Option<bool>,
	pub message_notifications: Option<u8>,
	absent: u8,
}
#[derive(Deserialize)]
struct OverrideWire {
	#[serde(default)]
	flags: model::Patch<u64>,
	#[serde(default)]
	collapsed: model::Patch<bool>,
	channel_id: Id,
	#[serde(default)]
	mute_config: model::Patch<MuteConfig>,
	#[serde(default)]
	muted: model::Patch<bool>,
	#[serde(default)]
	message_notifications: model::Patch<u8>,
}
impl From<OverrideWire> for Override {
	fn from(wire: OverrideWire) -> Self {
		let mut absent = 0;
		let flags = match wire.flags {
			model::Patch::Value(v) => Some(v),
			model::Patch::Null => None,
			model::Patch::Absent => {
				absent |= 4;
				None
			}
		};
		let collapsed = match wire.collapsed {
			model::Patch::Value(v) => Some(v),
			model::Patch::Null => None,
			model::Patch::Absent => {
				absent |= 8;
				None
			}
		};
		let muted = match wire.muted {
			model::Patch::Value(value) => Some(value),
			model::Patch::Null => None,
			model::Patch::Absent => {
				absent |= 1;
				None
			}
		};
		let message_notifications = match wire.message_notifications {
			model::Patch::Value(value) => Some(value),
			model::Patch::Null => None,
			model::Patch::Absent => {
				absent |= 2;
				None
			}
		};
		let mute_config = match wire.mute_config {
			model::Patch::Value(value) => Some(value),
			model::Patch::Null => None,
			model::Patch::Absent => {
				absent |= 16;
				None
			}
		};
		Self {
			flags,
			collapsed,
			channel_id: wire.channel_id,
			mute_config,
			muted,
			message_notifications,
			absent,
		}
	}
}
#[derive(Deserialize)]
pub struct Overrides(
	#[serde(deserialize_with = "crate::read_state::account_entries")] pub Vec<Override>,
);
pub struct Setting {
	pub version: Option<u64>,
	pub flags: Option<u64>,
	pub guild_id: Option<Id>,
	pub mute_config: Option<MuteConfig>,
	pub muted: Option<bool>,
	pub suppress_everyone: Option<bool>,
	pub suppress_roles: Option<bool>,
	pub hide_muted_channels: Option<bool>,
	pub message_notifications: Option<u8>,
	pub channel_overrides: Option<Overrides>,
	pub overrides_unknown: bool,
	absent: u8,
}
impl<'de> Deserialize<'de> for Setting {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		struct Wire {
			#[serde(default)]
			version: Option<i64>,
			#[serde(default)]
			flags: model::Patch<u64>,
			#[serde(default, deserialize_with = "crate::read_state::optional_id")]
			guild_id: Option<Id>,
			#[serde(default)]
			mute_config: model::Patch<MuteConfig>,
			#[serde(default)]
			muted: model::Patch<bool>,
			#[serde(default)]
			suppress_everyone: model::Patch<bool>,
			#[serde(default)]
			suppress_roles: model::Patch<bool>,
			#[serde(default)]
			hide_muted_channels: model::Patch<bool>,
			#[serde(default)]
			message_notifications: model::Patch<u8>,
			#[serde(default)]
			channel_overrides: model::Patch<Overrides>,
		}
		fn value<T>(patch: model::Patch<T>, bit: u8, absent: &mut u8) -> Option<T> {
			match patch {
				model::Patch::Absent => {
					*absent |= bit;
					None
				}
				model::Patch::Null => None,
				model::Patch::Value(value) => Some(value),
			}
		}
		let wire = Wire::deserialize(deserializer)?;
		let mut absent = 0;
		let mute_config = value(wire.mute_config, 64, &mut absent);
		let overrides_unknown = matches!(wire.channel_overrides, model::Patch::Null);
		let channel_overrides = match wire.channel_overrides {
			model::Patch::Value(rows) => Some(rows),
			_ => None,
		};
		Ok(Self {
			version: wire.version.and_then(|value| u64::try_from(value).ok()),
			flags: value(wire.flags, 32, &mut absent),
			guild_id: wire.guild_id,
			mute_config,
			muted: value(wire.muted, 1, &mut absent),
			suppress_everyone: value(wire.suppress_everyone, 2, &mut absent),
			suppress_roles: value(wire.suppress_roles, 4, &mut absent),
			hide_muted_channels: value(wire.hide_muted_channels, 8, &mut absent),
			message_notifications: value(wire.message_notifications, 16, &mut absent),
			channel_overrides,
			overrides_unknown,
			absent,
		})
	}
}
impl Setting {
	/// Shared normalized representation; full Gateway records alone default omitted values.
	pub fn into_update(self, full: bool) -> model::guild_preferences::Update {
		use model::{
			Patch,
			guild_preferences::{Fields, Row, Update},
		};
		fn patch<T>(value: Option<T>, absent: bool, full: bool) -> Patch<T> {
			if absent && !full {
				Patch::Absent
			} else {
				value.map_or(Patch::Null, Patch::Value)
			}
		}
		let setting = if full { self.with_defaults() } else { self };
		let channels = setting
			.channel_overrides
			.map(|rows| {
				rows.0
					.into_iter()
					.map(|row| Row {
						channel: row.channel_id,
						fields: Fields {
							flags: patch(row.flags, row.absent & 4 != 0, full),
							collapsed: patch(row.collapsed, row.absent & 8 != 0, full),
							muted: patch(row.muted, row.absent & 1 != 0, full),
							level: patch(row.message_notifications, row.absent & 2 != 0, full),
							mute_until: patch(
								row.mute_config.as_ref().and_then(MuteConfig::until),
								row.absent & 16 != 0,
								full,
							),
							..Fields::default()
						},
					})
					.collect()
			})
			.or_else(|| (full && !setting.overrides_unknown).then(Vec::new));
		Update {
			guild: setting.guild_id,
			version: setting.version,
			full,
			fields: Fields {
				flags: patch(setting.flags, setting.absent & 32 != 0, full),
				muted: patch(setting.muted, setting.absent & 1 != 0, full),
				level: patch(
					setting.message_notifications,
					setting.absent & 16 != 0,
					full,
				),
				suppress_everyone: patch(setting.suppress_everyone, setting.absent & 2 != 0, full),
				suppress_roles: patch(setting.suppress_roles, setting.absent & 4 != 0, full),
				hide_muted: patch(setting.hide_muted_channels, setting.absent & 8 != 0, full),
				mute_until: patch(
					setting.mute_config.as_ref().and_then(MuteConfig::until),
					setting.absent & 64 != 0,
					full,
				),
				..Fields::default()
			},
			channels,
			channels_unknown: setting.overrides_unknown,
		}
	}
	/// Gateway objects are full settings records. Only omitted fields use service defaults;
	/// explicit null stays unknown. REST acknowledgements deliberately skip this conversion.
	pub fn with_defaults(mut self) -> Self {
		if self.absent & 32 != 0 {
			self.flags = Some(0);
		}
		if self.absent & 1 != 0 {
			self.muted = Some(false);
		}
		if self.absent & 2 != 0 {
			self.suppress_everyone = Some(false);
		}
		if self.absent & 4 != 0 {
			self.suppress_roles = Some(false);
		}
		if self.absent & 8 != 0 {
			self.hide_muted_channels = Some(false);
		}
		if self.absent & 16 != 0 {
			self.message_notifications = Some(3);
		}
		if let Some(overrides) = &mut self.channel_overrides {
			for row in &mut overrides.0 {
				if row.absent & 4 != 0 {
					row.flags = Some(0);
				}
				if row.absent & 8 != 0 {
					row.collapsed = Some(false);
				}
				if row.absent & 1 != 0 {
					row.muted = Some(false);
				}
				if row.absent & 2 != 0 {
					row.message_notifications = Some(3);
				}
			}
		}
		self
	}
}
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Snapshot {
	Versioned {
		#[serde(default)]
		version: Option<u64>,
		#[serde(deserialize_with = "crate::read_state::account_entries")]
		entries: Vec<Setting>,
		#[serde(default)]
		partial: bool,
	},
	Legacy(#[serde(deserialize_with = "crate::read_state::account_entries")] Vec<Setting>),
}
impl Snapshot {
	pub fn entries(self) -> (Vec<Setting>, bool) {
		match self {
			Self::Versioned {
				mut entries,
				partial,
				version,
			} => {
				for entry in &mut entries {
					if entry.version.is_none() {
						entry.version = version;
					}
				}
				(entries, !partial)
			}
			Self::Legacy(entries) => (entries, true),
		}
	}
}
#[derive(Deserialize)]
pub struct Session {
	pub status: String,
}
#[derive(Deserialize)]
pub struct Sessions(#[serde(deserialize_with = "crate::read_state::entries")] pub Vec<Session>);
impl Sessions {
	pub fn dnd(&self) -> Option<bool> {
		if self.0.iter().any(|s| s.status == "dnd") {
			return Some(true);
		}
		(!self.0.is_empty()
			&& self.0.iter().all(|s| {
				matches!(
					s.status.as_str(),
					"online" | "idle" | "offline" | "invisible"
				)
			}))
		.then_some(false)
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn defaults_apply_only_to_omitted_gateway_fields_not_rest_confirmation() {
		let sparse: Setting = crate::decode(br#"{"guild_id":"1"}"#).unwrap();
		assert_eq!(sparse.muted, None);
		assert_eq!(sparse.message_notifications, None);
		let defaults = sparse.with_defaults();
		assert_eq!(defaults.muted, Some(false));
		assert_eq!(defaults.message_notifications, Some(3));
		assert_eq!(defaults.suppress_everyone, Some(false));
		let unknown: Setting =
			crate::decode(br#"{"guild_id":"1","muted":null,"message_notifications":null}"#)
				.unwrap();
		let unknown = unknown.with_defaults();
		assert_eq!(unknown.muted, None);
		assert_eq!(unknown.message_notifications, None);
		let unknown = crate::decode::<Setting>(br#"{"guild_id":"1","channel_overrides":null}"#)
			.unwrap()
			.into_update(true);
		assert!(unknown.channels_unknown);
		assert_eq!(unknown.channels, None);
	}

	#[test]
	fn normalized_updates_preserve_sparse_presence_nulls_and_versions() {
		use model::Patch;
		let sparse:Setting=crate::decode(br#"{"guild_id":"1","version":14,"hide_muted_channels":false,"muted":null,"channel_overrides":[{"channel_id":"2","flags":4096,"mute_config":null}]}"#).unwrap();
		let update = sparse.into_update(false);
		assert!(!update.full);
		assert_eq!(update.version, Some(14));
		assert_eq!(update.fields.flags, Patch::Absent);
		assert_eq!(update.fields.muted, Patch::Null);
		assert_eq!(update.fields.hide_muted, Patch::Value(false));
		let row = &update.channels.as_ref().unwrap()[0];
		assert_eq!(row.fields.mute_until, Patch::Null);
		assert_eq!(row.fields.muted, Patch::Absent);
		assert_eq!(row.fields.flags, Patch::Value(4096));
		assert!(update.valid());
		let snapshot: Snapshot =
			crate::decode(br#"{"version":99,"entries":[{"guild_id":"1"}],"partial":true}"#)
				.unwrap();
		let (rows, replace) = snapshot.entries();
		assert!(!replace);
		let update = rows.into_iter().next().unwrap().into_update(true);
		assert_eq!(update.version, Some(99));
		assert_eq!(update.fields.flags, Patch::Value(0));
		assert_eq!(update.channels, Some(vec![]));
		let record: Setting = crate::decode(br#"{"guild_id":"1","version":-1}"#).unwrap();
		assert_eq!(record.version, None);
	}

	#[test]
	fn mention_suppression_settings_preserve_unknown_false_and_true() {
		for (fields, everyone, roles) in [
			(r#""#, None, None),
			(
				r#", "suppress_everyone":null,"suppress_roles":null"#,
				None,
				None,
			),
			(
				r#", "suppress_everyone":false,"suppress_roles":true"#,
				Some(false),
				Some(true),
			),
			(
				r#", "suppress_everyone":true,"suppress_roles":false"#,
				Some(true),
				Some(false),
			),
		] {
			let setting: Setting =
				crate::decode(format!(r#"{{"guild_id":"1"{fields}}}"#).as_bytes()).unwrap();
			assert_eq!(
				(setting.suppress_everyone, setting.suppress_roles),
				(everyone, roles)
			);
		}
		assert!(crate::decode::<Setting>(br#"{"guild_id":"1","suppress_roles":"false"}"#).is_err());
		assert!(crate::decode::<Setting>(br#"{"guild_id":"1","suppress_everyone":0}"#).is_err());
	}
	#[test]
	fn bounded_preferences_and_unknown_presence_fail_closed() {
		let snapshot: Snapshot = crate::decode(br#"{"entries":[{"guild_id":null,"muted":false,"message_notifications":0,"channel_overrides":[{"channel_id":"2","muted":true,"message_notifications":2}]}],"partial":false}"#).unwrap();
		let (settings, complete) = snapshot.entries();
		assert!(complete);
		assert_eq!(
			settings[0].channel_overrides.as_ref().unwrap().0[0].channel_id,
			Id(2)
		);
		assert_eq!(
			settings[0].channel_overrides.as_ref().unwrap().0[0].muted,
			Some(true)
		);
		assert_eq!(
			crate::decode::<Sessions>(br#"[{"status":"online"},{"status":"dnd"}]"#)
				.unwrap()
				.dnd(),
			Some(true)
		);
		assert_eq!(
			crate::decode::<Sessions>(br#"[{"status":"new-status"}]"#)
				.unwrap()
				.dnd(),
			None
		);
		assert_eq!(crate::decode::<Sessions>(br#"[]"#).unwrap().dnd(), None);
		let oversized = serde_json::json!({"entries":vec![serde_json::json!({"guild_id":null});model::account::MAX_ENTRIES+1]});
		assert!(crate::decode::<Snapshot>(&serde_json::to_vec(&oversized).unwrap()).is_err());
	}
}

#[cfg(test)]
mod personal_tests {
	use super::*;
	#[test]
	fn personal_flags_and_collapse_preserve_unknown_bits_and_explicit_null() {
		let record: Setting=crate::decode(br#"{"guild_id":"2","flags":36028797018980352,"channel_overrides":[{"channel_id":"3","flags":18014398509486080,"collapsed":true}]}"#).unwrap();
		let record = record.with_defaults();
		assert_eq!(record.flags, Some((1 << 55) | (1 << 14)));
		let row = &record.channel_overrides.unwrap().0[0];
		assert_eq!(row.flags, Some((1 << 54) | (1 << 12)));
		assert_eq!(row.collapsed, Some(true));
		assert_eq!(row.muted, Some(false));
		let record: Setting=crate::decode(br#"{"guild_id":"2","flags":null,"channel_overrides":[{"channel_id":"3","flags":null,"collapsed":null}]}"#).unwrap();
		let record = record.with_defaults();
		assert_eq!(record.flags, None);
		let row = &record.channel_overrides.unwrap().0[0];
		assert_eq!((row.flags, row.collapsed), (None, None));
	}
}
