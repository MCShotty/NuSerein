//! Session-local REST penalties. Route limits never become account-wide by accident.
use super::{Failure, Instant, Method};
use std::{
	collections::BTreeMap,
	hash::{Hash, Hasher},
	time::Duration,
};

const ENTRIES: usize = 256;
const BYTES: usize = 64 * 1024;
const RETAIN: Duration = Duration::from_secs(60);

pub(super) struct Route {
	key: String,
	major: String,
}
impl Route {
	pub fn new(method: &Method, path: &str) -> Self {
		let mut key = method.as_str().to_owned();
		let mut major = String::new();
		let mut previous = "";
		for (index, part) in path
			.split('?')
			.next()
			.unwrap_or("")
			.split('/')
			.skip(1)
			.enumerate()
		{
			key.push('/');
			if matches!(previous, "channels" | "guilds" | "webhooks")
				&& !part.is_empty()
				&& part.bytes().all(|b| b.is_ascii_digit())
			{
				major = format!("{previous}/{part}");
				key.push_str(part);
			} else if major.starts_with("webhooks/") && index == 2 {
				let mut hash = std::hash::DefaultHasher::new();
				part.hash(&mut hash);
				let fingerprint = format!(":token-{:016x}", hash.finish());
				major.push_str(&fingerprint);
				key.push_str(&fingerprint);
			} else if part.bytes().all(|b| b.is_ascii_digit()) || previous == "reactions" {
				key.push_str(":parameter");
			} else {
				key.extend(part.chars().take(64));
			}
			previous = part;
		}
		Self { key, major }
	}
}
struct Entry {
	bucket: Option<String>,
	deadline: Instant,
	expires: Instant,
}
pub(super) struct Limits {
	pub global: Instant,
	routes: BTreeMap<String, Entry>,
}
impl Default for Limits {
	fn default() -> Self {
		Self {
			global: Instant::now(),
			routes: BTreeMap::new(),
		}
	}
}
impl Limits {
	fn bytes(&self) -> usize {
		self.routes
			.keys()
			.map(|key| {
				key.capacity()
					+ 256 // Reserve bucket identity space before a request is sent.
					+ size_of::<Entry>()
					+ size_of::<String>()
					+ 64
			})
			.sum()
	}
	pub fn deadline(&mut self, route: &Route) -> Result<Instant, Failure> {
		let now = Instant::now();
		self.routes
			.retain(|_, entry| entry.expires > now || entry.deadline > now);
		if !self.routes.contains_key(&route.key) {
			if self.routes.len() >= ENTRIES || self.bytes() + route.key.len() + 512 > BYTES {
				return Err(Failure::Busy);
			}
			// A response can arrive after other requests have filled the table. Reserve
			// its route and maximum bucket space before transmission, so its debt fits.
			self.routes.insert(
				route.key.clone(),
				Entry {
					bucket: None,
					deadline: now,
					expires: now + RETAIN,
				},
			);
		}
		let current = self.routes.get(&route.key);
		let mut deadline = self.global.max(current.map_or(self.global, |e| e.deadline));
		if let Some(bucket) = current.and_then(|e| e.bucket.as_ref()) {
			for entry in self
				.routes
				.values()
				.filter(|e| e.bucket.as_ref() == Some(bucket))
			{
				deadline = deadline.max(entry.deadline);
			}
		}
		Ok(deadline)
	}
	pub fn observe(
		&mut self,
		route: &Route,
		bucket: Option<&str>,
		delay: Option<Duration>,
		global: bool,
	) -> Result<(), Failure> {
		let now = Instant::now();
		if global {
			self.global = self.global.max(now + delay.unwrap_or_default());
			return Ok(());
		}
		let bucket = bucket
			.filter(|v| !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_graphic()))
			.map(|v| format!("{}/{v}", route.major));
		self.deadline(route)?;
		let entry = self.routes.entry(route.key.clone()).or_insert(Entry {
			bucket: None,
			deadline: now,
			expires: now + RETAIN,
		});
		if bucket.is_some() {
			entry.bucket = bucket;
		}
		entry.deadline = entry.deadline.max(now + delay.unwrap_or_default());
		entry.expires = entry.deadline.max(now + RETAIN);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn buckets_share_only_their_major_resource_and_global_limits_remain_global() {
		let mut limits = Limits::default();
		let a = Route::new(&Method::GET, "/channels/1/messages?limit=50");
		let b = Route::new(&Method::POST, "/channels/1/messages");
		let other = Route::new(&Method::GET, "/channels/2/messages");
		limits.observe(&a, Some("shared"), None, false).unwrap();
		limits
			.observe(&b, Some("shared"), Some(Duration::from_secs(30)), false)
			.unwrap();
		assert!(limits.deadline(&a).unwrap() > Instant::now() + Duration::from_secs(29));
		assert!(limits.deadline(&other).unwrap() <= Instant::now());
		// Guild IDs also scope nested normal-account and application routes.
		for path in [
			"/users/@me/guilds/1/settings",
			"/applications/5/guilds/1/commands/9/permissions",
		] {
			let nested = Route::new(&Method::GET, path);
			let independent = Route::new(&Method::GET, &path.replace("guilds/1/", "guilds/2/"));
			limits
				.observe(
					&nested,
					Some("nested"),
					Some(Duration::from_secs(30)),
					false,
				)
				.unwrap();
			assert!(limits.deadline(&nested).unwrap() > Instant::now() + Duration::from_secs(29));
			assert!(limits.deadline(&independent).unwrap() <= Instant::now());
		}
		limits
			.observe(&a, None, Some(Duration::from_secs(60)), true)
			.unwrap();
		assert!(limits.deadline(&other).unwrap() > Instant::now() + Duration::from_secs(59));
	}
	#[test]
	fn capacity_preserves_unexpired_penalties_and_route_keys_omit_tokens() {
		let mut limits = Limits::default();
		let protected = Route::new(&Method::GET, "/channels/1/messages");
		limits
			.observe(
				&protected,
				Some("shared"),
				Some(Duration::from_secs(60)),
				false,
			)
			.unwrap();
		for n in 2..1000 {
			let _ = limits.observe(
				&Route::new(&Method::GET, &format!("/channels/{n}/messages")),
				Some("bucket"),
				Some(Duration::from_secs(60)),
				false,
			);
		}
		assert!(limits.routes.len() <= ENTRIES && limits.bytes() <= BYTES);
		assert!(limits.deadline(&protected).unwrap() > Instant::now() + Duration::from_secs(59));
		assert!(
			!Route::new(
				&Method::GET,
				"/webhooks/1/SYNTHETIC_SECRET?token=SYNTHETIC_SECRET"
			)
			.key
			.contains("SYNTHETIC_SECRET")
		);
	}
}
