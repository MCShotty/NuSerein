//! Explicit, cancellable background refresh. Never installs history into the visible timeline.
use super::{Desktop, Event, cache};
use client_core::{
	auth::{AuthState, Failure},
	force_sync_read::{HISTORY_MILLIS, Read, ReadResult, last_before},
};
use model::Id;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ui::force_sync::{Action, Outcome, Progress, Scope, Section, Stage, Status};

const MAX_TARGETS: usize = 10_000;
const MAX_CHECKPOINT: usize = 1024 * 1024;
const LAST_STEP: u8 = 11;
const MAX_STALE_RETRIES: u8 = 3;
const MAX_READ_RETRIES: u8 = 2;
static NEXT_JOB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn next_job_id(now: u64) -> Option<u64> {
	use std::sync::atomic::Ordering::Relaxed;
	let previous = NEXT_JOB
		.fetch_update(Relaxed, Relaxed, |old| old.max(now).checked_add(1))
		.ok()?;
	previous.max(now).checked_add(1)
}

#[derive(Clone, Serialize, Deserialize)]
struct Target {
	id: Id,
	kind: u8,
	#[serde(default)]
	parent: Option<Id>,
}
impl Target {
	fn has_history(&self) -> bool {
		matches!(self.kind, 0..=3 | 5 | 10..=12)
	}
	fn readable(&self, state: &client_core::State, guild: Option<Id>) -> bool {
		if state.channel(self.id).is_some() {
			return state.can_read_history(self.id);
		}
		matches!(self.kind, 10..=12)
			&& self.parent.is_some_and(|parent| {
				state
					.channel(parent)
					.is_some_and(|channel| channel.guild == guild)
					&& state.can_read_history(parent)
			})
	}
}

#[derive(Clone, Serialize, Deserialize)]
struct Job {
	version: u8,
	id: u64,
	account: Id,
	generation: u64,
	scope: Option<Id>,
	end_ms: i64,
	cutoff_ms: i64,
	guilds: Vec<Id>,
	guild_index: usize,
	dms: Vec<Id>,
	targets: Vec<Target>,
	channel_index: usize,
	step: u8,
	archive_index: usize,
	archive_kind: u8,
	archive_time: Option<i128>,
	archive_id: Option<Id>,
	members: Vec<Id>,
	member_index: usize,
	before: Option<Id>,
	request: u64,
	channels_done: u64,
	channels_total: u64,
	messages_saved: u64,
	outcomes: [u8; 7],
	#[serde(default)]
	details: [u8; 7],
	status: u8,
	roster_started: bool,
	#[serde(default)]
	roster_complete: bool,
}
impl Job {
	fn new(state: &client_core::State, scope: Scope) -> Option<Self> {
		let account = state.user.as_ref()?.id;
		let now = i64::try_from(
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.ok()?
				.as_millis(),
		)
		.ok()?;
		let guilds = match scope {
			Scope::Server(guild) => {
				state.guilds.iter().find(|g| g.id == guild)?;
				vec![guild]
			}
			Scope::Account => state.guilds.iter().map(|g| g.id).take(4096).collect(),
		};
		let dms = if scope == Scope::Account {
			state
				.channels
				.iter()
				.filter(|c| c.guild.is_none() && matches!(c.kind, 1 | 3) && state.can_view(c.id))
				.map(|c| c.id)
				.take(MAX_TARGETS)
				.collect()
		} else {
			Vec::new()
		};
		let mut job = Self {
			version: 1,
			id: next_job_id(now as u64)?,
			account,
			generation: state.generation,
			scope: match scope {
				Scope::Server(id) => Some(id),
				Scope::Account => None,
			},
			end_ms: now,
			cutoff_ms: now.saturating_sub(HISTORY_MILLIS),
			guilds,
			guild_index: 0,
			dms,
			targets: Vec::new(),
			channel_index: 0,
			step: 0,
			archive_index: 0,
			archive_kind: 0,
			archive_time: None,
			archive_id: None,
			members: Vec::new(),
			member_index: 0,
			before: None,
			request: 0,
			channels_done: 0,
			channels_total: 0,
			messages_saved: 0,
			outcomes: [0; 7],
			details: [0; 7],
			status: 0,
			roster_started: false,
			roster_complete: false,
		};
		if job.guilds.is_empty() {
			job.begin_dms();
		}
		Some(job)
	}
	fn scope(&self) -> Scope {
		self.scope.map_or(Scope::Account, Scope::Server)
	}
	fn guild(&self) -> Option<Id> {
		self.guilds.get(self.guild_index).copied()
	}
	fn stage(&self) -> Stage {
		match self.step {
			0 => Stage::Server,
			1 | 2 | 8 => Stage::Channels,
			3..=5 => Stage::Expressions,
			6 => Stage::Preferences,
			7 | 10 => Stage::Members,
			_ => Stage::History,
		}
	}
	fn stage_index(stage: Stage) -> usize {
		Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0)
	}
	fn record(&mut self, stage: Stage, outcome: Outcome) {
		let value = match outcome {
			Outcome::Updated => 1,
			Outcome::Partial => 2,
			Outcome::Unavailable => 3,
			Outcome::Failed => 4,
		};
		let slot = &mut self.outcomes[Self::stage_index(stage)];
		*slot = (*slot).max(value);
	}

	fn record_failure(&mut self, stage: Stage, failure: Failure) {
		let (outcome, detail) = match failure {
			Failure::Forbidden => (Outcome::Unavailable, 1),
			Failure::Network
			| Failure::Expired
			| Failure::InvalidCredential
			| Failure::Challenged => (Outcome::Failed, 2),
			Failure::RateLimited => (Outcome::Partial, 3),
			Failure::Busy | Failure::ResponseLimit | Failure::Capacity | Failure::CapacityAt(_) => {
				(Outcome::Partial, 4)
			}
			_ => (Outcome::Failed, 5),
		};
		self.record(stage, outcome);
		self.details[Self::stage_index(stage)] = detail;
	}
	fn record_storage_failure(&mut self, stage: Stage, failure: local_store::StoreError) {
		self.record(stage, Outcome::Partial);
		self.details[Self::stage_index(stage)] = match failure {
			local_store::StoreError::Capacity => 4,
			local_store::StoreError::Unavailable => 6,
			local_store::StoreError::Incompatible => 5,
		};
	}

	fn valid(&self) -> bool {
		let distinct = |ids: &[Id]| {
			ids.iter().all(|id| id.0 != 0)
				&& ids.iter().collect::<std::collections::BTreeSet<_>>().len() == ids.len()
		};
		self.version == 1
			&& self.account.0 != 0
			&& self.id != 0
			&& self.guilds.len() <= 4096
			&& self.dms.len() <= MAX_TARGETS
			&& self.targets.len() <= MAX_TARGETS
			&& self.members.len() <= MAX_TARGETS
			&& self.guild_index <= self.guilds.len()
			&& self.channel_index <= self.targets.len()
			&& self.member_index <= self.members.len()
			&& self.archive_index <= self.targets.len()
			&& self.step <= LAST_STEP
			&& self.status <= 4
			&& self.archive_kind <= 1
			&& self.outcomes.iter().all(|v| *v <= 4)
			&& self.details.iter().all(|v| *v <= 7)
			&& self.channels_done <= self.channels_total
			&& self.end_ms.checked_sub(self.cutoff_ms) == Some(HISTORY_MILLIS)
			&& self.cutoff_ms >= client_core::force_sync_read::DISCORD_EPOCH_MS
			&& last_before(self.end_ms).is_some()
			&& self.before.is_none_or(|id| {
				id.0 != 0 && last_before(self.end_ms).is_some_and(|upper| id <= upper)
			}) && self.scope.is_none_or(|guild| {
			guild.0 != 0 && self.guilds.as_slice() == [guild] && self.dms.is_empty()
		}) && distinct(&self.guilds)
			&& distinct(&self.dms)
			&& distinct(&self.members)
			&& self.targets.iter().all(|target| {
				target.id.0 != 0
					&& matches!(target.kind,0..=5|10..=16)
					&& target
						.parent
						.is_none_or(|parent| parent.0 != 0 && parent != target.id)
			}) && self
			.targets
			.iter()
			.map(|t| t.id)
			.collect::<std::collections::BTreeSet<_>>()
			.len() == self.targets.len()
			&& self.archive_time.is_none_or(|time| {
				self.archive_kind == 0 && time >= 0 && time <= i128::from(self.end_ms) * 1_000_000
			}) && self
			.archive_id
			.is_none_or(|id| self.archive_kind == 1 && id.0 != 0)
	}
	fn skip_read(&mut self, read: &Read) {
		match read {
			Read::History { .. } => self.next_channel(),
			Read::Archives { .. } => self.advance_archive(),
			Read::Members { .. } => {
				self.member_index = (self.member_index + 100).min(self.members.len())
			}
			_ => self.advance(),
		}
	}
	fn advance_archive(&mut self) {
		self.archive_time = None;
		self.archive_id = None;
		if self.archive_kind == 0 {
			self.archive_kind = 1;
		} else {
			self.archive_kind = 0;
			self.archive_index += 1;
		}
	}

	fn add_targets(&mut self, rows: impl IntoIterator<Item = Target>) {
		for row in rows {
			if self.targets.iter().any(|t| t.id == row.id) {
				continue;
			}
			if self.targets.len() == MAX_TARGETS {
				self.record(Stage::History, Outcome::Partial);
				break;
			}
			self.targets.push(row);
		}
	}
	fn discover(&mut self, state: &client_core::State) {
		let guild = self.guild();
		self.add_targets(
			state
				.channels
				.iter()
				.filter(|c| c.guild == guild && state.can_view(c.id))
				.map(|c| Target {
					id: c.id,
					kind: c.kind,
					parent: c.parent_id,
				}),
		);
	}
	fn add_members(&mut self, rows: impl IntoIterator<Item = Id>) {
		for id in rows {
			if !self.members.contains(&id) {
				if self.members.len() == MAX_TARGETS {
					self.record(Stage::Members, Outcome::Partial);
					break;
				}
				self.members.push(id);
			}
		}
	}
	fn begin_dms(&mut self) {
		self.targets = self
			.dms
			.iter()
			.map(|id| Target {
				id: *id,
				kind: 1,
				parent: None,
			})
			.collect();
		self.channels_total += self.targets.len() as u64;
		self.channel_index = 0;
		self.before = last_before(self.end_ms);
		self.step = 9;
	}
	fn next_guild(&mut self) {
		self.guild_index += 1;
		self.targets.clear();
		self.members.clear();
		self.member_index = 0;
		self.channel_index = 0;
		self.archive_index = 0;
		self.archive_kind = 0;
		self.archive_time = None;
		self.archive_id = None;
		self.before = None;
		self.roster_started = false;
		self.roster_complete = false;
		self.step = 0;
		if self.guild_index >= self.guilds.len() {
			self.begin_dms();
		}
	}
	fn next_channel(&mut self) {
		self.channels_done += 1;
		self.channel_index += 1;
		self.before = last_before(self.end_ms);
	}
	fn skip_unreadable_targets(&mut self, state: &client_core::State) {
		while let Some(target) = self.targets.get(self.channel_index) {
			if !target.has_history() {
				self.channel_index += 1;
			} else if !target.readable(state, self.guild()) {
				self.record_failure(Stage::History, Failure::Forbidden);
				self.next_channel();
			} else {
				break;
			}
		}
	}
	fn advance(&mut self) {
		self.step += 1;
		if self.step == 9 {
			self.channels_total += self.targets.iter().filter(|t| t.has_history()).count() as u64;
			self.before = last_before(self.end_ms);
		}
	}
	fn progress(&self, cache_bytes: u64) -> Progress {
		Progress {
			scope: self.scope(),
			status: match self.status {
				1 => Status::Paused,
				2 => Status::Waiting,
				3 => Status::Complete,
				4 => Status::Cancelled,
				_ => Status::Running,
			},
			stage: self.stage(),
			channels_done: self.channels_done,
			channels_total: self.channels_total,
			messages_saved: self.messages_saved,
			cache_bytes,
			sections: Stage::ALL
				.iter()
				.enumerate()
				.filter_map(|(i, stage)| {
					let outcome = match self.outcomes[i] {
						1 => Outcome::Updated,
						2 => Outcome::Partial,
						3 => Outcome::Unavailable,
						4 => Outcome::Failed,
						_ => return None,
					};
					Some(Section {
						stage: *stage,
						outcome,
						detail: match self.details[i] {
							1 => Some("force-sync-error-permission"),
							2 => Some("force-sync-error-network"),
							3 => Some("force-sync-error-rate-limit"),
							4 => Some("force-sync-error-capacity"),
							5 => Some("force-sync-error-unsupported"),
							6 => Some("force-sync-error-storage"),
							7 => Some("force-sync-error-stale"),
							_ if *stage == Stage::Members && self.outcomes[i] == 2 => {
								Some("force-sync-members-partial")
							}
							_ => None,
						},
					})
				})
				.collect(),
			detail: if self.status == 2 {
				Some("force-sync-waiting-connection")
			} else {
				None
			},
			can_retry: self.status >= 3 && self.outcomes.iter().any(|x| *x >= 2),
		}
	}
}

struct Active {
	request: u64,
	revision: u64,
	read: Read,
	task: tokio::task::JoinHandle<Result<ReadResult, Failure>>,
}
impl Drop for Active {
	fn drop(&mut self) {
		self.task.abort();
	}
}
struct PageCommit {
	channel: Id,
	request: u64,
	next: Option<Id>,
	done: bool,
}

#[derive(Default)]
pub(super) struct Sync {
	job: Option<Job>,
	active: Option<Active>,
	pending_page: Option<PageCommit>,
	revision_requested: bool,
	revision: Option<u64>,
	checkpoint_loaded: Option<(u64, Id)>,
	clear_checkpoint: bool,
	metadata_revision: u64,
	history_revision: u64,
	preference: Option<u64>,
	preference_due: Option<Instant>,
	retry_read: Option<Read>,
	stale_retries: u8,
	read_retries: u8,
	next_read: Option<Instant>,
	dirty: bool,
	usage: u64,
	usage_due: Option<Instant>,
	restore: Option<local_store::SyncCheckpoint>,
	resume: Option<Box<Job>>,
	needs_revalidate: bool,
	roster_chunk: Option<(Id, Vec<model::Member>, usize)>,
	roster_changed: std::collections::BTreeSet<Id>,
	roster_write: Option<(u64, Id)>,
	roster_finished: bool,
	roster_users: Option<Vec<Id>>,
	roster_reconcile: Option<u64>,
	roster_attempt: Option<u64>,
	roster_sequence: u64,
}

impl Sync {
	fn prepare_read(&mut self, read: &Read) {
		if self.retry_read.as_ref() != Some(read) {
			self.retry_read = Some(read.clone());
			self.stale_retries = 0;
			self.read_retries = 0;
			self.next_read = None;
		}
	}
	fn reject_stale(&mut self, job: &mut Job, read: &Read) {
		self.revision = None;
		self.revision_requested = false;
		self.stale_retries = self.stale_retries.saturating_add(1);
		if self.stale_retries >= MAX_STALE_RETRIES {
			job.record(job.stage(), Outcome::Partial);
			job.details[Job::stage_index(job.stage())] = 7;
			job.skip_read(read);
			self.retry_read = None;
		}
		self.dirty = true;
	}
	fn defer_read(&mut self, failure: Failure, now: Instant) -> bool {
		if !matches!(failure, Failure::Busy | Failure::RateLimited)
			|| self.read_retries >= MAX_READ_RETRIES
		{
			return false;
		}
		self.read_retries += 1;
		let base = if failure == Failure::RateLimited {
			60
		} else {
			1
		};
		self.next_read = Some(now + Duration::from_secs(base * u64::from(self.read_retries)));
		true
	}
	fn should_measure_usage(&self, visible: bool) -> bool {
		visible || self.job.as_ref().is_some_and(|job| job.status == 0)
	}
}

impl Desktop {
	pub(super) fn poll_force_sync(&mut self, ctx: &eframe::egui::Context) {
		let Some(account) = self.state.user.as_ref().map(|u| u.id) else {
			if let Some(cache) = &self.cache {
				cache.protect_history(Id(0), None);
				cache.protect_guild(Id(0), None);
				cache.protect_sync_history(Id(0), &[]);
			}
			self.cancel_force_sync_roster();
			self.force_sync = Sync::default();
			self.messaging.force_sync_progress = None;
			return;
		};
		if let Some(cache) = &self.cache {
			let syncing = self
				.force_sync
				.job
				.as_ref()
				.filter(|job| job.status == 0 && job.step == 9)
				.and_then(|job| job.targets.get(job.channel_index))
				.map(|target| target.id);
			cache.protect_sync_history(account, &syncing.into_iter().collect::<Vec<_>>());
			cache.protect_history(account, self.state.selected);
			cache.protect_guild(
				account,
				self.state
					.selected
					.and_then(|id| self.state.channel(id))
					.and_then(|channel| channel.guild),
			);
		}
		if self
			.force_sync
			.job
			.as_ref()
			.is_some_and(|j| j.account != account || j.generation != self.state.generation)
		{
			self.cancel_force_sync_roster();
			self.force_sync = Sync::default();
			self.messaging.force_sync_progress = None;
		}
		if self.state.auth != AuthState::Authenticated {
			return;
		}
		let key = (self.state.generation, account);
		if self.force_sync.checkpoint_loaded != Some(key)
			&& self.queue_cache(cache::Operation::LoadSyncCheckpoint)
		{
			self.force_sync.checkpoint_loaded = Some(key);
		}
		if let Some(checkpoint) = self.force_sync.restore.take()
			&& self.force_sync.job.is_none()
			&& checkpoint.payload.len() <= MAX_CHECKPOINT
			&& let Ok(mut job) = serde_json::from_slice::<Job>(&checkpoint.payload)
			&& job.valid()
			&& job.account == account
			&& checkpoint.job == job.id
			&& checkpoint.scope == job.scope
			&& checkpoint.started_ms == job.end_ms
			&& checkpoint.cutoff_ms == job.cutoff_ms
			&& job.status < 3
		{
			job.generation = self.state.generation;
			job.status = 1;
			job.roster_started = false;
			self.force_sync.needs_revalidate = true;
			self.force_sync.job = Some(job);
		}
		if let Some(action) = self.messaging.force_sync_action.take() {
			self.force_sync_action(action);
		}
		if self.force_sync.clear_checkpoint
			&& self.queue_cache(cache::Operation::ClearSyncCheckpoint)
		{
			self.force_sync.clear_checkpoint = false;
		}
		if self
			.force_sync
			.should_measure_usage(self.messaging.force_sync_usage_visible())
			&& self
				.force_sync
				.usage_due
				.is_none_or(|at| Instant::now() >= at)
			&& self.queue_cache(cache::Operation::CacheUsage)
		{
			self.force_sync.usage_due = Some(Instant::now() + Duration::from_secs(5));
		}
		if self.force_sync.job.is_none() {
			return;
		}
		let mut job = self.force_sync.job.take().expect("job checked");
		if !self.state.gateway_connected && job.status == 0 {
			job.status = 2;
			job.roster_started = false;
			self.cancel_force_sync_roster();
			self.force_sync.active = None;
			self.force_sync.revision = None;
			self.force_sync.revision_requested = false;
			self.force_sync.dirty = true;
		}
		if self.state.gateway_connected && job.status == 2 {
			job.status = 0;
			self.force_sync.dirty = true;
		}
		if job.status == 0 {
			if let Some(active) = self.force_sync.active.take() {
				if active.task.is_finished() {
					let mut active = active;
					let finished = self
						.runtime
						.block_on(&mut active.task)
						.unwrap_or(Err(Failure::Network));
					let revision = if matches!(active.read, Read::History { .. }) {
						self.force_sync.history_revision
					} else {
						self.force_sync.metadata_revision
					};
					if active.request != job.request || active.revision != revision {
						self.force_sync.reject_stale(&mut job, &active.read);
					} else {
						self.force_sync_read_result(&mut job, active.read.clone(), finished);
					}
				} else {
					self.force_sync.active = Some(active);
				}
			}
			if self.force_sync.active.is_none() && self.force_sync.pending_page.is_none() {
				self.drive_force_sync(&mut job, ctx);
			}
		}
		if self.force_sync.dirty {
			let checkpoint_job = self.force_sync.resume.as_deref().unwrap_or(&job);
			if let Ok(payload) = serde_json::to_vec(checkpoint_job)
				&& payload.len() <= MAX_CHECKPOINT
			{
				let checkpoint = local_store::SyncCheckpoint {
					job: checkpoint_job.id,
					scope: checkpoint_job.scope,
					started_ms: checkpoint_job.end_ms,
					cutoff_ms: checkpoint_job.cutoff_ms,
					payload,
				};
				if self.queue_cache(cache::Operation::SaveSyncCheckpoint(checkpoint)) {
					self.force_sync.dirty = false;
				}
			}
		}
		let mut progress = job.progress(self.force_sync.usage);
		if job.status == 0
			&& self
				.force_sync
				.next_read
				.is_some_and(|at| at > Instant::now())
		{
			progress.status = Status::Waiting;
			progress.detail = Some("force-sync-waiting-rate-limit");
		}
		self.messaging.force_sync_progress = Some(progress);
		if job.status == 0 && self.force_sync.roster_chunk.is_some() {
			ctx.request_repaint_after(Duration::from_millis(16));
		}
		if job.status == 0 {
			ctx.request_repaint_after(
				self.force_sync
					.next_read
					.map_or(Duration::from_millis(250), |at| {
						at.saturating_duration_since(Instant::now())
					}),
			);
		}
		self.force_sync.job = Some(job);
	}

	fn force_sync_action(&mut self, action: Action) {
		match action {
			Action::Start(scope) => {
				if self.force_sync.job.as_ref().is_some_and(|j| j.status < 3) {
					return;
				}
				if self.state.demo {
					self.messaging.force_sync_progress =
						Job::new(&self.state, scope).map(|j| j.progress(0));
					return;
				}
				if let Some(job) = Job::new(&self.state, scope) {
					// Explicit sync retries presentation writes after storage pressure clears.
					self.server_cache_failed = false;
					self.cancel_force_sync_roster();
					self.force_sync.active = None;
					self.force_sync.pending_page = None;
					self.force_sync.revision = None;
					self.force_sync.revision_requested = false;
					self.force_sync.retry_read = None;
					self.force_sync.next_read = None;
					self.force_sync.preference_due = None;
					self.force_sync.job = Some(job);
					self.force_sync.dirty = true;
				}
			}
			Action::Pause => {
				if let Some(j) = &mut self.force_sync.job {
					j.status = 1;
				}
				self.force_sync.active = None;
				self.force_sync.revision = None;
				self.force_sync.revision_requested = false;
				self.force_sync.dirty = true;
				self.cancel_force_sync_roster();
			}
			Action::Resume => {
				if let Some(mut job) = self.force_sync.job.take() {
					if job.status < 3 {
						self.server_cache_failed = false;
						job.status = 0;
						if self.force_sync.needs_revalidate {
							self.force_sync.resume = Some(Box::new(job.clone()));
							self.force_sync.needs_revalidate = false;
							if job.guild().is_some() {
								job.step = 0;
								job.targets.clear();
								job.channel_index = 0;
							} else {
								self.restore_force_sync_continuation(&mut job);
							}
						}
						self.force_sync.dirty = true;
					}
					self.force_sync.job = Some(job);
				}
			}
			Action::Cancel => {
				if let Some(j) = &mut self.force_sync.job {
					j.status = 4;
				}
				self.force_sync.active = None;
				self.force_sync.pending_page = None;
				self.force_sync.preference = None;
				self.force_sync.preference_due = None;
				self.force_sync.next_read = None;
				self.force_sync.dirty = false;
				self.force_sync.clear_checkpoint = true;
				self.cancel_force_sync_roster();
			}
			Action::Retry => {
				if let Some(j) = &self.force_sync.job {
					let scope = j.scope();
					self.force_sync.job = None;
					self.force_sync_action(Action::Start(scope));
				}
			}
		}
	}

	fn drive_force_sync(&mut self, job: &mut Job, ctx: &eframe::egui::Context) {
		if let Some(due) = self.force_sync.next_read
			&& due > Instant::now()
		{
			ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
			return;
		}
		if let Some(guild) = job.guild()
			&& self.state.guilds.iter().all(|g| g.id != guild)
		{
			job.record(job.stage(), Outcome::Unavailable);
			job.next_guild();
			self.force_sync.dirty = true;
			return;
		}
		let guild = job.guild();
		let read = match job.step {
			0 => guild.map(|guild| Read::Guild {
				guild,
				user: job.account,
			}),
			1 => guild.map(|guild| Read::Channels {
				guild,
				user: job.account,
			}),
			2 => guild.map(|guild| Read::ActiveThreads { guild }),
			3 => guild.map(|guild| Read::Emojis { guild }),
			4 => guild.map(|guild| Read::Stickers { guild }),
			5 => guild.map(|guild| Read::Sounds { guild }),
			6 => {
				let due = *self
					.force_sync
					.preference_due
					.get_or_insert_with(|| Instant::now() + Duration::from_secs(45));
				if Instant::now() >= due {
					job.record_failure(Stage::Preferences, Failure::Network);
					job.advance();
					self.force_sync.preference = None;
					self.force_sync.preference_due = None;
					self.force_sync.dirty = true;
					return;
				}
				if self.force_sync.preference.is_none()
					&& let Some(guild) = guild
					&& let Some(command) = self.state.refresh_channel_preferences(guild)
				{
					if let client_core::Command::ChannelPreferences(request) = &command {
						self.force_sync.preference = Some(request.request);
					}
					self.command(command);
				}
				return;
			}
			7 => {
				self.drive_force_sync_roster(job);
				return;
			}
			8 => {
				while job.archive_index < job.targets.len()
					&& !matches!(job.targets[job.archive_index].kind, 0 | 5 | 15 | 16)
				{
					job.archive_index += 1;
				}
				if let Some(target) = job.targets.get(job.archive_index) {
					guild.map(|guild| Read::Archives {
						guild,
						parent: target.id,
						kind: if job.archive_kind == 0 {
							model::archives::Kind::Public
						} else {
							model::archives::Kind::JoinedPrivate
						},
						before: if job.archive_kind == 0 {
							job.archive_time.map(model::archives::Cursor::Time)
						} else {
							job.archive_id.map(model::archives::Cursor::Id)
						},
					})
				} else {
					job.advance();
					self.force_sync.dirty = true;
					return;
				}
			}
			9 => {
				job.skip_unreadable_targets(&self.state);
				let Some(target) = job.targets.get(job.channel_index) else {
					if guild.is_some() {
						job.advance();
					} else {
						job.status = 3;
						self.force_sync.clear_checkpoint = true;
					}
					self.force_sync.dirty = job.status != 3;
					return;
				};
				if self.force_sync.revision.is_none() {
					if !self.force_sync.revision_requested
						&& self.queue_cache(cache::Operation::SyncReadRevision {
							job: job.id,
							request: job.request,
							channel: target.id,
						}) {
						self.force_sync.revision_requested = true;
					}
					return;
				}
				Some(Read::History {
					channel: target.id,
					before: job.before,
					cutoff_ms: job.cutoff_ms,
					end_ms: job.end_ms,
				})
			}
			10 => {
				if self.force_sync.roster_write.is_some() {
					return;
				}
				if job.roster_complete {
					job.advance();
					return;
				}
				let end = (job.member_index + 100).min(job.members.len());
				if end == job.member_index {
					job.advance();
					return;
				}
				let mut users = job.members[job.member_index..end].to_vec();
				users.sort_unstable();
				users.dedup();
				guild.map(|guild| Read::Members { guild, users })
			}
			_ => {
				job.next_guild();
				self.force_sync.dirty = true;
				return;
			}
		};
		let Some(read) = read else {
			return;
		};
		self.force_sync.prepare_read(&read);
		let api = self
			.connection
			.as_ref()
			.and_then(|c| c.sync_api.borrow().clone());
		let Some(api) = api else {
			return;
		};
		job.request = job.request.wrapping_add(1);
		let request = job.request;
		let revision = if matches!(read, Read::History { .. }) {
			self.force_sync.history_revision
		} else {
			self.force_sync.metadata_revision
		};
		let worker_read = read.clone();
		self.force_sync.next_read = None;
		let wake = ctx.clone();
		let task = self.runtime.spawn(async move {
			let result =
				tokio::time::timeout(Duration::from_secs(45), api.force_sync_read(worker_read))
					.await
					.unwrap_or(Err(Failure::Network));
			wake.request_repaint();
			result
		});
		self.force_sync.active = Some(Active {
			request,
			revision,
			read,
			task,
		});
	}

	fn force_sync_read_result(
		&mut self,
		job: &mut Job,
		read: Read,
		result: Result<ReadResult, Failure>,
	) {
		self.force_sync.dirty = true;
		let stage = job.stage();
		let result = match result {
			Ok(result) => {
				self.force_sync.read_retries = 0;
				self.force_sync.next_read = None;
				if !matches!(read, Read::History { .. }) {
					self.force_sync.stale_retries = 0;
				}
				result
			}
			Err(failure) => {
				if failure.ends_session() {
					if let Some(connection) = &self.connection {
						if let Some(api) = connection.sync_api.borrow().as_ref() {
							api.stop();
						}
						connection.sync_failure.send_replace(Some(failure));
					}
					job.record_failure(stage, failure);
					job.status = 1;
					self.force_sync.next_read = None;
					self.force_sync.pending_page = None;
					self.cancel_force_sync_roster();
					return;
				}
				if self.force_sync.defer_read(failure, Instant::now()) {
					return;
				}
				job.record_failure(stage, failure);
				job.skip_read(&read);
				self.force_sync.revision = None;
				self.force_sync.revision_requested = false;
				self.force_sync.retry_read = None;
				self.force_sync.next_read = None;
				return;
			}
		};
		if result.bytes() > 8 * 1024 * 1024 {
			job.record_failure(stage, Failure::ResponseLimit);
			job.skip_read(&read);
			return;
		}
		match result {
			ReadResult::History(page) => {
				if page.channel != job.targets[job.channel_index].id
					|| !job.targets[job.channel_index].readable(&self.state, job.guild())
				{
					job.record(Stage::History, Outcome::Unavailable);
					job.next_channel();
					return;
				}
				job.add_members(page.messages.iter().map(|m| m.author.id));
				let before = match read {
					Read::History { before, .. } => before,
					_ => None,
				};
				if page
					.oldest
					.is_some_and(|oldest| before.is_some_and(|bound| oldest >= bound))
				{
					job.record_failure(Stage::History, Failure::Protocol);
					job.next_channel();
					self.force_sync.revision = None;
					self.force_sync.revision_requested = false;
					return;
				}
				let done = page.boundary_reached || page.oldest.is_none();
				let coverage = local_store::HistoryCoverage {
					job: job.id,
					request: job.request,
					cutoff: Id(
						((job.cutoff_ms - client_core::force_sync_read::DISCORD_EPOCH_MS) as u64)
							<< 22,
					),
					upper: last_before(job.end_ms).unwrap_or(Id(u64::MAX)),
					before,
					next_before: if done { None } else { page.oldest },
					complete: done,
					revision: self.force_sync.revision.unwrap_or(0),
				};
				let commit = PageCommit {
					channel: page.channel,
					request: job.request,
					next: page.oldest,
					done,
				};
				if self.queue_cache(cache::Operation::SyncSavePage {
					job: job.id,
					request: job.request,
					channel: page.channel,
					messages: page.messages,
					coverage,
				}) {
					self.force_sync.pending_page = Some(commit);
				} else {
					job.record_failure(Stage::History, Failure::Busy);
					job.next_channel();
				}
				self.force_sync.revision = None;
				self.force_sync.revision_requested = false;
			}
			ReadResult::Archives(page) => {
				let next = page.next;
				job.add_targets(page.threads.iter().map(|c| Target {
					id: c.id,
					kind: c.kind,
					parent: c.parent_id,
				}));

				match next {
					Some(model::archives::Cursor::Time(at))
						if at >= i128::from(job.cutoff_ms) * 1_000_000 =>
					{
						job.archive_time = Some(at)
					}
					Some(model::archives::Cursor::Id(id)) => job.archive_id = Some(id),
					_ => self.advance_sync_archive(job),
				}
			}
			ReadResult::Members {
				guild,
				items,
				missing,
			} => {
				let missing_on_disk = missing.clone();
				let completed_members = items.clone();
				let rows = items
					.iter()
					.map(|member| model::MemberDisplay {
						guild,
						user: member.user.id,
						name: member.user.name.clone(),
						username: member.user.username.clone(),
						nick: member.nick.clone(),
					})
					.collect();
				if !missing.is_empty() {
					job.record(Stage::Members, Outcome::Partial);
				}
				if !self.state.apply_force_sync_result(ReadResult::Members {
					guild,
					items,
					missing,
				}) {
					job.record(Stage::Members, Outcome::Partial);
				}
				if self.queue_cache(cache::Operation::SyncSaveMembers {
					job: job.id,
					request: job.request,
					guild,
					rows,
					missing: missing_on_disk,
				}) {
					self.force_sync.roster_write = Some((job.request, guild));
					let count = completed_members.len();
					self.force_sync.roster_chunk = Some((guild, completed_members, count));
				} else {
					job.record_failure(Stage::Members, Failure::Busy);
					job.member_index = (job.member_index + 100).min(job.members.len());
				}
			}
			other => {
				let previously_readable: Vec<_> =
					if matches!(read, Read::Guild { .. } | Read::Channels { .. }) {
						self.state
							.channels
							.iter()
							.filter(|channel| {
								channel.guild == read.guild()
									&& self.state.can_read_history(channel.id)
							})
							.map(|channel| channel.id)
							.collect()
					} else {
						Vec::new()
					};
				let roles = matches!(&other, ReadResult::Guild(_));
				let roles_complete =
					!matches!(&other, ReadResult::Guild(value) if value.self_member.is_err());
				if let ReadResult::Guild(value) = &other
					&& let Err(failure) = value.self_member.as_ref()
				{
					job.record_failure(Stage::Members, *failure);
				}
				let applied = self.state.apply_force_sync_result(other);
				for channel in previously_readable {
					if !self.state.can_read_history(channel)
						&& let Some(cache) = &self.cache
					{
						if cache.invalidate_scope(
							self.state.generation,
							job.account,
							cache::Operation::ClearChannel { channel },
						) {
							self.cache_pending += 1;
						} else {
							cache.history.fail();
							job.record_failure(Stage::History, Failure::Busy);
							job.status = 1;
						}
					}
				}
				let outcome = if applied {
					Outcome::Updated
				} else {
					Outcome::Partial
				};
				if roles {
					job.record(
						Stage::Roles,
						if roles_complete {
							outcome
						} else {
							Outcome::Partial
						},
					);
				}
				if applied && matches!(read, Read::Channels { .. } | Read::ActiveThreads { .. }) {
					job.discover(&self.state);
				}
				job.record(stage, outcome);
				if !applied {
					job.details[Job::stage_index(stage)] = 5;
				}
				job.advance();
				if matches!(read, Read::ActiveThreads { .. }) {
					self.restore_force_sync_continuation(job);
				}
			}
		}
	}
	fn advance_sync_archive(&mut self, job: &mut Job) {
		job.advance_archive();
	}

	pub(super) fn force_sync_cache_result(
		&mut self,
		generation: u64,
		outcome: &cache::Outcome,
	) -> bool {
		match outcome {
			cache::Outcome::SyncCheckpoint { epoch, result } => {
				if self.force_sync_epoch_valid(generation, *epoch) {
					self.force_sync.restore = result.as_ref().ok().and_then(Clone::clone);
				}
				true
			}
			cache::Outcome::SyncMembersReconciled {
				job,
				request,
				guild,
				epoch,
				result,
			} => {
				if generation == self.state.generation
					&& self.force_sync.roster_reconcile == Some(*request)
					&& self
						.force_sync
						.job
						.as_ref()
						.is_some_and(|j| j.id == *job && j.guild() == Some(*guild) && j.step == 7)
				{
					self.force_sync.roster_reconcile = None;
					let mut current = self.force_sync.job.take().expect("job checked");
					let failure = match result {
						_ if !self.force_sync_epoch_valid(generation, *epoch) => {
							Some(Failure::Busy)
						}
						Ok(_)
							if self.force_sync.roster_changed.is_empty()
								&& self.force_sync.roster_users.as_ref().is_some_and(|users| {
									self.state.reconcile_force_sync_roster(*guild, users)
								}) =>
						{
							None
						}
						Ok(_) => Some(Failure::Busy),
						Err(error) => {
							current.record_storage_failure(Stage::Members, *error);
							Some(Failure::Busy)
						}
					};
					self.finish_force_sync_roster(&mut current, failure);
					self.force_sync.job = Some(current);
				}
				true
			}
			cache::Outcome::SyncCheckpointSaved { epoch, result } => {
				if result.is_err()
					&& self.force_sync_epoch_valid(generation, *epoch)
					&& let Some(j) = &mut self.force_sync.job
				{
					j.record(Stage::History, Outcome::Partial);
					j.details[Job::stage_index(Stage::History)] = 6;
					if j.status >= 3 {
						self.force_sync.clear_checkpoint = true;
					}
				}
				true
			}
			cache::Outcome::SyncRevision {
				job,
				request,
				channel,
				epoch,
				result,
			} => {
				if generation == self.state.generation
					&& self.force_sync.job.as_ref().is_some_and(|j| {
						j.id == *job
							&& j.request == *request
							&& j.targets
								.get(j.channel_index)
								.is_some_and(|t| t.id == *channel)
					}) {
					match result {
						Ok(revision) if self.force_sync_epoch_valid(generation, *epoch) => {
							self.force_sync.revision = Some(*revision)
						}
						_ => {
							if let Some(j) = &mut self.force_sync.job {
								j.record(Stage::History, Outcome::Partial);
								j.details[Job::stage_index(Stage::History)] = 6;
								j.next_channel();
							}
							self.force_sync.revision_requested = false;
						}
					}
				}
				true
			}
			cache::Outcome::SyncPageSaved {
				job,
				request,
				channel,
				epoch,
				result,
			} => {
				if generation == self.state.generation
					&& self.force_sync.job.as_ref().is_some_and(|j| j.id == *job)
					&& self
						.force_sync
						.pending_page
						.as_ref()
						.is_some_and(|p| p.channel == *channel && p.request == *request)
				{
					let page = self.force_sync.pending_page.take().expect("page checked");
					let current_epoch = self.force_sync_epoch_valid(generation, *epoch);
					let j = self.force_sync.job.as_mut().expect("job checked");
					match result {
						_ if !current_epoch => {
							j.record(Stage::History, Outcome::Partial);
							j.details[Job::stage_index(Stage::History)] = 6;
							j.next_channel();
						}
						Ok(saved) if !saved.stale => {
							self.force_sync.stale_retries = 0;
							j.messages_saved += saved.saved as u64;
							if page.done {
								j.record(Stage::History, Outcome::Updated);
								j.next_channel();
							} else {
								j.before = page.next;
							}
						}
						Ok(_) => {
							self.force_sync.stale_retries =
								self.force_sync.stale_retries.saturating_add(1);
							if self.force_sync.stale_retries >= MAX_STALE_RETRIES {
								j.record(Stage::History, Outcome::Partial);
								j.details[Job::stage_index(Stage::History)] = 7;
								j.next_channel();
								self.force_sync.retry_read = None;
							}
						}
						Err(error) => {
							j.record_storage_failure(Stage::History, *error);
							j.next_channel();
						}
					}
					self.force_sync.dirty = true;
				}
				true
			}
			cache::Outcome::SyncMembersSaved {
				job,
				request,
				guild,
				epoch,
				result,
			} => {
				if generation == self.state.generation
					&& self.force_sync.job.as_ref().is_some_and(|current| {
						current.id == *job
							&& matches!(current.step, 7 | 10)
							&& current.guild() == Some(*guild)
					}) && self.force_sync.roster_write == Some((*request, *guild))
				{
					let current_epoch = self.force_sync_epoch_valid(generation, *epoch);
					self.force_sync.roster_write = None;
					self.force_sync.roster_chunk = None;
					if self
						.force_sync
						.job
						.as_ref()
						.is_some_and(|job| job.step == 10)
					{
						let current = self.force_sync.job.as_mut().expect("job checked");
						if result.is_err() || !current_epoch {
							current.record_storage_failure(
								Stage::Members,
								result
									.as_ref()
									.err()
									.copied()
									.unwrap_or(local_store::StoreError::Capacity),
							);
						}
						current.member_index =
							(current.member_index + 100).min(current.members.len());
						self.force_sync.dirty = true;
						return true;
					}
					if result.is_err() || !current_epoch {
						let mut current = self.force_sync.job.take().expect("job checked");
						self.finish_force_sync_roster(&mut current, Some(Failure::Busy));
						current.record_storage_failure(
							Stage::Members,
							result
								.as_ref()
								.err()
								.copied()
								.unwrap_or(local_store::StoreError::Capacity),
						);
						self.force_sync.job = Some(current);
					}
					self.force_sync.dirty = true;
				}
				true
			}
			cache::Outcome::CacheUsage(result) => {
				if let Ok(usage) = result {
					self.force_sync.usage = usage.total;
					self.messaging.force_sync_cache = Some(ui::force_sync::CacheSummary {
						total_bytes: usage.total,
						history_bytes: Some(usage.database),
						metadata_bytes: None,
						media_bytes: Some(usage.media),
						artwork_bytes: Some(usage.artwork),
						last_cleanup: (usage.last_cleanup_reclaimed > 0)
							.then_some("force-sync-cache-cleaned"),
					});
				}
				true
			}
			_ => false,
		}
	}

	pub(super) fn observe_force_sync_event(&mut self, event: &Event) {
		let Some(job) = self.force_sync.job.as_ref() else {
			return;
		};
		let account = job.account;
		let history_target = (job.step == 9)
			.then(|| job.targets.get(job.channel_index).map(|t| t.id))
			.flatten();
		if let Some(active) = &self.force_sync.active
			&& sync_event_affects_read(&active.read, event, &self.state, account)
		{
			if matches!(active.read, Read::History { .. }) {
				self.force_sync.history_revision = self.force_sync.history_revision.wrapping_add(1);
			} else {
				self.force_sync.metadata_revision =
					self.force_sync.metadata_revision.wrapping_add(1);
			}
		}
		self.observe_force_sync_roster_event(event);
		let changed = match event {
			Event::Message(message) => Some((message.channel, message.id)),
			Event::Edited {
				channel,
				message,
				result: Ok(_),
				..
			} => Some((*channel, *message)),
			Event::Patch(patch) => Some((patch.channel, patch.id)),
			Event::Delete { channel, id } => Some((*channel, *id)),
			Event::DeleteBulk { channel, ids } => ids.first().map(|id| (*channel, *id)),
			_ => None,
		};
		if let Some((channel, message)) = changed
			&& history_target == Some(channel)
			&& !self.queue_cache(cache::Operation::ObserveHistoryChange { channel, message })
		{
			// A missing disk fence cannot permit an old page to overwrite a live edit.
			self.force_sync.active = None;
			self.force_sync.revision = None;
			self.force_sync.revision_requested = false;
			if let Some(job) = &mut self.force_sync.job {
				job.record(Stage::History, Outcome::Partial);
				job.next_channel();
			}
			self.force_sync.dirty = true;
		}
		if let Event::ChannelPreferences(finished) = event
			&& self.force_sync.preference == Some(finished.request)
			&& self.force_sync.job.as_ref().is_some_and(|job| {
				finished.generation == job.generation && Some(finished.guild) == job.guild()
			}) {
			if let Some(job) = &mut self.force_sync.job {
				match &finished.result {
					Ok(_) => job.record(Stage::Preferences, Outcome::Updated),
					Err(failure) => job.record_failure(Stage::Preferences, *failure),
				};
				job.advance();
				self.force_sync.dirty = true;
			}
			self.force_sync.preference = None;
			self.force_sync.preference_due = None;
		}
	}

	fn observe_force_sync_roster_event(&mut self, event: &Event) {
		let Some(job) = self.force_sync.job.as_ref().filter(|job| {
			job.step == 7 && job.roster_started
				|| job.step == 10 && self.force_sync.roster_write.is_some()
		}) else {
			return;
		};
		let Some(guild) = job.guild() else {
			return;
		};
		if matches!(event, Event::Disconnected | Event::Resync)
			|| matches!(event,Event::GuildRemoved(id) if *id==guild)
		{
			self.cancel_force_sync_roster();
			return;
		}
		let changed: Vec<Id> = match event {
			Event::MemberIdentity(update) if update.guild == guild => vec![update.user],
			Event::Members(list) if list.guild == Some(guild) => list
				.slots
				.iter()
				.filter_map(|slot| match slot {
					Some(model::MemberSlot::Person(member)) => Some(member.user.id),
					_ => None,
				})
				.take(MAX_TARGETS + 1)
				.collect(),
			Event::MemberSearch {
				request,
				result: Ok(members),
			} if request.guild == guild => members
				.iter()
				.map(|member| member.user.id)
				.take(MAX_TARGETS + 1)
				.collect(),
			_ => return,
		};
		for user in changed {
			if self.force_sync.roster_changed.len() == MAX_TARGETS
				&& !self.force_sync.roster_changed.contains(&user)
			{
				let mut job = self.force_sync.job.take().expect("job checked");
				self.finish_force_sync_roster(&mut job, Some(Failure::ResponseLimit));
				self.force_sync.job = Some(job);
				return;
			}
			self.force_sync.roster_changed.insert(user);
		}
		// If a complete disk batch was already queued, retire its epoch before a newer
		// nickname can be overwritten. A completed write is followed by scoped cleanup.
		let changed_disk = self.force_sync.roster_reconcile.is_some()
			|| self.force_sync.roster_write.is_some()
				&& self.force_sync.roster_chunk.as_ref().is_some_and(
					|(_, members, _)| match event {
						Event::MemberIdentity(update) => members
							.iter()
							.find(|member| member.user.id == update.user)
							.is_some_and(|member| {
								let nick = match &update.nick {
									model::Patch::Value(nick) => {
										member.nick.as_deref() != Some(nick.as_str())
									}
									model::Patch::Null => member.nick.is_some(),
									model::Patch::Absent => false,
								};
								nick || update.departed
									|| update.profile.as_ref().is_some_and(|user| {
										user.name != member.user.name
											|| user.username != member.user.username
									})
							}),
						Event::Members(list) => list.slots.iter().any(|slot| match slot {
							Some(model::MemberSlot::Person(current)) => members
								.iter()
								.find(|member| member.user.id == current.user.id)
								.is_some_and(|old| {
									old.nick != current.nick
										|| old.user.name != current.user.name
										|| old.user.username != current.user.username
								}),
							_ => false,
						}),
						Event::MemberSearch {
							result: Ok(current),
							..
						} => current.iter().any(|current| {
							members
								.iter()
								.find(|member| member.user.id == current.user.id)
								.is_some_and(|old| {
									old.nick != current.nick
										|| old.user.name != current.user.name
										|| old.user.username != current.user.username
								})
						}),
						_ => false,
					},
				);
		if changed_disk {
			let user = match event {
				Event::MemberIdentity(update) => Some(update.user),
				_ => None,
			};
			let account = self.state.user.as_ref().map(|user| user.id);
			if let (Some(cache), Some(account)) = (&self.cache, account) {
				if cache.invalidate_scope(
					self.state.generation,
					account,
					cache::Operation::RemoveMember { guild, user },
				) {
					self.cache_pending += 1;
				} else {
					self.cache_error = true;
					self.cache_status =
						"Could not retire a changed member cache entry; cache reads remain paused";
				}
			}
		}
	}
	fn force_sync_epoch_valid(&self, generation: u64, epoch: u64) -> bool {
		generation == self.state.generation
			&& self
				.cache
				.as_ref()
				.is_some_and(|cache| cache.history.allows(epoch))
	}
	pub(super) fn cancel_force_sync_roster(&mut self) {
		if let Some(connection) = &self.connection {
			connection.sync_roster.cancel();
		}
		self.force_sync.roster_chunk = None;
		self.force_sync.roster_changed.clear();
		self.force_sync.roster_write = None;
		self.force_sync.roster_finished = false;
		self.force_sync.roster_users = None;
		self.force_sync.roster_reconcile = None;
		self.force_sync.roster_attempt = None;
		if let Some(job) = &mut self.force_sync.job {
			job.roster_started = false;
		}
	}
	fn finish_force_sync_roster(&mut self, job: &mut Job, failure: Option<Failure>) {
		let changed = !self.force_sync.roster_changed.is_empty();
		if let Some(failure) = failure {
			job.record_failure(Stage::Members, failure);
			job.roster_complete = false;
		} else {
			job.roster_complete = !changed;
			job.record(
				Stage::Members,
				if changed {
					Outcome::Partial
				} else {
					Outcome::Updated
				},
			);
		}
		job.add_members(self.force_sync.roster_changed.iter().copied());
		self.cancel_force_sync_roster();
		job.roster_started = false;
		job.advance();
		self.force_sync.dirty = true;
	}
	fn drive_force_sync_roster(&mut self, job: &mut Job) {
		let Some(guild) = job.guild() else {
			job.advance();
			return;
		};
		let Some(permit) = self.state.force_sync_roster_permit(guild) else {
			self.cancel_force_sync_roster();
			job.roster_started = false;
			job.roster_complete = false;
			job.record(Stage::Members, Outcome::Partial);
			job.advance();
			self.force_sync.dirty = true;
			return;
		};
		if self.force_sync.roster_write.is_some() {
			return;
		}
		if self.force_sync.roster_reconcile.is_some() {
			return;
		}
		if let Some((chunk_guild, members, index)) = self.force_sync.roster_chunk.take() {
			if chunk_guild != guild {
				self.finish_force_sync_roster(job, Some(Failure::Protocol));
				return;
			}
			let end = (index + 100).min(members.len());
			let accepted: Vec<_> = members[index..end]
				.iter()
				.filter(|member| !self.force_sync.roster_changed.contains(&member.user.id))
				.cloned()
				.collect();
			self.state.apply_force_sync_members(guild, &accepted);
			if end < members.len() {
				self.force_sync.roster_chunk = Some((guild, members, end));
				return;
			}
			let rows = members
				.iter()
				.filter(|member| !self.force_sync.roster_changed.contains(&member.user.id))
				.map(|member| model::MemberDisplay {
					guild,
					user: member.user.id,
					name: member.user.name.clone(),
					username: member.user.username.clone(),
					nick: member.nick.clone(),
				})
				.collect();
			job.request = job.request.wrapping_add(1);
			if self.queue_cache(cache::Operation::SyncSaveMembers {
				job: job.id,
				request: job.request,
				guild,
				rows,
				missing: Vec::new(),
			}) {
				self.force_sync.roster_write = Some((job.request, guild));
				self.force_sync.roster_chunk = Some((guild, members, end));
			} else {
				self.finish_force_sync_roster(job, Some(Failure::Busy));
			}
			return;
		}
		if self.force_sync.roster_finished {
			if !self.force_sync.roster_changed.is_empty() {
				self.finish_force_sync_roster(job, None);
				return;
			}
			let Some(users) = self.force_sync.roster_users.as_ref() else {
				self.finish_force_sync_roster(job, Some(Failure::Protocol));
				return;
			};
			if users.len() > 250_000 || users.binary_search(&job.account).is_err() {
				self.finish_force_sync_roster(job, Some(Failure::Protocol));
				return;
			}
			job.request = job.request.wrapping_add(1);
			if self.queue_cache(cache::Operation::SyncReconcileMembers {
				job: job.id,
				request: job.request,
				guild,
				users: users.clone(),
			}) {
				self.force_sync.roster_reconcile = Some(job.request);
			} else {
				self.finish_force_sync_roster(job, Some(Failure::Busy));
			}
			return;
		}
		if !job.roster_started {
			let Some(connection) = &self.connection else {
				return;
			};
			self.force_sync.roster_sequence =
				self.force_sync.roster_sequence.wrapping_add(1).max(1);
			let attempt = self.force_sync.roster_sequence;
			match connection.sync_roster.start_roster(permit, attempt) {
				Ok(()) => {
					job.roster_started = true;
					self.force_sync.roster_attempt = Some(attempt);
					self.force_sync.roster_changed.clear();
					self.force_sync.dirty = true;
				}
				Err(failure) => self.finish_force_sync_roster(job, Some(failure)),
			}
			return;
		}
		let event = self
			.connection
			.as_ref()
			.and_then(|connection| connection.sync_roster.try_recv());
		let Some(event) = event else {
			return;
		};
		use discord_gateway::roster::Event as R;
		let request = match &event {
			R::Chunk { request, .. } | R::Failed { request, .. } => *request,
			R::Finished { request, .. } => *request,
		};
		if request.guild != guild || Some(request.job) != self.force_sync.roster_attempt {
			return;
		}
		match event {
			R::Chunk { members, .. } => {
				self.force_sync.roster_chunk = Some((guild, members, 0));
			}
			R::Finished { users, .. } => {
				self.force_sync.roster_finished = true;
				self.force_sync.roster_users = Some(users);
			}
			R::Failed { failure, .. } => self.finish_force_sync_roster(job, Some(failure)),
		}
	}
	fn restore_force_sync_continuation(&mut self, job: &mut Job) {
		let Some(saved) = self.force_sync.resume.take() else {
			return;
		};
		if saved.id != job.id
			|| saved.account != job.account
			|| saved.generation != job.generation
			|| saved.guild() != job.guild()
			|| saved.guild_index != job.guild_index
			|| saved.step < 3
			|| !saved.valid()
		{
			return;
		}
		let mut saved = *saved;
		let archive_target = saved
			.targets
			.get(saved.archive_index)
			.map(|target| target.id);
		let target = saved
			.targets
			.get(saved.channel_index)
			.map(|target| target.id);
		let mut next_index = 0;
		let guild = saved.guild();
		let retained = saved
			.targets
			.iter()
			.enumerate()
			.filter(|(_, target)| target.readable(&self.state, guild))
			.map(|(index, target)| {
				if index < saved.channel_index {
					next_index += 1;
				}
				target.clone()
			})
			.collect();
		saved.targets = retained;
		saved.channel_index = next_index.min(saved.targets.len());
		if target
			!= saved
				.targets
				.get(saved.channel_index)
				.map(|target| target.id)
		{
			saved.before = last_before(saved.end_ms);
			saved.record(Stage::History, Outcome::Partial);
		}
		saved.archive_index = archive_target
			.and_then(|id| saved.targets.iter().position(|target| target.id == id))
			.unwrap_or_else(|| {
				saved.archive_time = None;
				saved.archive_id = None;
				saved.archive_kind = 0;
				saved.archive_index.min(saved.targets.len())
			});
		saved.roster_started = false;
		saved.request = saved.request.max(job.request);
		for index in 0..3 {
			if job.outcomes[index] > saved.outcomes[index] {
				saved.outcomes[index] = job.outcomes[index];
				saved.details[index] = job.details[index];
			}
		}
		*job = saved;
		self.force_sync.dirty = true;
	}
}

/// Only changes to the resource being read can invalidate its response. Continuous
/// traffic in another channel must not prevent background history from making progress.
fn sync_event_affects_read(
	read: &Read,
	event: &Event,
	state: &client_core::State,
	account: Id,
) -> bool {
	let channel = match read {
		Read::History { channel, .. } => Some(*channel),
		_ => None,
	};
	let guild = read.guild().or_else(|| {
		channel
			.and_then(|id| state.channel(id))
			.and_then(|channel| channel.guild)
	});
	match event {
		Event::Message(message) => channel == Some(message.channel),
		Event::Edited {
			channel: changed,
			result: Ok(_),
			..
		}
		| Event::Delete {
			channel: changed, ..
		}
		| Event::DeleteBulk {
			channel: changed, ..
		} => channel == Some(*changed),
		Event::Patch(patch) => channel == Some(patch.channel),
		Event::GuildRemoved(changed) => guild == Some(*changed),
		Event::GuildChanged(patch) => matches!(read, Read::Guild { .. }) && guild == Some(patch.id),
		Event::GuildEmojis { guild: changed, .. } => {
			matches!(read, Read::Emojis { .. }) && guild == Some(*changed)
		}
		Event::GuildStickers { guild: changed, .. } => {
			matches!(read, Read::Stickers { .. }) && guild == Some(*changed)
		}
		Event::Soundboard(client_core::soundboard::Event::Changed(changed)) => {
			matches!(read, Read::Sounds { .. }) && guild == Some(*changed)
		}
		Event::MemberIdentity(update) => {
			guild == Some(update.guild)
				&& match read {
					Read::Guild { .. } => update.user == account,
					Read::Members { users, .. } => users.contains(&update.user),
					_ => false,
				}
		}
		Event::Members(list) => matches!(read, Read::Members { .. }) && guild == list.guild,
		Event::PermissionsChanged => matches!(
			read,
			Read::Guild { .. } | Read::Channels { .. } | Read::History { .. }
		),
		Event::Permissions(update)
			if matches!(
				read,
				Read::Guild { .. } | Read::Channels { .. } | Read::History { .. }
			) =>
		{
			use client_core::permissions::Event as P;
			match update {
				P::Snapshot(snapshot) => {
					snapshot.guilds.iter().any(|g| Some(g.id) == guild)
						|| snapshot
							.channels
							.iter()
							.any(|c| Some(c.id) == channel || Some(c.guild) == guild)
				}
				P::Guild(value) => guild == Some(value.id),
				P::Role { guild: changed, .. }
				| P::RoleRemoved { guild: changed, .. }
				| P::Member { guild: changed, .. }
				| P::Owner { guild: changed, .. }
				| P::UnavailableGuild(changed) => guild == Some(*changed),
				P::Members(updates) => updates
					.iter()
					.any(|(changed, _, _)| guild == Some(*changed)),
				P::Channel {
					channel: changed,
					guild: changed_guild,
					..
				} => {
					if channel.is_some() {
						channel == Some(*changed)
					} else {
						guild.is_some() && guild == *changed_guild
					}
				}
			}
		}
		Event::ChannelCreated(changed) | Event::ChannelRestored(changed) => {
			matches!(
				read,
				Read::Channels { .. } | Read::ActiveThreads { .. } | Read::Archives { .. }
			) && guild.is_some()
				&& guild == changed.guild
		}
		Event::ChannelChanged(changed) => {
			channel == Some(changed.id)
				|| (matches!(
					read,
					Read::Channels { .. } | Read::ActiveThreads { .. } | Read::Archives { .. }
				) && state
					.channel(changed.id)
					.is_some_and(|c| guild.is_some() && c.guild == guild))
		}
		Event::ThreadChanged { guild: changed, .. }
		| Event::ThreadRemoved { guild: changed, .. }
		| Event::ThreadsSync { guild: changed, .. } => {
			matches!(read, Read::ActiveThreads { .. } | Read::Archives { .. })
				&& guild == Some(*changed)
		}
		Event::Unavailable(changed) => {
			channel == Some(*changed)
				|| (matches!(read, Read::Channels { .. })
					&& state
						.channel(*changed)
						.is_some_and(|c| guild.is_some() && c.guild == guild))
		}
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn job_identity_does_not_repeat_within_one_millisecond_or_after_clock_reversal() {
		let first = next_job_id(42).unwrap();
		let second = next_job_id(42).unwrap();
		let reversed = next_job_id(1).unwrap();
		assert!(first < second && second < reversed);
	}
	#[test]
	fn checkpoint_is_bounded_and_does_not_serialize_live_permissions() {
		let state = test_support::demo_state();
		let job = Job::new(&state, Scope::Account).unwrap();
		assert!(job.valid());
		let bytes = serde_json::to_vec(&job).unwrap();
		assert!(bytes.len() < MAX_CHECKPOINT);
		assert!(!String::from_utf8_lossy(&bytes).contains("permissions"));
		assert!(serde_json::from_slice::<Job>(&bytes).unwrap().valid());
	}
	#[test]
	fn partial_outcomes_survive_later_success() {
		let state = test_support::demo_state();
		let mut job = Job::new(&state, Scope::Account).unwrap();
		job.record(Stage::Members, Outcome::Partial);
		job.record(Stage::Members, Outcome::Updated);
		assert_eq!(job.outcomes[Job::stage_index(Stage::Members)], 2);
	}
	#[test]
	fn targets_are_deduplicated_without_replacing_the_time_range() {
		let state = test_support::demo_state();
		let mut job = Job::new(&state, Scope::Account).unwrap();
		let cutoff = job.cutoff_ms;
		job.add_targets([
			Target {
				id: Id(5),
				kind: 0,
				parent: None,
			},
			Target {
				id: Id(5),
				kind: 0,
				parent: None,
			},
		]);
		assert_eq!(job.targets.iter().filter(|t| t.id == Id(5)).count(), 1);
		assert_eq!(job.end_ms - cutoff, HISTORY_MILLIS);
	}
	#[test]
	fn checkpoint_rejects_scope_cursor_duplicates_and_invalid_archive_position() {
		let state = test_support::demo_state();
		let first = state.guilds[0].id;
		let mut job = Job::new(&state, Scope::Server(first)).unwrap();
		assert!(job.valid());
		job.scope = Some(Id(u64::MAX));
		assert!(!job.valid());
		job.scope = Some(first);
		job.guilds.push(first);
		assert!(!job.valid());
		job.guilds.pop();
		job.before = Some(Id(u64::MAX));
		assert!(!job.valid());
		job.before = None;
		job.archive_index = 1;
		assert!(!job.valid());
		job.archive_index = 0;
		job.archive_kind = 2;
		assert!(!job.valid());
		job.archive_kind = 0;
		job.dms.push(Id(9));
		assert!(!job.valid());
		job.dms.clear();
		assert!(job.valid());
	}
	#[test]
	fn unrelated_live_traffic_cannot_starve_a_history_or_expression_read() {
		let state = test_support::demo_state();
		let channel = state.selected.unwrap();
		let guild = state.channel(channel).unwrap().guild.unwrap();
		let own = state.user.as_ref().unwrap().id;
		let read = Read::History {
			channel,
			before: None,
			cutoff_ms: client_core::force_sync_read::DISCORD_EPOCH_MS,
			end_ms: client_core::force_sync_read::DISCORD_EPOCH_MS + HISTORY_MILLIS,
		};
		assert!(!sync_event_affects_read(
			&read,
			&Event::Message(test_support::message(5, Id(987654321))),
			&state,
			own
		));
		assert!(sync_event_affects_read(
			&read,
			&Event::Message(test_support::message(5, channel)),
			&state,
			own
		));
		let update = Event::MemberIdentity(Box::new(model::MemberIdentity {
			avatar: model::Patch::Absent,
			guild,
			user: Id(987654321),
			profile: None,
			nick: model::Patch::Value("changed".into()),
			roles: model::Patch::Absent,
			departed: false,
		}));
		assert!(!sync_event_affects_read(
			&Read::Emojis { guild },
			&update,
			&state,
			own
		));
		assert!(!sync_event_affects_read(
			&Read::Guild { guild, user: own },
			&update,
			&state,
			own
		));
		assert!(sync_event_affects_read(
			&Read::Members {
				guild,
				users: vec![Id(987654321)]
			},
			&update,
			&state,
			own
		));
	}
	#[test]
	fn stale_reads_and_transient_retries_are_bounded_and_hidden_completed_jobs_do_not_poll() {
		let state = test_support::demo_state();
		let mut job = Job::new(&state, Scope::Account).unwrap();
		let read = Read::Emojis {
			guild: job.guild().unwrap(),
		};
		job.step = 3;
		let mut sync = Sync::default();
		sync.prepare_read(&read);
		for _ in 0..MAX_STALE_RETRIES {
			sync.reject_stale(&mut job, &read);
		}
		assert_eq!(job.step, 4);
		assert_eq!(job.outcomes[Job::stage_index(Stage::Expressions)], 2);
		let now = Instant::now();
		sync.read_retries = 0;
		assert!(sync.defer_read(Failure::RateLimited, now));
		assert_eq!(sync.next_read, Some(now + Duration::from_secs(60)));
		assert!(sync.defer_read(Failure::RateLimited, now));
		assert_eq!(sync.next_read, Some(now + Duration::from_secs(120)));
		assert!(!sync.defer_read(Failure::RateLimited, now));
		job.status = 3;
		sync.job = Some(job);
		assert!(!sync.should_measure_usage(false));
		assert!(sync.should_measure_usage(true));
	}
	#[test]
	fn approved_thread_targets_use_parent_access_without_overriding_known_denials() {
		let mut state = test_support::demo_state();
		let parent = state.selected.unwrap();
		let guild = state.channel(parent).unwrap().guild;
		assert!(state.can_read_history(parent));
		let target = Target {
			id: Id(7654321),
			kind: 11,
			parent: Some(parent),
		};
		assert!(target.readable(&state, guild));
		assert!(!target.readable(&state, Some(Id(u64::MAX))));
		let mut thread = state.channel(parent).unwrap().clone();
		thread.id = target.id;
		thread.parent_id = Some(parent);
		thread.kind = 11;
		state.channels.push(thread);
		// A known channel's own current permissions take precedence over parent fallback.
		assert_eq!(
			target.readable(&state, guild),
			state.can_read_history(target.id)
		);
	}
	#[test]
	fn lost_history_access_reports_unavailable_and_completes_its_progress_slot() {
		let state = test_support::demo_state();
		let channel = state.channel(state.selected.unwrap()).unwrap();
		let mut job = Job::new(&state, Scope::Server(channel.guild.unwrap())).unwrap();
		job.targets = vec![
			Target {
				id: Id(87654321),
				kind: 0,
				parent: None,
			},
			Target {
				id: Id(87654322),
				kind: 4,
				parent: None,
			},
			Target {
				id: channel.id,
				kind: channel.kind,
				parent: channel.parent_id,
			},
		];
		job.step = 8;
		job.advance();
		assert_eq!(job.channels_total, 2);
		job.skip_unreadable_targets(&state);
		assert_eq!(job.channels_done, 1);
		assert_eq!(job.channel_index, 2);
		assert_eq!(job.outcomes[Job::stage_index(Stage::History)], 3);
		assert_eq!(job.details[Job::stage_index(Stage::History)], 1);
		assert!(job.valid());
	}
}
