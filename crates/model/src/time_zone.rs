//! Device-local display timezone; timestamps themselves always retain their UTC instant.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TimeZonePreference {
	#[default]
	System,
	Named(String),
	/// Minutes east of UTC, from -14:00 through +14:00.
	FixedOffset(i16),
}

impl TimeZonePreference {
	pub fn is_valid(&self) -> bool {
		match self {
			Self::System => true,
			Self::FixedOffset(minutes) => (-840..=840).contains(minutes),
			Self::Named(name) => {
				!name.is_empty()
					&& name.len() <= 128
					&& name
						.split('/')
						.all(|part| !part.is_empty() && part != "." && part != "..")
					&& name.bytes().all(|byte| {
						byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+')
					})
			}
		}
	}

	pub fn parse_fixed(value: &str) -> Option<Self> {
		let value = value.trim().strip_prefix("UTC").unwrap_or(value.trim());
		let bytes = value.as_bytes();
		if bytes.len() != 6
			|| !matches!(bytes[0], b'+' | b'-')
			|| bytes[3] != b':'
			|| ![bytes[1], bytes[2], bytes[4], bytes[5]]
				.into_iter()
				.all(|b| b.is_ascii_digit())
		{
			return None;
		}
		let hours = i16::from(bytes[1] - b'0') * 10 + i16::from(bytes[2] - b'0');
		let minutes = i16::from(bytes[4] - b'0') * 10 + i16::from(bytes[5] - b'0');
		if minutes >= 60 {
			return None;
		}
		let minutes = (hours * 60 + minutes) * if bytes[0] == b'-' { -1 } else { 1 };
		let result = Self::FixedOffset(minutes);
		result.is_valid().then_some(result)
	}

	pub fn fixed_label(minutes: i16) -> String {
		let absolute = i32::from(minutes).abs();
		format!(
			"UTC{}{:02}:{:02}",
			if minutes < 0 { '-' } else { '+' },
			absolute / 60,
			absolute % 60
		)
	}
}

impl serde::Serialize for TimeZonePreference {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::System => serializer.serialize_str("system"),
			Self::Named(name) => serializer.serialize_str(&format!("iana:{name}")),
			Self::FixedOffset(minutes) => serializer.serialize_str(&format!("offset:{minutes}")),
		}
	}
}

impl<'de> serde::Deserialize<'de> for TimeZonePreference {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(serde::Deserialize)]
		#[serde(untagged)]
		enum Stored {
			Text(String),
			Other(serde::de::IgnoredAny),
		}
		let Stored::Text(value) = Stored::deserialize(deserializer)? else {
			return Ok(Self::System);
		};
		let result = if let Some(name) = value.strip_prefix("iana:") {
			Self::Named(name.to_owned())
		} else if let Some(minutes) = value.strip_prefix("offset:").and_then(|s| s.parse().ok()) {
			Self::FixedOffset(minutes)
		} else {
			Self::System
		};
		Ok(if result.is_valid() {
			result
		} else {
			Self::System
		})
	}
}

#[cfg(test)]
mod tests {
	use super::TimeZonePreference;

	#[test]
	fn fixed_offsets_validate_minutes_boundaries_and_quarter_hours() {
		for (text, minutes) in [
			("UTC+05:45", 345),
			("-03:30", -210),
			("+14:00", 840),
			("-14:00", -840),
			("+00:00", 0),
		] {
			assert_eq!(
				TimeZonePreference::parse_fixed(text),
				Some(TimeZonePreference::FixedOffset(minutes))
			);
			assert_eq!(
				TimeZonePreference::parse_fixed(&TimeZonePreference::fixed_label(minutes)),
				Some(TimeZonePreference::FixedOffset(minutes))
			);
		}
		for invalid in [
			"+14:01",
			"-14:30",
			"+05:60",
			"+5:45",
			"05:45",
			"UTC+00:00junk",
			"−03:00",
		] {
			assert_eq!(TimeZonePreference::parse_fixed(invalid), None, "{invalid}");
		}
	}
}
