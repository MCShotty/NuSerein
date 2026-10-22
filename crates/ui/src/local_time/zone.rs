//! One cached timezone, with one active and one replacement resolution request.
//! Native discovery runs in the host's blocking worker; only begin_frame publishes its result.
use jiff::tz::{TimeZone, TimeZoneDatabase};
use model::TimeZonePreference;
use std::sync::{Mutex, MutexGuard, OnceLock, atomic::Ordering};

struct Snapshot {
	zone: TimeZone,
	preference: TimeZonePreference,
	status: Option<&'static str>,
}
#[derive(Clone)]
struct FrameSnapshot {
	zone: TimeZone,
	utc: time::OffsetDateTime,
	local: time::OffsetDateTime,
	status: Option<&'static str>,
}
impl Default for FrameSnapshot {
	fn default() -> Self {
		let now = time::OffsetDateTime::now_utc();
		Self {
			zone: TimeZone::UTC,
			utc: now,
			local: now,
			status: None,
		}
	}
}
thread_local! {
	static FRAME: std::cell::RefCell<FrameSnapshot> = std::cell::RefCell::new(FrameSnapshot::default());
}
impl Default for Snapshot {
	fn default() -> Self {
		Self {
			zone: TimeZone::UTC,
			preference: TimeZonePreference::System,
			status: None,
		}
	}
}
#[derive(Default)]
struct Service {
	desired: TimeZonePreference,
	generation: u64,
	initialized: bool,
	running: bool,
	queued: Option<TimezoneRequest>,
	pending: Option<(u64, Snapshot)>,
	active: Snapshot,
}
fn service() -> MutexGuard<'static, Service> {
	static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
	SERVICE
		.get_or_init(Mutex::default)
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Queue a changed preference. No filesystem, native lookup or parsing occurs in rendering.
pub fn set_timezone(preference: &TimeZonePreference) {
	let preference = if preference.is_valid() {
		preference.clone()
	} else {
		TimeZonePreference::System
	};
	let mut state = service();
	if state.initialized && state.desired == preference {
		return;
	}
	state.initialized = true;
	state.desired = preference.clone();
	state.generation = state.generation.wrapping_add(1);
	state.pending = None;
	state.queued = Some(TimezoneRequest {
		generation: state.generation,
		preference,
	});
}

/// Queue discovery on focus/resume or foreground maintenance, only while following System.
pub fn refresh_system_timezone() {
	let mut state = service();
	if state.desired != TimeZonePreference::System || state.running || state.queued.is_some() {
		return;
	}
	state.initialized = true;
	state.generation = state.generation.wrapping_add(1);
	state.pending = None;
	state.queued = Some(TimezoneRequest {
		generation: state.generation,
		preference: TimeZonePreference::System,
	});
}

/// A bounded job for the application's existing blocking runtime.
pub struct TimezoneRequest {
	generation: u64,
	preference: TimeZonePreference,
}

/// At most one resolution runs; rapid settings changes replace the one waiting job.
pub fn take_timezone_request() -> Option<TimezoneRequest> {
	let mut state = service();
	if state.running {
		return None;
	}
	let request = state.queued.take()?;
	state.running = true;
	Some(request)
}
impl TimezoneRequest {
	/// Run outside the UI thread, then request repaint so begin_frame can publish the result.
	pub fn resolve(self) {
		// Prepare the bounded region search index before its picker is first painted.
		let _ = names();
		let result = resolve(&self.preference, || {
			let name = iana_time_zone::get_timezone().map_err(|_| ())?;
			lookup_zone(&name)
		});
		let mut state = service();
		state.running = false;
		if self.generation == state.generation {
			state.pending = Some((self.generation, result));
		}
	}
}

/// Publish only between frames. Returns a recovered preference for persistence when necessary.
pub fn begin_frame() -> Option<TimeZonePreference> {
	let mut state = service();
	let mut recovery = None;
	if let Some((generation, result)) = state.pending.take()
		&& generation == state.generation
	{
		recovery = (state.desired != result.preference).then(|| result.preference.clone());
		state.desired = result.preference.clone();
		if state.active.zone != result.zone || state.active.preference != result.preference {
			super::FORMAT_REVISION.fetch_add(1, Ordering::Relaxed);
		}
		state.active = result;
	}
	FRAME.with(|frame| {
		let utc = time::OffsetDateTime::now_utc();
		let local = convert(&state.active.zone, utc);
		#[cfg(not(test))]
		if frame.borrow().local.date() != local.date() {
			super::FORMAT_REVISION.fetch_add(1, Ordering::Relaxed);
		}
		*frame.borrow_mut() = FrameSnapshot {
			zone: state.active.zone.clone(),
			utc,
			local,
			status: if state.running || state.queued.is_some() {
				Some("timezone-updating")
			} else {
				state.active.status
			},
		}
	});
	recovery
}

fn resolve(
	preference: &TimeZonePreference,
	system: impl FnOnce() -> Result<TimeZone, ()>,
) -> Snapshot {
	match preference {
		TimeZonePreference::Named(name) => {
			if let Ok(zone) = lookup_zone(name) {
				return Snapshot {
					zone,
					preference: preference.clone(),
					status: None,
				};
			}
			let mut recovered = resolve(&TimeZonePreference::System, system);
			if recovered.status.is_none() {
				recovered.status = Some("timezone-unknown-recovered");
			}
			recovered
		}
		TimeZonePreference::FixedOffset(minutes) => {
			let seconds = i32::from(*minutes) * 60;
			let zone = jiff::tz::Offset::from_seconds(seconds)
				.map(TimeZone::fixed)
				.unwrap_or(TimeZone::UTC);
			Snapshot {
				zone,
				preference: preference.clone(),
				status: None,
			}
		}
		TimeZonePreference::System => match system().ok().filter(|zone| !zone.is_unknown()) {
			Some(zone) => Snapshot {
				zone,
				..Default::default()
			},
			None => Snapshot {
				status: Some("timezone-system-unavailable"),
				..Default::default()
			},
		},
	}
}

pub fn timezone_status() -> Option<&'static str> {
	FRAME.with(|frame| frame.borrow().status)
}

pub fn timezone_label() -> String {
	let zone = display_zone();
	let offset = zone.to_offset(
		jiff::Timestamp::from_second(frame_now().unix_timestamp()).expect("current timestamp"),
	);
	let label = TimeZonePreference::fixed_label((offset.seconds() / 60) as i16);
	if let Some(name) = zone.iana_name() {
		format!("{name} · {label}")
	} else {
		label
	}
}

pub(super) fn display_zone() -> TimeZone {
	#[cfg(test)]
	{
		TEST_ZONE.with(|zone| zone.borrow().clone().unwrap_or(TimeZone::UTC))
	}
	#[cfg(not(test))]
	{
		FRAME.with(|frame| frame.borrow().zone.clone())
	}
}

pub(super) fn frame_now() -> time::OffsetDateTime {
	FRAME.with(|frame| frame.borrow().utc)
}

#[cfg(not(test))]
pub(super) fn local_now() -> time::OffsetDateTime {
	FRAME.with(|frame| frame.borrow().local)
}

/// The next minute boundary is the next change to the settings' seconds-free example.
pub fn minute_deadline(at: time::OffsetDateTime) -> std::time::Duration {
	let remaining = 60_000_000_000_i128 - at.unix_timestamp_nanos().rem_euclid(60_000_000_000);
	std::time::Duration::from_nanos(remaining as u64)
}

/// Resolve local input without guessing across a daylight-saving gap or fold.
pub fn wall_time(value: &str) -> Vec<time::OffsetDateTime> {
	if value.len() != 16 {
		return Vec::new();
	}
	let Ok(civil) = value.replace(' ', "T").parse::<jiff::civil::DateTime>() else {
		return Vec::new();
	};
	let zone = display_zone();
	let ambiguous = zone.to_ambiguous_zoned(civil);
	let mut result = Vec::with_capacity(2);
	for resolved in [ambiguous.clone().earlier(), ambiguous.later()]
		.into_iter()
		.flatten()
	{
		if resolved.datetime() != civil {
			continue;
		}
		if let Ok(utc) =
			time::OffsetDateTime::from_unix_timestamp_nanos(resolved.timestamp().as_nanosecond())
		{
			let at = convert(&zone, utc);
			if !result.contains(&at) {
				result.push(at);
			}
		}
	}
	result
}

pub(super) fn convert(zone: &TimeZone, instant: time::OffsetDateTime) -> time::OffsetDateTime {
	let offset = jiff::Timestamp::from_second(instant.unix_timestamp())
		.ok()
		.and_then(|timestamp| {
			time::UtcOffset::from_whole_seconds(zone.to_offset(timestamp).seconds()).ok()
		})
		.unwrap_or(time::UtcOffset::UTC);
	instant.checked_to_offset(offset).unwrap_or(instant)
}

fn database() -> &'static TimeZoneDatabase {
	static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
	DATABASE.get_or_init(TimeZoneDatabase::bundled)
}

fn lookup_zone(name: &str) -> Result<TimeZone, ()> {
	let result = database().get(name).map_err(|_| ());
	// Jiff's bundled database otherwise retains every parsed zone indefinitely. The app
	// retains only its active/pending snapshots, not a second history of settings choices.
	database().reset();
	result.and_then(|zone| {
		(zone.memory_usage() <= 1024 * 1024)
			.then_some(zone)
			.ok_or(())
	})
}

/// Stable, bounded search index, with lowercase keys allocated only once.
pub(crate) fn names() -> &'static [(String, String)] {
	static NAMES: OnceLock<Vec<(String, String)>> = OnceLock::new();
	NAMES.get_or_init(|| {
		let mut names: Vec<_> = database()
			.available()
			.take(4096)
			.map(|name| {
				let name = name.to_string();
				let query = name.replace('_', " ").to_ascii_lowercase();
				(name, query)
			})
			.collect();
		names.sort_unstable_by(|left, right| left.0.cmp(&right.0));
		names
	})
}

#[cfg(test)]
thread_local! {
	static TEST_ZONE: std::cell::RefCell<Option<TimeZone>> = const { std::cell::RefCell::new(None) };
	static TEST_REVISION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn test_revision() -> u32 {
	TEST_REVISION.get()
}

/// A thread-local zone for production-path rendering tests. Other tests keep deterministic UTC.
#[cfg(test)]
pub(crate) fn pin_timezone(name: &str) -> impl Drop {
	struct Restore(Option<TimeZone>);
	impl Drop for Restore {
		fn drop(&mut self) {
			TEST_ZONE.with(|zone| *zone.borrow_mut() = self.0.take());
			TEST_REVISION.set(TEST_REVISION.get().wrapping_add(1));
		}
	}
	let zone = lookup_zone(name).expect("known synthetic timezone");
	let previous = TEST_ZONE.with(|current| current.replace(Some(zone)));
	TEST_REVISION.set(TEST_REVISION.get().wrapping_add(1));
	Restore(previous)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn settings_deadline_tracks_the_next_displayed_minute_without_polling() {
		assert_eq!(
			minute_deadline(instant("2026-10-22T12:34:00Z")),
			std::time::Duration::from_secs(60)
		);
		assert_eq!(
			minute_deadline(instant("2026-10-22T12:34:59.875Z")),
			std::time::Duration::from_millis(125)
		);
		assert_eq!(
			minute_deadline(instant("1969-12-31T23:59:59.999Z")),
			std::time::Duration::from_millis(1)
		);
	}

	#[test]
	fn reminder_wall_times_distinguish_dst_gaps_and_folds() {
		let _zone = pin_timezone("Europe/London");
		assert!(wall_time("2026-03-29 01:30").is_empty());
		let fold = wall_time("2026-10-25 01:30");
		assert_eq!(fold.len(), 2);
		assert_eq!((fold[1] - fold[0]).whole_hours(), 1);
		assert_eq!(wall_time("2026-10-25 12:30").len(), 1);
		assert!(wall_time("invalid time").is_empty());
	}

	#[test]
	fn frame_clock_is_stable_between_paints_and_is_retired_at_the_boundary() {
		let old = FRAME.with(|frame| frame.borrow().clone());
		let at = instant("2026-10-22T23:59:59Z");
		FRAME.with(|frame| {
			let mut frame = frame.borrow_mut();
			frame.utc = at;
			frame.local = convert(&frame.zone, at);
		});
		assert_eq!(
			FRAME.with(|frame| frame.borrow().local.unix_timestamp()),
			at.unix_timestamp()
		);
		assert_eq!(frame_now(), at);
		assert_eq!(frame_now(), frame_now());
		FRAME.with(|frame| *frame.borrow_mut() = old);
	}

	fn instant(value: &str) -> time::OffsetDateTime {
		time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).unwrap()
	}

	#[test]
	fn named_zones_follow_each_instants_dst_offset_without_changing_the_instant() {
		let cases = [
			("Europe/London", "2026-03-29T00:59:59Z", 0, 0),
			("Europe/London", "2026-03-29T01:00:00Z", 3600, 2),
			("Europe/London", "2026-10-25T00:59:59Z", 3600, 1),
			("Europe/London", "2026-10-25T01:00:00Z", 0, 1),
			("Australia/Sydney", "2026-01-01T00:00:00Z", 39600, 11),
			("Australia/Sydney", "2026-07-01T00:00:00Z", 36000, 10),
			("Asia/Kathmandu", "2026-01-01T00:00:00Z", 20700, 5),
			("Asia/Kathmandu", "1980-01-01T00:00:00Z", 19800, 5),
			("America/St_Johns", "2026-01-01T12:00:00Z", -12600, 8),
		];
		for (name, value, offset, hour) in cases {
			let utc = instant(value);
			let converted = convert(&TimeZone::get(name).unwrap(), utc);
			assert_eq!(converted.offset().whole_seconds(), offset, "{name} {value}");
			assert_eq!(converted.hour(), hour);
			assert_eq!(converted.unix_timestamp_nanos(), utc.unix_timestamp_nanos());
		}
	}

	#[test]
	fn fixed_offsets_do_not_follow_dst_and_zone_selection_changes_calendar_boundaries() {
		let fixed = resolve(&TimeZonePreference::FixedOffset(345), || {
			panic!("fixed zones never query the OS")
		});
		for value in ["2026-01-01T20:00:00Z", "2026-07-01T20:00:00Z"] {
			let utc = instant(value);
			let at = convert(&fixed.zone, utc);
			assert_eq!((at.hour(), at.minute()), (1, 45));
			assert_eq!(at.date(), utc.date().next_day().unwrap());
		}
	}

	#[test]
	fn calendar_headers_use_the_selected_zones_day_and_preserve_explicit_timestamp_instants() {
		let _zone = pin_timezone("Asia/Riyadh");
		let utc = instant("2026-12-31T21:30:00Z");
		let _now = super::super::pin_now(utc);
		assert_eq!(super::super::local(utc).date().to_string(), "2027-01-01");
		assert!(super::super::calendar(utc).starts_with("Today at "));
		assert!(super::super::header(instant("2026-12-31T20:00:00Z")).starts_with("Yesterday at "));
		assert_eq!(
			super::super::local(utc).unix_timestamp_nanos(),
			utc.unix_timestamp_nanos()
		);
	}

	#[test]
	fn timezone_test_overrides_invalidate_only_the_calling_threads_layout_revision() {
		let before = test_revision();
		let revision = super::super::format_revision();
		{
			let _zone = pin_timezone("Asia/Riyadh");
			assert_ne!(super::super::format_revision(), revision);
			assert_eq!(std::thread::spawn(test_revision).join().unwrap(), 0);
		}
		assert_eq!(test_revision(), before + 2);
		assert_eq!(display_zone(), TimeZone::UTC);
	}

	#[test]
	fn display_conversion_near_supported_date_edges_never_panics() {
		for value in ["9999-12-31T23:59:59Z", "-9999-01-01T00:00:00Z"] {
			let Ok(utc) =
				time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
			else {
				continue;
			};
			for minutes in [-840, 840] {
				let zone = resolve(&TimeZonePreference::FixedOffset(minutes), || Err(()));
				assert_eq!(
					convert(&zone.zone, utc).unix_timestamp_nanos(),
					utc.unix_timestamp_nanos()
				);
			}
		}
	}

	#[test]
	fn unknown_names_recover_to_system_and_failed_system_lookup_is_explicit_utc() {
		let requested = TimeZonePreference::Named("Never/Exists".into());
		let recovered = resolve(&requested, || Ok(TimeZone::get("Asia/Riyadh").unwrap()));
		assert_eq!(recovered.preference, TimeZonePreference::System);
		assert_eq!(recovered.status, Some("timezone-unknown-recovered"));
		assert_eq!(recovered.zone.iana_name(), Some("Asia/Riyadh"));
		let unavailable = resolve(&TimeZonePreference::System, || Err(()));
		assert_eq!(unavailable.zone, TimeZone::UTC);
		assert_eq!(unavailable.status, Some("timezone-system-unavailable"));
		let both_unavailable = resolve(&requested, || Err(()));
		assert_eq!(both_unavailable.preference, TimeZonePreference::System);
		assert_eq!(both_unavailable.status, Some("timezone-system-unavailable"));
	}

	#[test]
	fn search_index_is_bounded_and_contains_common_regions() {
		assert!(names().len() <= 4096);
		for name in ["Europe/London", "Asia/Riyadh", "Asia/Kathmandu"] {
			assert!(names().iter().any(|entry| entry.0 == name));
		}
	}
}
