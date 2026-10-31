//! One process-wide disk ceiling for history, presentation hints and downloaded artwork.
//! All filesystem work belongs on storage/image workers, never the rendering thread.
use std::{
	collections::{BTreeMap, BinaryHeap},
	fs, io,
	path::{Path, PathBuf},
	sync::{Arc, LazyLock, Mutex, Weak},
	time::{Duration, Instant, SystemTime},
};

const MAX_SCAN_ENTRIES: usize = 200_000;
const MAX_PROTECTED: usize = 64;
const MAX_ROOTS: usize = 64;
const CANDIDATES: usize = 128;
const RECENT_FILES: usize = 256;
const RECENT_BYTES: usize = 256 * 1024;
const RECENT_GRACE: Duration = Duration::from_secs(60);
/// Kept free for byte-bounded SQLite delete/checkpoint/vacuum transactions.
pub const CLEANUP_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;

/// Work only the database owner can perform, in eviction-priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclaimStage {
	ExpiredHistory,
	RecentHistory,
	PresentationHints,
}

/// Cached measurements; reading this snapshot performs no filesystem access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheUsage {
	pub database: u64,
	pub media: u64,
	pub artwork: u64,
	pub temporary: u64,
	pub reserved: u64,
	pub total: u64,
	pub pressure: bool,
	pub last_cleanup_reclaimed: u64,
}

#[derive(Clone, Copy)]
struct Limits {
	ceiling: u64,
	headroom: u64,
	start: u64,
	target: u64,
}
impl Default for Limits {
	fn default() -> Self {
		Self {
			ceiling: model::TOTAL_CACHE_BYTES,
			headroom: CLEANUP_HEADROOM_BYTES,
			start: model::CACHE_CLEANUP_START_BYTES,
			target: model::CACHE_CLEANUP_TARGET_BYTES,
		}
	}
}
struct State {
	usage: CacheUsage,
	measured: bool,
	inventory: u64,
	reserved: u64,
	protected: BTreeMap<PathBuf, usize>,
	recent: BTreeMap<PathBuf, Instant>,
	recent_bytes: usize,
	active: Option<String>,
	retention: Duration,
}
impl Default for State {
	fn default() -> Self {
		Self {
			usage: CacheUsage::default(),
			measured: false,
			inventory: 0,
			reserved: 0,
			protected: BTreeMap::new(),
			recent: BTreeMap::new(),
			recent_bytes: 0,
			active: None,
			retention: Duration::from_secs(90 * 86_400),
		}
	}
}
pub struct CacheBudget {
	root: PathBuf,
	limits: Limits,
	state: Mutex<State>,
}

static ROOTS: LazyLock<Mutex<BTreeMap<PathBuf, Weak<CacheBudget>>>> =
	LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Stays charged until a cache write has committed or its temporary file is removed.
pub struct CacheReservation {
	budget: Arc<CacheBudget>,
	bytes: u64,
	inventory: u64,
}
impl CacheReservation {
	/// Commit the image worker's known file replacement without rescanning every image.
	/// Call after the atomic rename succeeds. Consumes the reservation so its delta
	/// cannot accidentally be committed twice.
	pub fn commit_file(self, previous: u64, written: u64, artwork: bool) -> io::Result<()> {
		let mut state = self.budget.state.lock().unwrap_or_else(|e| e.into_inner());
		if written > self.bytes {
			state.measured = false;
			return Err(io::ErrorKind::InvalidInput.into());
		}
		if state.inventory != self.inventory {
			// A maintenance/clear scan may already include this renamed replacement.
			// Applying its old→new delta again can undercount unrelated files when it
			// shrank. Reconcile the actual inventory instead of guessing its old charge.
			self.budget.measure(&mut state)?;
			return Ok(());
		}
		let bytes = if artwork {
			&mut state.usage.artwork
		} else {
			&mut state.usage.media
		};
		*bytes = bytes.saturating_sub(previous).saturating_add(written);
		update_total(&mut state);
		Ok(())
	}
}
impl Drop for CacheReservation {
	fn drop(&mut self) {
		let mut state = self.budget.state.lock().unwrap_or_else(|e| e.into_inner());
		state.reserved = state.reserved.saturating_sub(self.bytes);
		update_total(&mut state);
		// The next worker admission measures committed files; no IO occurs in Drop.
	}
}

/// Prevents global eviction while an encoded cache file is being read.
pub struct CacheFileGuard {
	budget: Arc<CacheBudget>,
	path: PathBuf,
}
impl Drop for CacheFileGuard {
	fn drop(&mut self) {
		let mut state = self.budget.state.lock().unwrap_or_else(|e| e.into_inner());
		if let Some(readers) = state.protected.get_mut(&self.path) {
			*readers -= 1;
			if *readers == 0 {
				state.protected.remove(&self.path);
			}
		}
	}
}

impl CacheBudget {
	/// Uses the same absolute data directory as `client.sqlite3`; does not scan it.
	pub fn for_root(root: &Path) -> Arc<Self> {
		let mut roots = ROOTS.lock().unwrap_or_else(|e| e.into_inner());
		roots.retain(|_, budget| budget.strong_count() != 0);
		if let Some(budget) = roots.get(root).and_then(Weak::upgrade) {
			return budget;
		}
		let limits = if roots.len() < MAX_ROOTS && root.is_absolute() {
			Limits::default()
		} else {
			// Misconfiguration must not silently create an independent unaccounted writer.
			Limits {
				ceiling: 0,
				headroom: 0,
				start: 0,
				target: 0,
			}
		};
		let budget = Arc::new(Self {
			root: root.to_owned(),
			limits,
			state: Mutex::new(State::default()),
		});
		if limits.ceiling != 0 {
			roots.insert(root.to_owned(), Arc::downgrade(&budget));
		}
		budget
	}
	pub fn usage(&self) -> CacheUsage {
		self.state.lock().unwrap_or_else(|e| e.into_inner()).usage
	}
	pub fn needs_maintenance(&self) -> bool {
		self.usage().pressure
	}
	pub fn set_active_account(&self, account: Option<model::Id>) {
		self.state.lock().unwrap_or_else(|e| e.into_inner()).active =
			account.map(|id| id.to_string());
	}
	pub fn set_retention(&self, retention: Duration) {
		self.state
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.retention = retention;
	}
	pub fn protect(self: &Arc<Self>, path: &Path) -> io::Result<CacheFileGuard> {
		if !path.starts_with(self.root.join("avatars")) {
			return Err(io::ErrorKind::InvalidInput.into());
		}
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		if state.protected.len() >= MAX_PROTECTED && !state.protected.contains_key(path) {
			return Err(io::ErrorKind::WouldBlock.into());
		}
		*state.protected.entry(path.to_owned()).or_default() += 1;
		Ok(CacheFileGuard {
			budget: self.clone(),
			path: path.to_owned(),
		})
	}
	pub fn is_protected(&self, path: &Path) -> bool {
		let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		file_is_protected(&state, path)
	}
	/// A successful worker read/write protects its encoded source briefly, so cache
	/// pressure cannot immediately force the same fresh card to download again.
	/// Uses no filesystem IO. Retains at most 256 paths / 256 KiB across all accounts.
	pub fn record_file_use(&self, path: &Path) {
		if !path.starts_with(self.root.join("avatars")) {
			return;
		}
		let charge = recent_charge(path);
		if charge > RECENT_BYTES {
			return;
		}
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		if let Some((old, _)) = state.recent.remove_entry(path) {
			state.recent_bytes = state.recent_bytes.saturating_sub(recent_charge(&old));
		}
		while let Some((old, at)) = state.recent.iter().min_by_key(|(_, at)| *at) {
			if at.elapsed() < RECENT_GRACE
				&& state.recent.len() < RECENT_FILES
				&& state.recent_bytes.saturating_add(charge) <= RECENT_BYTES
			{
				break;
			}
			let old = old.clone();
			state.recent.remove(&old);
			state.recent_bytes = state.recent_bytes.saturating_sub(recent_charge(&old));
		}
		state.recent.insert(path.to_owned(), Instant::now());
		state.recent_bytes += charge;
	}
	/// Worker-only remeasurement. Directory traversal never follows symlinks.
	pub fn refresh(&self) -> io::Result<CacheUsage> {
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		self.measure(&mut state)?;
		Ok(state.usage)
	}
	/// Fast worker-only snapshot after a SQLite transaction. Image commits maintain
	/// their own inventory; do not rescan the artwork tree for every progress update.
	pub fn refresh_database(&self) -> io::Result<CacheUsage> {
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		self.measure_admission(&mut state)?;
		Ok(state.usage)
	}
	/// Image-worker admission. Database reclamation is requested through cached pressure.
	pub fn reserve(self: &Arc<Self>, bytes: u64) -> io::Result<CacheReservation> {
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		self.measure_admission(&mut state)?;
		self.admit(&mut state, bytes)
	}
	/// Storage-worker admission with priority-ordered database reclamation.
	/// The callback receives bytes to reclaim, and must not call back into this budget
	/// or acquire an image-worker lock.
	pub fn reserve_with_reclaim(
		self: &Arc<Self>,
		bytes: u64,
		mut reclaim: impl FnMut(ReclaimStage, u64) -> io::Result<()>,
	) -> io::Result<CacheReservation> {
		let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
		self.measure_admission(&mut state)?;
		if state.usage.total.saturating_add(bytes) >= self.limits.start || state.usage.pressure {
			self.cleanup(&mut state, bytes, &mut reclaim)?;
		}
		self.admit(&mut state, bytes)
	}
	/// Explicit startup/pressure maintenance, run by the serialized storage owner.
	pub fn maintain(
		self: &Arc<Self>,
		reclaim: impl FnMut(ReclaimStage, u64) -> io::Result<()>,
	) -> io::Result<CacheUsage> {
		let reservation = self.reserve_with_reclaim(0, reclaim)?;
		drop(reservation);
		Ok(self.usage())
	}
	fn admit(self: &Arc<Self>, state: &mut State, bytes: u64) -> io::Result<CacheReservation> {
		let total = state
			.usage
			.total
			.checked_add(bytes)
			.ok_or(io::ErrorKind::StorageFull)?;
		state.usage.pressure |= total >= self.limits.start;
		if self.limits.ceiling == 0
			|| total > self.limits.ceiling.saturating_sub(self.limits.headroom)
		{
			state.usage.pressure = true;
			return Err(io::ErrorKind::StorageFull.into());
		}
		state.reserved += bytes;
		state.usage.reserved = state.reserved;
		state.usage.total = total;
		Ok(CacheReservation {
			budget: self.clone(),
			bytes,
			inventory: state.inventory,
		})
	}
	fn database_bytes(&self) -> io::Result<u64> {
		let mut bytes = 0u64;
		for name in [
			"client.sqlite3",
			"client.sqlite3-wal",
			"client.sqlite3-shm",
			"client.sqlite3-journal",
		] {
			match fs::symlink_metadata(self.root.join(name)) {
				Ok(metadata) if metadata.file_type().is_file() => {
					bytes = bytes.saturating_add(metadata.len());
				}
				Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
				Err(error) if error.kind() == io::ErrorKind::NotFound => {}
				Err(error) => return Err(error),
			}
		}
		Ok(bytes)
	}
	fn measure_admission(&self, state: &mut State) -> io::Result<()> {
		if !state.measured {
			return self.measure(state);
		}
		// SQLite changes pages and its WAL during a transaction. Four metadata reads
		// account for that without traversing the image tree for every history page.
		state.usage.database = self.database_bytes()?;
		update_total(state);
		state.usage.pressure |= state.usage.total >= self.limits.start;
		Ok(())
	}
	fn measure(&self, state: &mut State) -> io::Result<()> {
		// A failed scan cannot leave an old successful inventory admitting new files.
		state.measured = false;
		let mut usage = CacheUsage {
			database: self.database_bytes()?,
			..CacheUsage::default()
		};
		self.visit_files(|path, metadata, artwork, _| {
			let bytes = if path.extension().is_some_and(|ext| ext == "part") {
				&mut usage.temporary
			} else if artwork {
				&mut usage.artwork
			} else {
				&mut usage.media
			};
			*bytes = bytes.saturating_add(metadata.len());
			Ok(())
		})?;
		usage.reserved = state.reserved;
		usage.total = usage
			.database
			.saturating_add(usage.media)
			.saturating_add(usage.artwork)
			.saturating_add(usage.temporary)
			.saturating_add(usage.reserved);
		usage.pressure = usage.total >= self.limits.start
			|| state.usage.pressure && usage.total > self.limits.target;
		usage.last_cleanup_reclaimed = state.usage.last_cleanup_reclaimed;
		state.usage = usage;
		state.measured = true;
		state.inventory = state.inventory.wrapping_add(1);
		Ok(())
	}
	fn cleanup(
		&self,
		state: &mut State,
		reserve: u64,
		reclaim: &mut impl FnMut(ReclaimStage, u64) -> io::Result<()>,
	) -> io::Result<()> {
		self.measure(state)?;
		let before = state.usage.total;
		let target = self.limits.target.saturating_sub(reserve);
		// Stale temporary/expired files and old history precede reusable recent data.
		self.evict_files(state, target, Evict::Expired)?;
		reclaim(
			ReclaimStage::ExpiredHistory,
			state.usage.total.saturating_sub(target),
		)?;
		self.measure(state)?;
		if state.usage.total > target {
			self.evict_files(state, target, Evict::Media)?;
		}
		if state.usage.total > target {
			reclaim(
				ReclaimStage::RecentHistory,
				state.usage.total.saturating_sub(target),
			)?;
			self.measure(state)?;
		}
		if state.usage.total > target {
			self.evict_files(state, target, Evict::Artwork)?;
		}
		if state.usage.total > target {
			reclaim(
				ReclaimStage::PresentationHints,
				state.usage.total.saturating_sub(target),
			)?;
			self.measure(state)?;
		}
		state.usage.last_cleanup_reclaimed = before.saturating_sub(state.usage.total);
		state.usage.pressure = state.usage.total > target;
		Ok(())
	}
	fn evict_files(&self, state: &mut State, target: u64, kind: Evict) -> io::Result<()> {
		loop {
			// Only a fixed batch is retained, even for a hostile or very large old cache.
			let mut oldest = BinaryHeap::new();
			self.visit_files(|path, metadata, artwork, account| {
				if file_is_protected(state, path) {
					return Ok(());
				}
				let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
				let age = SystemTime::now()
					.duration_since(modified)
					.unwrap_or_default();
				let temporary = path.extension().is_some_and(|ext| ext == "part");
				let eligible = match kind {
					Evict::Expired => {
						if temporary {
							age >= Duration::from_secs(86_400)
						} else {
							age >= state.retention
						}
					}
					Evict::Media => !artwork && !temporary,
					Evict::Artwork => artwork && !temporary,
				};
				if eligible {
					let active = state.active.as_deref() == Some(account);
					oldest.push((
						active,
						modified,
						std::cmp::Reverse(metadata.len()),
						artwork,
						temporary,
						path.to_owned(),
					));
					if oldest.len() > CANDIDATES {
						oldest.pop();
					}
				}
				Ok(())
			})?;
			if oldest.is_empty() {
				break;
			}
			let mut removed = false;
			for (_, _, std::cmp::Reverse(bytes), artwork, temporary, path) in
				oldest.into_sorted_vec()
			{
				match fs::remove_file(path) {
					Ok(()) => {
						removed = true;
						let category = if temporary {
							&mut state.usage.temporary
						} else if artwork {
							&mut state.usage.artwork
						} else {
							&mut state.usage.media
						};
						*category = category.saturating_sub(bytes);
						update_total(state);
					}
					Err(error)
						if matches!(
							error.kind(),
							io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
						) => {}
					Err(error) => return Err(error),
				}
				if removed && !matches!(kind, Evict::Expired) && state.usage.total <= target {
					break;
				}
			}
			self.measure(state)?;
			if !removed || !matches!(kind, Evict::Expired) && state.usage.total <= target {
				break;
			}
		}
		Ok(())
	}
	fn visit_files(
		&self,
		mut visit: impl FnMut(&Path, &fs::Metadata, bool, &str) -> io::Result<()>,
	) -> io::Result<()> {
		let accounts = match fs::read_dir(self.root.join("avatars")) {
			Ok(entries) => entries,
			Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
			Err(error) => return Err(error),
		};
		let mut count = 0usize;
		for account in accounts {
			let account = account?;
			count += 1;
			if count > MAX_SCAN_ENTRIES {
				return Err(io::ErrorKind::InvalidData.into());
			}
			if account.file_type()?.is_file() {
				visit(&account.path(), &account.metadata()?, false, "")?;
				continue;
			}
			if !account.file_type()?.is_dir() {
				continue;
			}
			let name = account.file_name();
			// Old or malformed account directory names still occupy cache space.
			let name = name.to_str().unwrap_or("");
			for (directory, artwork) in [
				(account.path(), false),
				(account.path().join("artwork"), true),
			] {
				let entries = match fs::read_dir(directory) {
					Ok(entries) => entries,
					Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
					Err(error) => return Err(error),
				};
				for entry in entries {
					let entry = entry?;
					count += 1;
					if count > MAX_SCAN_ENTRIES {
						return Err(io::ErrorKind::InvalidData.into());
					}
					if entry.file_type()?.is_file() {
						visit(&entry.path(), &entry.metadata()?, artwork, name)?;
					} else if entry.file_type()?.is_dir()
						&& (artwork || entry.file_name() != "artwork")
					{
						// There is no supported deeper cache tree. Refuse new writes rather
						// than omit arbitrary nested content from global accounting.
						return Err(io::ErrorKind::InvalidData.into());
					}
				}
			}
		}
		Ok(())
	}
}

fn update_total(state: &mut State) {
	state.usage.reserved = state.reserved;
	state.usage.total = state
		.usage
		.database
		.saturating_add(state.usage.media)
		.saturating_add(state.usage.artwork)
		.saturating_add(state.usage.temporary)
		.saturating_add(state.reserved);
}

fn file_is_protected(state: &State, path: &Path) -> bool {
	state.protected.contains_key(path)
		|| state
			.recent
			.get(path)
			.is_some_and(|used| used.elapsed() < RECENT_GRACE)
}
fn recent_charge(path: &Path) -> usize {
	path.as_os_str().as_encoded_bytes().len() + size_of::<(PathBuf, Instant)>()
}

#[derive(Clone, Copy)]
enum Evict {
	Expired,
	Media,
	Artwork,
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicU64, Ordering};
	static SEQUENCE: AtomicU64 = AtomicU64::new(0);
	struct Fixture {
		root: PathBuf,
		budget: Arc<CacheBudget>,
	}
	impl Fixture {
		fn new() -> Self {
			let root = std::env::temp_dir().join(format!(
				"serein-cache-budget-{}-{}",
				std::process::id(),
				SEQUENCE.fetch_add(1, Ordering::Relaxed)
			));
			fs::create_dir_all(root.join("avatars/1/artwork")).unwrap();
			fs::create_dir_all(root.join("avatars/2/artwork")).unwrap();
			let budget = Arc::new(CacheBudget {
				root: root.clone(),
				limits: Limits {
					ceiling: 1000,
					headroom: 0,
					start: 900,
					target: 700,
				},
				state: Mutex::new(State::default()),
			});
			Self { root, budget }
		}
		fn file(&self, name: &str, bytes: usize) {
			fs::write(self.root.join(name), vec![0; bytes]).unwrap();
		}
	}
	impl Drop for Fixture {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.root);
		}
	}

	#[test]
	fn all_accounts_journals_and_pending_writes_share_one_ceiling() {
		let fixture = Fixture::new();
		fixture.file("client.sqlite3", 400);
		fixture.file("client.sqlite3-wal", 100);
		fixture.file("avatars/1/a.png", 100);
		fixture.file("avatars/2/artwork/b.png", 100);
		fixture.file("avatars/2/current.part", 50);
		let reservation = fixture.budget.reserve(200).unwrap();
		let usage = fixture.budget.usage();
		assert_eq!(
			(
				usage.database,
				usage.media,
				usage.artwork,
				usage.temporary,
				usage.reserved,
				usage.total
			),
			(500, 100, 100, 50, 200, 950)
		);
		assert_eq!(
			fixture.budget.reserve(51).err().unwrap().kind(),
			io::ErrorKind::StorageFull
		);
		drop(reservation);
		assert!(fixture.budget.reserve(250).is_ok());
	}

	#[test]
	fn eviction_preserves_active_reads_and_cheap_artwork_until_history_is_tried() {
		let fixture = Fixture::new();
		fixture.file("client.sqlite3", 500);
		fixture.file("avatars/1/active.png", 200);
		fixture.file("avatars/2/old.png", 100);
		fixture.file("avatars/2/artwork/icon.png", 100);
		fixture.budget.set_active_account(Some(model::Id(1)));
		let guard = fixture
			.budget
			.protect(&fixture.root.join("avatars/1/active.png"))
			.unwrap();
		let mut stages = vec![];
		let usage = fixture
			.budget
			.maintain(|stage, _| {
				stages.push(stage);
				if stage == ReclaimStage::RecentHistory {
					fixture.file("client.sqlite3", 300);
				}
				Ok(())
			})
			.unwrap();
		assert_eq!(
			stages,
			[ReclaimStage::ExpiredHistory, ReclaimStage::RecentHistory]
		);
		assert!(fixture.root.join("avatars/1/active.png").exists());
		assert!(!fixture.root.join("avatars/2/old.png").exists());
		assert!(fixture.root.join("avatars/2/artwork/icon.png").exists());
		assert!(usage.total <= 700);
		drop(guard);
	}

	#[test]
	fn pressure_without_storage_owner_cannot_exceed_ceiling_or_delete_history() {
		let fixture = Fixture::new();
		fixture.file("client.sqlite3", 950);
		assert!(fixture.budget.reserve(51).is_err());
		assert!(fixture.budget.needs_maintenance());
		assert_eq!(
			fs::metadata(fixture.root.join("client.sqlite3"))
				.unwrap()
				.len(),
			950
		);
	}

	#[test]
	fn shared_roots_reuse_reservations_and_drop_never_needs_the_filesystem() {
		let fixture = Fixture::new();
		let first = CacheBudget::for_root(&fixture.root);
		let second = CacheBudget::for_root(&fixture.root);
		assert!(Arc::ptr_eq(&first, &second));
		let reservation = first.reserve(100).unwrap();
		assert_eq!(second.usage().reserved, 100);
		fs::remove_dir_all(fixture.root.join("avatars")).unwrap();
		drop(reservation);
		assert_eq!(second.usage().reserved, 0);
	}

	#[test]
	fn committed_replacements_keep_other_writer_reservations_charged() {
		let fixture = Fixture::new();
		fixture.file("avatars/1/a.png", 100);
		let first = fixture.budget.reserve(200).unwrap();
		let second = fixture.budget.reserve(300).unwrap();
		fixture.file("avatars/1/a.png", 200);
		first.commit_file(100, 200, false).unwrap();
		assert_eq!(
			(
				fixture.budget.usage().media,
				fixture.budget.usage().reserved
			),
			(200, 300)
		);
		assert!(fixture.budget.reserve(501).is_err());
		drop(second);
		assert_eq!(fixture.budget.refresh().unwrap().total, 200);
	}

	#[test]
	fn refresh_between_rename_and_commit_does_not_apply_a_shrink_twice() {
		let fixture = Fixture::new();
		fixture.file("avatars/1/a.png", 200);
		fixture.file("avatars/2/other.png", 100);
		let replacement = fixture.budget.reserve(100).unwrap();
		fixture.file("avatars/1/a.png", 100);
		fixture.budget.refresh().unwrap();
		replacement.commit_file(200, 100, false).unwrap();
		assert_eq!(fixture.budget.usage().media, 200);
		assert_eq!(fixture.budget.usage().total, 200);
		assert!(fixture.budget.reserve(801).is_err());
	}

	#[test]
	fn unknown_cache_names_are_counted_and_unsupported_nested_trees_refuse_writes() {
		let fixture = Fixture::new();
		fixture.file("avatars/legacy.png", 100);
		fs::create_dir_all(fixture.root.join("avatars/old-account")).unwrap();
		fixture.file("avatars/old-account/image.png", 100);
		assert_eq!(fixture.budget.refresh().unwrap().media, 200);
		fs::create_dir_all(fixture.root.join("avatars/old-account/unrecognized")).unwrap();
		assert_eq!(
			fixture.budget.refresh().unwrap_err().kind(),
			io::ErrorKind::InvalidData
		);
		assert_eq!(
			fixture.budget.reserve(1).err().unwrap().kind(),
			io::ErrorKind::InvalidData
		);
	}

	#[test]
	fn ordinary_admission_leaves_room_for_bounded_cleanup_journals() {
		let mut fixture = Fixture::new();
		Arc::get_mut(&mut fixture.budget).unwrap().limits.headroom = 100;
		fixture.file("client.sqlite3", 900);
		assert!(fixture.budget.reserve(1).is_err());
		let usage = fixture
			.budget
			.maintain(|stage, wanted| {
				if stage == ReclaimStage::ExpiredHistory {
					assert!(wanted >= 200);
					// A cleanup journal can briefly coexist with the full old database.
					fixture.file("client.sqlite3-wal", 100);
					fixture.file("client.sqlite3", 600);
					fs::remove_file(fixture.root.join("client.sqlite3-wal"))?;
				}
				Ok(())
			})
			.unwrap();
		assert_eq!(usage.total, 600);
		assert!(fixture.budget.reserve(300).is_ok());
	}

	#[test]
	fn fresh_file_grace_survives_pressure_but_expires_without_a_read_lease() {
		let fixture = Fixture::new();
		fixture.file("client.sqlite3", 700);
		fixture.file("avatars/1/fresh.png", 200);
		fixture.file("avatars/2/artwork/icon.png", 100);
		let fresh = fixture.root.join("avatars/1/fresh.png");
		fixture.budget.record_file_use(&fresh);
		fixture.budget.maintain(|_, _| Ok(())).unwrap();
		assert!(fresh.exists());
		assert!(fixture.budget.is_protected(&fresh));
		*fixture
			.budget
			.state
			.lock()
			.unwrap()
			.recent
			.get_mut(&fresh)
			.unwrap() = Instant::now() - RECENT_GRACE - Duration::from_secs(1);
		fixture.budget.maintain(|_, _| Ok(())).unwrap();
		assert!(!fresh.exists());
		assert_eq!(fixture.budget.usage().total, 700);
	}

	#[test]
	fn recent_file_grace_has_fixed_item_and_byte_bounds() {
		let fixture = Fixture::new();
		for index in 0..RECENT_FILES + 16 {
			fixture
				.budget
				.record_file_use(&fixture.root.join(format!("avatars/1/{index}.png")));
		}
		let state = fixture.budget.state.lock().unwrap();
		assert_eq!(state.recent.len(), RECENT_FILES);
		assert!(state.recent_bytes <= RECENT_BYTES);
		assert!(!file_is_protected(
			&state,
			&fixture.root.join("avatars/1/0.png")
		));
		assert!(file_is_protected(
			&state,
			&fixture
				.root
				.join(format!("avatars/1/{}.png", RECENT_FILES + 15))
		));
	}
}
