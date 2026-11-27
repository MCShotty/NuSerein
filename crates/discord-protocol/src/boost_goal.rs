//! Unofficial, display-only projection of one legacy READY rollout. Never triggers exposures.
//! See the archived Canary build 681713, modules 343446 and 831617.
use model::{Id, Patch, permissions::murmur3};
use serde::Deserialize;
use serde_json::value::RawValue;

const NAME: &str = "2025-08_portkey_enabled";
const MAX_TABLE: usize = 512 * 1024;
const MAX_ROW: usize = 32 * 1024;
const MAX_RECORDS: usize = 4096;
const MAX_RULES: usize = 64;
const MAX_IDS: usize = 2048;

fn read<'a, T: Deserialize<'a>>(raw: &'a RawValue) -> Option<T> {
	serde_json::from_str(raw.get()).ok()
}

/// Project the full feature list into a three-state patch. A malformed value cannot preserve
/// a previously exact result, whereas an omitted field on a partial patch can.
pub(crate) fn tier_override<'de, D: serde::Deserializer<'de>>(
	d: D,
) -> Result<Patch<bool>, D::Error> {
	let raw = Box::<RawValue>::deserialize(d)?;
	Ok(
		if raw.get().len() <= 16 * 1024
			&& let Some(features) = read::<Vec<&str>>(&raw)
			&& features.len() <= 256
			&& features.iter().all(|s| !s.is_empty() && s.len() <= 128)
		{
			Patch::Value(features.contains(&"PREMIUM_TIER_3_OVERRIDE"))
		} else {
			Patch::Null
		},
	)
}

pub(crate) enum Portkey<'a> {
	DefaultOff,
	Configured(Rollout<'a>),
}
#[derive(Deserialize)]
struct Override {
	b: i32,
	k: Vec<Id>,
}
type Params<'a> = Vec<(u32, &'a RawValue)>;
struct Population {
	buckets: Vec<(i32, Vec<Range>)>,
	filters: Vec<Filter>,
}
enum Filter {
	Ids(Vec<Id>),
	IdRange { min: Option<Id>, max: Option<Id> },
	Hash { key: u32, limit: u64 },
	Unknown,
}
impl Filter {
	fn matches(&self, guild: Id) -> Option<bool> {
		match self {
			Self::Ids(ids) => Some(ids.binary_search(&guild).is_ok()),
			Self::IdRange { min, max } => {
				Some(min.is_none_or(|id| guild >= id) && max.is_none_or(|id| guild <= id))
			}
			Self::Hash { key, limit } => {
				let hash = u64::from(murmur3(format!("{key}:{guild}").as_bytes()));
				Some((2 * hash) % 10_000 < *limit)
			}
			Self::Unknown => None,
		}
	}
}
#[derive(Deserialize)]
struct Range {
	s: u16,
	e: u16,
}
pub(crate) struct Rollout<'a> {
	key: &'a str,
	overrides: Vec<Override>,
	formatted: Vec<Vec<Population>>,
	populations: Vec<Population>,
	holdout: bool,
	aa: bool,
}

impl<'a> Portkey<'a> {
	pub(crate) fn parse(table: &'a RawValue) -> Option<Self> {
		if table.get().len() > MAX_TABLE {
			return None;
		}
		let records = read::<Vec<&RawValue>>(table)?;
		if records.len() > MAX_RECORDS {
			return None;
		}
		let target = murmur3(NAME.as_bytes());
		let mut found = None;
		for raw in records {
			let fields = read::<Vec<&RawValue>>(raw)?;
			if read::<u32>(fields.first()?)? != target {
				continue;
			}
			if raw.get().len() > MAX_ROW || found.is_some() {
				return None;
			}
			found = Some(Rollout::parse(&fields)?);
		}
		Some(found.map_or(Self::DefaultOff, Self::Configured))
	}
	pub(crate) fn enabled(&self, guild: Id) -> Option<bool> {
		if guild.0 == 0 {
			return None;
		}
		match self {
			Self::DefaultOff => Some(false),
			Self::Configured(rollout) => rollout.enabled(guild),
		}
	}
}
impl<'a> Rollout<'a> {
	fn parse(fields: &[&'a RawValue]) -> Option<Self> {
		let [
			_,
			key,
			revision,
			populations,
			overrides,
			formatted,
			holdout,
			holdout_bucket,
			aa,
			debug,
		] = fields
		else {
			return None;
		};
		read::<u32>(revision)?;
		read::<u8>(debug)?;
		let key = read::<Option<&str>>(key)?.unwrap_or(NAME);
		let holdout = read::<Option<&str>>(holdout)?;
		let holdout_bucket = read::<Option<i32>>(holdout_bucket)?;
		let aa = match read::<u8>(aa)? {
			0 => false,
			1 => true,
			_ => return None,
		};
		let overrides = read::<Option<Vec<Override>>>(overrides)?.unwrap_or_default();
		let formatted = read::<Option<Vec<Vec<&RawValue>>>>(formatted)?.unwrap_or_default();
		let raw_populations = read::<Vec<&RawValue>>(populations)?;
		if key.is_empty()
			|| key.len() > 128
			|| !key.is_ascii()
			|| holdout.is_some() != holdout_bucket.is_some()
			|| overrides.len() > MAX_RULES
			|| formatted.len() > MAX_RULES
			|| overrides.iter().map(|group| group.k.len()).sum::<usize>() > MAX_IDS
			|| formatted.iter().map(Vec::len).sum::<usize>() + raw_populations.len() > MAX_RULES
		{
			return None;
		}
		Some(Self {
			key,
			overrides,
			formatted: formatted
				.into_iter()
				.map(parse_populations)
				.collect::<Option<_>>()?,
			populations: parse_populations(raw_populations)?,
			holdout: holdout.is_some_and(|name| name != NAME),
			aa,
		})
	}
	fn enabled(&self, guild: Id) -> Option<bool> {
		let mut direct = None;
		for entry in &self.overrides {
			if entry.k.contains(&guild)
				&& direct
					.replace(entry.b)
					.is_some_and(|before| before != entry.b)
			{
				return None;
			}
		}
		if let Some(bucket) = direct {
			return known_bucket(bucket);
		}
		let position = (murmur3(format!("{}:{guild}", self.key).as_bytes()) % 10_000) as u16;
		for populations in &self.formatted {
			if let Some(bucket) = population_bucket(populations, guild, position)? {
				return known_bucket(bucket);
			}
		}
		if self.aa {
			return Some(false);
		}
		let enabled =
			known_bucket(population_bucket(&self.populations, guild, position)?.unwrap_or(0))?;
		(!enabled || !self.holdout).then_some(enabled)
	}
}

fn known_bucket(bucket: i32) -> Option<bool> {
	match bucket {
		-1 | 0 => Some(false),
		1 => Some(true),
		_ => None,
	}
}
fn parse_populations(rows: Vec<&RawValue>) -> Option<Vec<Population>> {
	rows.into_iter()
		.map(|raw| {
			let (buckets, filters) = read::<(Vec<(i32, Vec<Range>)>, Params<'_>)>(raw)?;
			if buckets.len() > MAX_RULES
				|| filters.len() > 8
				|| buckets.iter().any(|(_, ranges)| {
					ranges.len() > MAX_RULES
						|| ranges
							.iter()
							.any(|range| range.s >= range.e || range.e > 10_000)
				}) {
				return None;
			}
			let filters = filters
				.into_iter()
				.map(|(key, raw)| parse_filter(key, raw).unwrap_or(Filter::Unknown))
				.collect();
			Some(Population { buckets, filters })
		})
		.collect()
}
fn population_bucket(populations: &[Population], guild: Id, position: u16) -> Option<Option<i32>> {
	for population in populations {
		let mut unknown = false;
		let mut excluded = false;
		for filter in &population.filters {
			match filter.matches(guild) {
				Some(false) => {
					excluded = true;
					break;
				}
				Some(true) => {}
				None => unknown = true,
			}
		}
		if excluded {
			continue;
		}
		if unknown {
			return None;
		}
		let mut matches = population.buckets.iter().filter(|(_, positions)| {
			positions
				.iter()
				.any(|range| range.s <= position && position < range.e)
		});
		let bucket = matches.next().map_or(0, |(bucket, _)| *bucket);
		if matches.any(|(other, _)| *other != bucket) {
			return None;
		}
		// Not eligible stops this group; for formatted overrides, the next group can still match.
		return Some((bucket != -1).then_some(bucket));
	}
	Some(None)
}
fn parse_filter(key: u32, raw: &RawValue) -> Option<Filter> {
	let params = read::<Params<'_>>(raw)?;
	let only = |name: &str| {
		let mut iter = params
			.iter()
			.filter(|(key, _)| *key == murmur3(name.as_bytes()));
		let value = iter.next().map(|(_, raw)| *raw);
		iter.next().is_none().then_some(value)
	};
	if key == murmur3(b"guild_ids") && params.len() == 1 {
		let mut ids = read::<Vec<Id>>(only("guild_ids")??)?;
		if ids.len() > MAX_IDS {
			return None;
		}
		ids.sort_unstable();
		return Some(Filter::Ids(ids));
	}
	if key == murmur3(b"guild_id_range") {
		let min = only("min_id")?
			.map(read::<Option<Id>>)
			.unwrap_or(Some(None))?;
		let max = only("max_id")?
			.map(read::<Option<Id>>)
			.unwrap_or(Some(None))?;
		if params
			.iter()
			.any(|(key, _)| *key != murmur3(b"min_id") && *key != murmur3(b"max_id"))
		{
			return None;
		}
		// The archived client uses inclusive comparison for both bounds.
		return Some(Filter::IdRange { min, max });
	}
	if key == murmur3(b"guild_in_range_by_hash") && params.len() == 2 {
		let hash = only("hash_key")??;
		let hash = hash.get().trim_matches('"');
		if hash.is_empty() || hash.len() > 10 || !hash.bytes().all(|b| b.is_ascii_digit()) {
			return None;
		}
		let target = only("target")??
			.get()
			.trim_matches('"')
			.parse::<u64>()
			.ok()?;
		if target > 10_000 {
			return None;
		}
		let key: u32 = hash.parse().ok()?;
		if hash.chars().len() != key.to_string().len() {
			return None;
		}
		return Some(Filter::Hash { key, limit: target });
	}
	// Unknown mutable inputs never turn into a false "disabled" result.
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{Value, json};
	fn entry(populations: Value, direct: Value, formatted: Value, aa: u8) -> Value {
		json!([
			murmur3(NAME.as_bytes()),
			null,
			0,
			populations,
			direct,
			formatted,
			null,
			null,
			aa,
			0
		])
	}
	fn value(table: Value, guild: Id) -> Option<bool> {
		let raw = RawValue::from_string(table.to_string()).unwrap();
		Portkey::parse(&raw)?.enabled(guild)
	}
	#[test]
	fn resolve_half_open_population_and_explicit_override_priority() {
		let guild = Id(123);
		let pos = murmur3(format!("{NAME}:{guild}").as_bytes()) % 10_000;
		let p = json!([[[[1,[{"s":pos,"e":pos+1}]]],[]]]);
		assert_eq!(
			value(json!([entry(p.clone(), json!([]), json!([]), 0)]), guild),
			Some(true)
		);
		let p2 = json!([[[[1,[{"s":pos+1,"e":10_000}]]],[]]]);
		assert_eq!(
			value(json!([entry(p2, json!([]), json!([]), 0)]), guild),
			Some(false)
		);
		let formatted = json!([p.clone()]);
		assert_eq!(
			value(
				json!([entry(json!([]), json!([]), formatted.clone(), 1)]),
				guild
			),
			Some(true)
		);
		assert_eq!(
			value(
				json!([entry(p.clone(), json!([{"b":0,"k":["123"]}]), formatted, 0)]),
				guild
			),
			Some(false)
		);
		assert_eq!(
			value(json!([entry(p, json!([]), json!([]), 1)]), guild),
			Some(false)
		);
	}
	#[test]
	fn unknown_or_conflicting_inputs_never_get_a_number() {
		let guild = Id(123);
		let p = json!([[[[1,[{"s":0,"e":10000}]]],[[456,[]]]]]);
		let row = entry(p, json!([]), json!([]), 0);
		assert_eq!(value(json!([row.clone()]), guild), None);
		assert_eq!(value(json!([row.clone(), row]), guild), None);
		assert_eq!(value(json!([]), guild), Some(false));
		assert_eq!(value(json!(null), guild), None);
		let too_many = json!(vec![json!([0]); MAX_RECORDS + 1]);
		assert_eq!(value(too_many, guild), None);
	}
}
