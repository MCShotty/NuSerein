//! Durable, bounded history pages and resumable Force Sync checkpoints.
//! None of these records establishes current membership or channel permissions.
use super::{Id, LocalStore, Message, OptionalExtension, Result, StoreError, params};
use rusqlite::Transaction;

pub const MAX_CACHED_MESSAGES: usize = 1_000_000;
const MAX_TOMBSTONES: usize = 100_000;
const MAX_CHECKPOINT_BYTES: usize = 1024 * 1024;
const MAX_HISTORY_SCOPES: i64 = 1_000_000;
// A production schema31 file was capped at64MiB. Opening it can add bounded
// schema32 tables/indexes before the worker reserves its layout conversion.
const MAX_LEGACY_LAYOUT_BYTES: u64 = 96 * 1024 * 1024;
type StoredCoverage = (String, String, String, String, Option<String>, bool, bool);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryPageQuery {
	pub before: Option<Id>,
	pub around: Option<Id>,
	pub limit: usize,
}
impl Default for HistoryPageQuery {
	fn default() -> Self {
		Self {
			before: None,
			around: None,
			limit: 500,
		}
	}
}

/// The response proves only the interval from its oldest row to its exclusive
/// `before` cursor. `complete` proves the remaining interval down to `cutoff`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryCoverage {
	pub job: u64,
	pub request: u64,
	pub cutoff: Id,
	pub upper: Id,
	pub before: Option<Id>,
	pub next_before: Option<Id>,
	pub complete: bool,
	/// Storage revision observed before transmitting the corresponding HTTP read.
	pub revision: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncPageResult {
	pub saved: usize,
	pub stale: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncCheckpoint {
	pub job: u64,
	/// None denotes an account-wide job.
	pub scope: Option<Id>,
	pub started_ms: i64,
	pub cutoff_ms: i64,
	/// A versioned orchestration projection, never a credential or media body.
	pub payload: Vec<u8>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryCacheStats {
	pub messages: u64,
	pub channels: u64,
	pub database_bytes: u64,
}

impl LocalStore {
	pub fn history_revision(&self, account: Id, channel: Id) -> Result<u64> {
		if account.0 == 0 || channel.0 == 0 {
			return Err(StoreError::Capacity);
		}
		let revision: Option<i64> = self
			.0
			.query_row(
				"SELECT revision FROM history_channels WHERE account=?1 AND channel=?2",
				params![account.to_string(), channel.to_string()],
				|row| row.get(0),
			)
			.optional()?;
		u64::try_from(revision.unwrap_or(0)).map_err(|_| StoreError::Incompatible)
	}
	/// Record a live change before delayed background reads may be committed.
	/// Edits retain their current body; deletions atomically remove it and retain a tombstone.
	pub fn observe_history_change(
		&mut self,
		account: Id,
		channel: Id,
		message: Id,
		deleted: bool,
	) -> Result<u64> {
		let transaction = self.0.transaction()?;
		let revision = observe_change(&transaction, account, channel, message, deleted)?;
		transaction.commit()?;
		Ok(revision)
	}
	pub fn save_sync_page(
		&mut self,
		account: Id,
		channel: Id,
		messages: &[Message],
		coverage: &HistoryCoverage,
	) -> Result<SyncPageResult> {
		self.save_sync_page_scoped(account, channel, messages, coverage, None, None)
	}
	/// A history page retains its server/parent cleanup scope even when an archived
	/// thread was never installed in the active navigation snapshot.
	pub fn save_sync_page_scoped(
		&mut self,
		account: Id,
		channel: Id,
		messages: &[Message],
		coverage: &HistoryCoverage,
		guild: Option<Id>,
		parent: Option<Id>,
	) -> Result<SyncPageResult> {
		if account.0 == 0
			|| channel.0 == 0
			|| guild.is_some_and(|id| id.0 == 0)
			|| parent.is_some_and(|id| id.0 == 0 || id == channel || guild.is_none())
			|| coverage.job == 0
			|| coverage.request == 0
			|| coverage.cutoff.0 == 0
			|| coverage.upper < coverage.cutoff
			|| messages.len() > 50
			|| messages.iter().map(Message::bytes).sum::<usize>() > super::MAX_WINDOW_BYTES
			|| messages.iter().any(|message| {
				message.channel != channel
					|| message.id.0 == 0
					|| message.id > coverage.upper
					|| coverage.before.is_some_and(|before| message.id >= before)
			}) || messages.iter().enumerate().any(|(index, message)| {
			messages[..index]
				.iter()
				.any(|previous| previous.id == message.id)
		}) || coverage
			.before
			.is_some_and(|before| before <= coverage.cutoff)
			|| (!coverage.complete
				&& (messages.len() != 50
					|| coverage.next_before != messages.iter().map(|message| message.id).min()))
			|| coverage.complete
				&& (coverage.next_before.is_some()
					|| messages.len() == 50
						&& messages.iter().all(|message| message.id > coverage.cutoff))
		{
			return Err(StoreError::Capacity);
		}
		// Job IDs are opaque identities, never chronological clocks. A persisted active
		// checkpoint authorizes this exact frozen interval; cancellation or a new job
		// retires even an otherwise idempotent queued page from the previous owner.
		const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
		let cutoff_ms = (coverage.cutoff.0 >> 22) as i64 + DISCORD_EPOCH_MS;
		let end_ms = (coverage.upper.0.saturating_sub(1) >> 22) as i64 + DISCORD_EPOCH_MS;
		let active:bool=self.0.query_row("SELECT EXISTS(SELECT 1 FROM sync_checkpoints WHERE account=?1 AND job=?2 AND started_ms=?3 AND cutoff_ms=?4 AND (?5 IS NULL OR scope IS NULL OR scope=?5))",params![account.to_string(),coverage.job.to_string(),end_ms,cutoff_ms,guild.map(|id|id.to_string())],|row|row.get(0))?;
		if !active {
			return Ok(SyncPageResult {
				saved: 0,
				stale: true,
			});
		}
		let revision = self.history_revision(account, channel)?;
		let prior: Option<StoredCoverage> = self.0.query_row(
			"SELECT job,request,cutoff,upper_id,next_before,evicted,complete FROM history_sync WHERE account=?1 AND channel=?2",
			params![account.to_string(),channel.to_string()],
			read_stored_coverage,
		).optional()?;
		if let Some((job, request, cutoff, upper, next, evicted, complete)) = prior {
			if job == coverage.job.to_string()
				&& request == coverage.request.to_string()
				&& cutoff == coverage.cutoff.to_string()
				&& upper == coverage.upper.to_string()
				&& next == coverage.next_before.map(|id| id.to_string())
				&& complete == coverage.complete
				&& !evicted
			{
				// The transaction reached disk before a crash but its orchestration checkpoint
				// did not. Acknowledge without replaying or overwriting newer live data.
				return Ok(SyncPageResult {
					saved: 0,
					stale: false,
				});
			}
			if job == coverage.job.to_string()
				&& cutoff == coverage.cutoff.to_string()
				&& upper == coverage.upper.to_string()
				&& (complete
					|| request
						.parse::<u64>()
						.map_err(|_| StoreError::Incompatible)?
						>= coverage.request
					|| next != coverage.before.map(|id| id.to_string())
					|| evicted)
			{
				return Ok(SyncPageResult {
					stale: true,
					saved: 0,
				});
			}
		}
		if revision != coverage.revision {
			return Ok(SyncPageResult {
				stale: true,
				saved: 0,
			});
		}
		let retained: Vec<_> = messages
			.iter()
			.filter(|message| message.id >= coverage.cutoff)
			.collect();
		let existing = self.load_history_page(
			account,
			channel,
			HistoryPageQuery {
				before: coverage.before,
				around: None,
				limit: 50,
			},
		)?;
		// Only an accepted, complete page proves absence in this interval. Failed or
		// truncated reads never reach here, and live changes fence the whole response.
		let prune = if let Some(lower) = if coverage.complete {
			Some(coverage.cutoff)
		} else {
			retained.iter().map(|message| message.id).min()
		} {
			let upper = coverage
				.before
				.map(|id| Id(id.0.saturating_sub(1)))
				.unwrap_or(coverage.upper)
				.min(coverage.upper);
			let keep = retained
				.iter()
				.map(|message| message.id.to_string())
				.collect::<Vec<_>>();
			let mut sql = "DELETE FROM messages WHERE account=?1 AND channel=?2 AND (length(id),id)>=(length(?3),?3) AND (length(id),id)<=(length(?4),?4)".to_owned();
			if !keep.is_empty() {
				sql.push_str(" AND id NOT IN (");
				for index in 0..keep.len() {
					if index > 0 {
						sql.push(',');
					}
					sql.push_str(&format!("?{}", index + 5));
				}
				sql.push(')');
			}
			let mut values = vec![
				account.to_string(),
				channel.to_string(),
				lower.to_string(),
				upper.to_string(),
			];
			values.extend(keep);
			self.prune_large_verified_range(account, channel, &sql, &values)?;
			Some((sql, values))
		} else {
			None
		};
		let transaction = self.0.transaction()?;
		if let Some((sql, values)) = prune {
			transaction.execute(&sql, rusqlite::params_from_iter(values))?;
		}
		let saved = super::history::write_messages(
			&transaction,
			account,
			channel,
			&retained,
			&existing,
			false,
		)?;
		// Unscoped legacy callers cannot erase a known archived-parent relationship.
		transaction.execute("INSERT INTO history_scopes(account,channel,guild,parent) VALUES(?1,?2,?3,?4) ON CONFLICT(account,channel) DO UPDATE SET guild=CASE WHEN excluded.guild IS NULL THEN history_scopes.guild ELSE excluded.guild END,parent=CASE WHEN excluded.guild IS NULL THEN history_scopes.parent ELSE excluded.parent END",params![account.to_string(),channel.to_string(),guild.map(|id|id.to_string()),parent.map(|id|id.to_string())])?;
		let scopes: i64 = transaction.query_row(
			"SELECT count FROM history_scope_totals WHERE singleton=1",
			[],
			|row| row.get(0),
		)?;
		if scopes > MAX_HISTORY_SCOPES {
			return Err(StoreError::Capacity);
		}
		transaction.execute("INSERT INTO history_sync(account,channel,job,request,cutoff,upper_id,next_before,complete,evicted) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0) ON CONFLICT(account,channel) DO UPDATE SET job=excluded.job,request=excluded.request,cutoff=excluded.cutoff,upper_id=excluded.upper_id,next_before=excluded.next_before,complete=excluded.complete,evicted=0", params![account.to_string(),channel.to_string(),coverage.job.to_string(),coverage.request.to_string(),coverage.cutoff.to_string(),coverage.upper.to_string(),coverage.next_before.map(|id|id.to_string()),coverage.complete])?;
		transaction.commit()?;
		Ok(SyncPageResult {
			saved,
			stale: false,
		})
	}
	/// Large confirmed absence is retired in bounded transactions before the final
	/// page/cursor commit. A crash leaves the previous cursor and partial coverage,
	/// so the same verified page is safely fetched again on Resume.
	fn prune_large_verified_range(
		&mut self,
		account: Id,
		channel: Id,
		delete_sql: &str,
		values: &[String],
	) -> Result<()> {
		let charge = self.cache_row_charge("messages", None)?;
		let select = delete_sql.replacen(
			"DELETE FROM messages",
			&format!("SELECT id,{charge} FROM messages"),
			1,
		) + " ORDER BY length(id),id LIMIT 129";
		loop {
			let mut statement = self.0.prepare_cached(&select)?;
			let rows = statement
				.query_map(rusqlite::params_from_iter(values), |row| {
					Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
				})?
				.collect::<rusqlite::Result<Vec<_>>>()?;
			drop(statement);
			if rows.iter().any(|(_, charge)| *charge < 0) {
				return Err(StoreError::Incompatible);
			}
			let total = rows
				.iter()
				.fold(0i64, |total, (_, charge)| total.saturating_add(*charge));
			if rows.len() <= 128 && total <= crate::cache_cleanup::CLEANUP_BYTES {
				return Ok(());
			}
			let mut bytes = 0i64;
			let transaction = self.0.transaction()?;
			for (index, (id, charge)) in rows.iter().enumerate() {
				if index == 128
					|| index > 0
						&& bytes.saturating_add(*charge) > crate::cache_cleanup::CLEANUP_BYTES
				{
					break;
				}
				transaction.execute(
					"DELETE FROM messages WHERE account=?1 AND channel=?2 AND id=?3",
					params![account.to_string(), channel.to_string(), id],
				)?;
				bytes = bytes.saturating_add(*charge);
			}
			transaction.execute(
				"UPDATE history_sync SET complete=0 WHERE account=?1 AND channel=?2",
				params![account.to_string(), channel.to_string()],
			)?;
			transaction.commit()?;
			self.reclaim_history_space()?;
		}
	}
	pub fn history_coverage(&self, account: Id, channel: Id) -> Result<Option<HistoryCoverage>> {
		let record: Option<StoredCoverage> = self.0.query_row("SELECT job,request,cutoff,upper_id,next_before,complete,evicted FROM history_sync WHERE account=?1 AND channel=?2",params![account.to_string(),channel.to_string()],read_stored_coverage).optional()?;
		record
			.map(|(job, request, cutoff, upper, next, complete, evicted)| {
				Ok(HistoryCoverage {
					job: job.parse().map_err(|_| StoreError::Incompatible)?,
					request: request.parse().map_err(|_| StoreError::Incompatible)?,
					cutoff: cutoff.parse().map_err(|_| StoreError::Incompatible)?,
					upper: upper.parse().map_err(|_| StoreError::Incompatible)?,
					before: None,
					next_before: next
						.map(|id| id.parse())
						.transpose()
						.map_err(|_| StoreError::Incompatible)?,
					complete: complete && !evicted,
					revision: self.history_revision(account, channel)?,
				})
			})
			.transpose()
	}
	pub fn save_sync_checkpoint(&mut self, account: Id, checkpoint: &SyncCheckpoint) -> Result<()> {
		if account.0 == 0
			|| checkpoint.job == 0
			|| checkpoint.scope.is_some_and(|id| id.0 == 0)
			|| checkpoint.started_ms < checkpoint.cutoff_ms
			|| checkpoint.cutoff_ms < 0
			|| checkpoint.payload.len() > MAX_CHECKPOINT_BYTES
		{
			return Err(StoreError::Capacity);
		}
		let transaction = self.0.transaction()?;
		transaction.execute("INSERT INTO sync_checkpoints(account,job,scope,started_ms,cutoff_ms,payload) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account) DO UPDATE SET job=excluded.job,scope=excluded.scope,started_ms=excluded.started_ms,cutoff_ms=excluded.cutoff_ms,payload=excluded.payload",params![account.to_string(),checkpoint.job.to_string(),checkpoint.scope.map(|id|id.to_string()),checkpoint.started_ms,checkpoint.cutoff_ms,checkpoint.payload])?;
		let (count, bytes): (i64, i64) = transaction.query_row(
			"SELECT count(*),coalesce(sum(length(payload)),0) FROM sync_checkpoints",
			[],
			|row| Ok((row.get(0)?, row.get(1)?)),
		)?;
		if count > 16 || bytes > 16 * MAX_CHECKPOINT_BYTES as i64 {
			return Err(StoreError::Capacity);
		}
		transaction.commit()?;
		Ok(())
	}
	pub fn load_sync_checkpoint(&self, account: Id) -> Result<Option<SyncCheckpoint>> {
		let mut statement = self.0.prepare_cached(
			"SELECT job,scope,started_ms,cutoff_ms,payload FROM sync_checkpoints WHERE account=?1",
		)?;
		let mut rows = statement.query([account.to_string()])?;
		let Some(row) = rows.next()? else {
			return Ok(None);
		};
		for column in 0..=1 {
			let field = row.get_ref(column)?;
			if matches!(field, rusqlite::types::ValueRef::Null) && column == 1 {
				continue;
			}
			let field = field.as_str().map_err(|_| StoreError::Incompatible)?;
			if field.is_empty()
				|| field.len() > 20
				|| !field.bytes().all(|byte| byte.is_ascii_digit())
			{
				return Err(StoreError::Incompatible);
			}
		}
		let payload = row
			.get_ref(4)?
			.as_blob()
			.map_err(|_| StoreError::Incompatible)?;
		if payload.len() > MAX_CHECKPOINT_BYTES {
			return Err(StoreError::Capacity);
		}
		Ok(Some(SyncCheckpoint {
			job: row
				.get::<_, String>(0)?
				.parse()
				.map_err(|_| StoreError::Incompatible)?,
			scope: row
				.get::<_, Option<String>>(1)?
				.map(|id| id.parse())
				.transpose()
				.map_err(|_| StoreError::Incompatible)?,
			started_ms: row.get(2)?,
			cutoff_ms: row.get(3)?,
			payload: payload.to_vec(),
		}))
	}
	pub fn clear_sync_checkpoint(&mut self, account: Id) -> Result<()> {
		self.0.execute(
			"DELETE FROM sync_checkpoints WHERE account=?1",
			[account.to_string()],
		)?;
		Ok(())
	}
	pub fn history_cache_stats(&self) -> Result<HistoryCacheStats> {
		let messages: i64 = self.0.query_row(
			"SELECT count FROM history_totals WHERE singleton=1",
			[],
			|row| row.get(0),
		)?;
		let channels: i64 = self
			.0
			.query_row("SELECT count(*) FROM channels", [], |row| row.get(0))?;
		let pages: i64 = self
			.0
			.pragma_query_value(None, "page_count", |row| row.get(0))?;
		let page_size: i64 = self
			.0
			.pragma_query_value(None, "page_size", |row| row.get(0))?;
		Ok(HistoryCacheStats {
			messages: u64::try_from(messages).map_err(|_| StoreError::Incompatible)?,
			channels: u64::try_from(channels).map_err(|_| StoreError::Incompatible)?,
			database_bytes: u64::try_from(pages.saturating_mul(page_size))
				.map_err(|_| StoreError::Incompatible)?,
		})
	}
	/// Worker-only maintenance, bounded to at most 4096 victims per pass.
	pub fn evict_history_before(&mut self, cutoff: Id, limit: usize) -> Result<usize> {
		self.evict_history(Some(cutoff), limit, &[])
	}
	/// Maintenance must preserve channels with active readers or writes even for old rows.
	pub fn evict_history_before_protected(
		&mut self,
		cutoff: Id,
		limit: usize,
		protected: &[(Id, Id)],
	) -> Result<usize> {
		self.evict_history(Some(cutoff), limit, protected)
	}
	/// Bounded exact lookups used only when a batch approaches the global item ceiling.
	pub fn missing_history_messages(&self, account: Id, channel: Id, ids: &[Id]) -> Result<usize> {
		if account.0 == 0 || channel.0 == 0 || ids.len() > 500 || ids.iter().any(|id| id.0 == 0) {
			return Err(StoreError::Capacity);
		}
		let mut query = self.0.prepare_cached(
			"SELECT EXISTS(SELECT 1 FROM messages WHERE account=?1 AND channel=?2 AND id=?3) OR EXISTS(SELECT 1 FROM history_tombstones WHERE account=?1 AND channel=?2 AND id=?3)",
		)?;
		let mut seen = std::collections::BTreeSet::new();
		let mut missing = 0;
		for id in ids {
			if !seen.insert(*id) {
				continue;
			}
			let exists: bool = query.query_row(
				params![account.to_string(), channel.to_string(), id.to_string()],
				|row| row.get(0),
			)?;
			missing += usize::from(!exists);
		}
		Ok(missing)
	}
	pub fn evict_oldest_history(&mut self, limit: usize, protected: &[(Id, Id)]) -> Result<usize> {
		self.evict_history(None, limit, protected)
	}
	fn evict_history(
		&mut self,
		cutoff: Option<Id>,
		limit: usize,
		protected: &[(Id, Id)],
	) -> Result<usize> {
		if limit == 0 || limit > 4096 || protected.len() > 16 {
			return Err(StoreError::Capacity);
		}
		let charge = self.cache_row_charge("messages", Some("m"))?;
		let mut sql = format!(
			"SELECT m.account,m.channel,m.id,{charge} FROM messages m LEFT JOIN channels c ON m.account=c.account AND m.channel=c.channel WHERE (?1 IS NULL OR (length(m.id),m.id)<(length(?1),?1))"
		);
		let mut values = vec![cutoff.map(|id| id.to_string())];
		for (index, (account, channel)) in protected.iter().enumerate() {
			sql.push_str(&format!(
				" AND NOT(m.account=?{} AND m.channel=?{})",
				index * 2 + 2,
				index * 2 + 3
			));
			values.push(Some(account.to_string()));
			values.push(Some(channel.to_string()));
		}
		sql.push_str(&format!(
			" ORDER BY coalesce(c.touched,0),length(m.id),m.id LIMIT {limit}"
		));
		let mut statement = self.0.prepare(&sql)?;
		let mut rows = statement.query(rusqlite::params_from_iter(values))?;
		let mut victims = Vec::new();
		let mut bytes = 0i64;
		while let Some(row) = rows.next()? {
			let charge: i64 = row.get(3)?;
			if charge < 0 {
				return Err(StoreError::Incompatible);
			}
			if !victims.is_empty()
				&& bytes.saturating_add(charge) > crate::cache_cleanup::CLEANUP_BYTES
			{
				break;
			}
			bytes = bytes.saturating_add(charge);
			victims.push((
				row.get::<_, String>(0)?,
				row.get::<_, String>(1)?,
				row.get::<_, String>(2)?,
			));
		}
		drop(rows);
		drop(statement);
		let transaction = self.0.transaction()?;
		let mut channels = std::collections::BTreeSet::new();
		for (account, channel, id) in &victims {
			transaction.execute(
				"DELETE FROM messages WHERE account=?1 AND channel=?2 AND id=?3",
				params![account, channel, id],
			)?;
			channels.insert((account, channel));
		}
		for (account, channel) in channels {
			transaction.execute(
				"UPDATE history_sync SET complete=0,evicted=1 WHERE account=?1 AND channel=?2",
				params![account, channel],
			)?;
			transaction.execute(
				"DELETE FROM history_starts WHERE account=?1 AND channel=?2",
				params![account, channel],
			)?;
		}
		transaction.commit()?;
		Ok(victims.len())
	}
	/// Drop only presentation hints, after media and history reclamation have been tried.
	pub fn evict_presentation_hints(
		&mut self,
		limit: usize,
		protected_guilds: &[(Id, Id)],
	) -> Result<usize> {
		if limit == 0 || limit > 4096 || protected_guilds.len() > 16 {
			return Err(StoreError::Capacity);
		}
		let mut victims = Vec::new();
		let mut bytes = 0i64;
		for table in ["member_displays", "expression_catalogs", "server_displays"] {
			let charge = self.cache_row_charge(table, None)?;
			let mut sql = format!("SELECT rowid,{charge} FROM {table} WHERE 1=1");
			let mut values = Vec::new();
			for (index, (account, guild)) in protected_guilds.iter().enumerate() {
				sql.push_str(&format!(
					" AND NOT(account=?{} AND guild=?{})",
					index * 2 + 1,
					index * 2 + 2
				));
				values.push(account.to_string());
				values.push(guild.to_string());
			}
			sql.push_str(&format!(
				" ORDER BY touched LIMIT {}",
				limit - victims.len()
			));
			let mut statement = self.0.prepare(&sql)?;
			let mut rows = statement.query(rusqlite::params_from_iter(values))?;
			while let Some(row) = rows.next()? {
				let charge: i64 = row.get(1)?;
				if charge < 0 {
					return Err(StoreError::Incompatible);
				}
				if !victims.is_empty()
					&& bytes.saturating_add(charge) > crate::cache_cleanup::CLEANUP_BYTES
				{
					break;
				}
				bytes = bytes.saturating_add(charge);
				victims.push((table, row.get::<_, i64>(0)?));
			}
			if victims.len() == limit || bytes >= crate::cache_cleanup::CLEANUP_BYTES {
				break;
			}
		}
		let transaction = self.0.transaction()?;
		for (table, rowid) in &victims {
			transaction.execute(&format!("DELETE FROM {table} WHERE rowid=?1"), [rowid])?;
		}
		transaction.commit()?;
		Ok(victims.len())
	}

	/// Required additional reservation before converting an old non-reclaimable
	/// cache layout. The 96-MiB envelope includes the 64-MiB legacy ceiling plus
	/// 32 MiB for bounded schema growth before conversion admission.
	pub fn cache_layout_migration_bytes(&self) -> Result<Option<u64>> {
		let mode: i64 = self
			.0
			.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
		if mode == 2 {
			return Ok(None);
		}
		if mode != 0 {
			return Err(StoreError::Incompatible);
		}
		let pages: i64 = self
			.0
			.pragma_query_value(None, "page_count", |row| row.get(0))?;
		let page_size: i64 = self
			.0
			.pragma_query_value(None, "page_size", |row| row.get(0))?;
		let logical = u64::try_from(pages.checked_mul(page_size).ok_or(StoreError::Capacity)?)
			.map_err(|_| StoreError::Incompatible)?;
		let physical = match self
			.0
			.path()
			.filter(|path| !path.is_empty() && *path != ":memory:")
		{
			Some(path) => std::fs::metadata(path)
				.map_err(|_| StoreError::Unavailable)?
				.len(),
			None => 0,
		};
		let bytes = logical.max(physical);
		if bytes > MAX_LEGACY_LAYOUT_BYTES {
			return Err(StoreError::Capacity);
		}
		Ok(Some(
			bytes
				.checked_mul(3)
				.and_then(|bytes| bytes.checked_add(8 * 1024 * 1024))
				.ok_or(StoreError::Capacity)?,
		))
	}
	/// Storage-worker operation. The caller must hold the additional reservation
	/// returned above; this method never performs an unbounded layout conversion.
	pub fn migrate_cache_layout(&mut self) -> Result<()> {
		if self.cache_layout_migration_bytes()?.is_none() {
			return Ok(());
		}
		if !self.0.is_autocommit() {
			return Err(StoreError::Unavailable);
		}
		self.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA auto_vacuum=INCREMENTAL; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
		if self
			.0
			.pragma_query_value(None, "auto_vacuum", |row| row.get::<_, i64>(0))?
			!= 2
		{
			return Err(StoreError::Incompatible);
		}
		Ok(())
	}
	pub fn reclaim_history_space(&mut self) -> Result<()> {
		self.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA incremental_vacuum(1024); PRAGMA wal_checkpoint(TRUNCATE);")?;
		Ok(())
	}
}

fn read_stored_coverage(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCoverage> {
	for column in 0..=4 {
		let value = row.get_ref(column)?;
		if column == 4 && matches!(value, rusqlite::types::ValueRef::Null) {
			continue;
		}
		let value = value.as_str()?;
		if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit())
		{
			return Err(rusqlite::Error::InvalidQuery);
		}
	}
	Ok((
		row.get(0)?,
		row.get(1)?,
		row.get(2)?,
		row.get(3)?,
		row.get(4)?,
		row.get(5)?,
		row.get(6)?,
	))
}

pub(super) fn observe_change(
	transaction: &Transaction<'_>,
	account: Id,
	channel: Id,
	message: Id,
	deleted: bool,
) -> Result<u64> {
	if [account, channel, message].iter().any(|id| id.0 == 0) {
		return Err(StoreError::Capacity);
	}
	transaction.execute("INSERT INTO history_channels(account,channel,revision) VALUES(?1,?2,1) ON CONFLICT(account,channel) DO UPDATE SET revision=revision+1",params![account.to_string(),channel.to_string()])?;
	let revision: i64 = transaction.query_row(
		"SELECT revision FROM history_channels WHERE account=?1 AND channel=?2",
		params![account.to_string(), channel.to_string()],
		|row| row.get(0),
	)?;
	if deleted {
		transaction.execute("INSERT INTO history_tombstones(account,channel,id,revision) VALUES(?1,?2,?3,?4) ON CONFLICT(account,channel,id) DO UPDATE SET revision=excluded.revision",params![account.to_string(),channel.to_string(),message.to_string(),revision])?;
		transaction.execute(
			"DELETE FROM messages WHERE account=?1 AND channel=?2 AND id=?3",
			params![
				account.to_string(),
				channel.to_string(),
				message.to_string()
			],
		)?;
		transaction.execute(
			"DELETE FROM history_starts WHERE account=?1 AND channel=?2 AND first=?3",
			params![
				account.to_string(),
				channel.to_string(),
				message.to_string()
			],
		)?;
		let count: i64 = transaction.query_row(
			"SELECT count FROM tombstone_totals WHERE singleton=1",
			[],
			|row| row.get(0),
		)?;
		if count > MAX_TOMBSTONES as i64 {
			transaction.execute("DELETE FROM history_tombstones WHERE rowid IN (SELECT rowid FROM history_tombstones ORDER BY length(id),id LIMIT ?1)",[count-MAX_TOMBSTONES as i64])?;
		}
	}
	u64::try_from(revision).map_err(|_| StoreError::Incompatible)
}

#[cfg(test)]
mod tests {
	use super::*;
	use rusqlite::Connection;
	fn store() -> LocalStore {
		let mut store = LocalStore::initialize(Connection::open_in_memory().unwrap()).unwrap();
		activate(&mut store, 1, 1_420_070_400_000, 1_420_070_400_000);
		store
	}
	fn activate(store: &mut LocalStore, job: u64, cutoff_ms: i64, end_ms: i64) {
		store
			.save_sync_checkpoint(
				Id(1),
				&SyncCheckpoint {
					job,
					scope: None,
					started_ms: end_ms,
					cutoff_ms,
					payload: Vec::new(),
				},
			)
			.unwrap();
	}
	fn message(store: &LocalStore, id: u64, channel: u64) -> Message {
		// Decode the same bounded persisted projection used by application reads.
		store.0.execute("INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES('99','99',?1,'4','Display','body',0,0)",[id.to_string()]).unwrap();
		let mut result = store.load_channel(Id(99), Id(99)).unwrap().pop().unwrap();
		store
			.0
			.execute("DELETE FROM messages WHERE account='99'", [])
			.unwrap();
		result.id = Id(id);
		result.channel = Id(channel);
		result
	}
	fn coverage(
		request: u64,
		before: Option<u64>,
		next: Option<u64>,
		complete: bool,
	) -> HistoryCoverage {
		HistoryCoverage {
			job: 1,
			request,
			cutoff: Id(1),
			upper: Id(10_000),
			before: before.map(Id),
			next_before: next.map(Id),
			complete,
			revision: 0,
		}
	}
	#[test]
	fn paged_history_survives_small_windows_and_more_than_twenty_channels() {
		let mut store = store();
		let template = message(&store, 1, 2);
		for page in 0..12 {
			let high = 600 - page * 50;
			let rows: Vec<_> = (high - 49..=high)
				.map(|id| {
					let mut row = template.clone();
					row.id = Id(id);
					row
				})
				.collect();
			let state = coverage(
				page + 1,
				(page > 0).then_some(high + 1),
				Some(high - 49),
				false,
			);
			assert!(
				!store
					.save_sync_page(Id(1), Id(2), &rows, &state)
					.unwrap()
					.stale
			);
		}
		let latest = store.load_channel(Id(1), Id(2)).unwrap();
		assert_eq!(
			(latest.len(), latest[0].id, latest[499].id),
			(500, Id(101), Id(600))
		);
		store
			.save_changes(Id(1), Id(2), &[], &[Id(600)], false)
			.unwrap();
		assert_eq!(store.history_cache_stats().unwrap().messages, 600);
		let earlier = store
			.load_history_page(
				Id(1),
				Id(2),
				HistoryPageQuery {
					before: Some(Id(101)),
					around: None,
					limit: 50,
				},
			)
			.unwrap();
		assert_eq!((earlier[0].id, earlier[49].id), (Id(51), Id(100)));
		let around = store
			.load_history_page(
				Id(1),
				Id(2),
				HistoryPageQuery {
					before: None,
					around: Some(Id(20)),
					limit: 10,
				},
			)
			.unwrap();
		assert_eq!((around[0].id, around[9].id), (Id(16), Id(25)));
		for channel in 10..=35 {
			let mut row = template.clone();
			row.channel = Id(channel);
			store
				.save_channel(Id(1), Id(channel), &[row], false)
				.unwrap();
		}
		assert_eq!(store.load_channel(Id(1), Id(10)).unwrap().len(), 1);
		assert_eq!(store.history_cache_stats().unwrap().channels, 27);
		let ending = coverage(13, Some(1), None, true);
		assert_eq!(
			store.save_sync_page(Id(1), Id(2), &[], &ending),
			Err(StoreError::Capacity)
		);
	}
	#[test]
	fn verified_intervals_prune_absence_but_keep_other_ranges_and_live_changes() {
		let mut store = store();
		let template = message(&store, 1, 2);
		let rows: Vec<_> = (1..=150)
			.map(|id| {
				let mut row = template.clone();
				row.id = Id(id);
				row
			})
			.collect();
		store.save_channel(Id(1), Id(2), &rows, false).unwrap();
		let mut page = coverage(1, None, Some(101), false);
		let returned: Vec<_> = (101..=150)
			.map(|id| {
				let mut row = template.clone();
				row.id = Id(id);
				row
			})
			.collect();
		store
			.observe_history_change(Id(1), Id(2), Id(140), true)
			.unwrap();
		assert!(
			store
				.save_sync_page(Id(1), Id(2), &returned, &page)
				.unwrap()
				.stale
		);
		assert!(
			!store
				.load_channel(Id(1), Id(2))
				.unwrap()
				.iter()
				.any(|row| row.id == Id(140))
		);
		page.revision = 1;
		store
			.save_sync_page(Id(1), Id(2), &returned, &page)
			.unwrap();
		assert!(
			!store
				.load_channel(Id(1), Id(2))
				.unwrap()
				.iter()
				.any(|row| row.id == Id(140))
		);
		let mut partial = coverage(2, Some(101), None, true);
		partial.revision = 1;
		partial.cutoff = Id(1);
		store
			.save_sync_page(Id(1), Id(2), &[template], &partial)
			.unwrap();
		let all = store.load_channel(Id(1), Id(2)).unwrap();
		assert_eq!(all.len(), 50);
		assert_eq!(all[0].id, Id(1));
		assert!(
			store
				.history_coverage(Id(1), Id(2))
				.unwrap()
				.unwrap()
				.complete
		);
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), &[], &partial)
				.unwrap()
				.stale
		);
	}
	#[test]
	fn cutoff_is_inclusive_and_pages_commit_with_their_cursor() {
		let mut store = store();
		let template = message(&store, 90, 2);
		let rows: Vec<_> = (90..=110)
			.map(|id| {
				let mut row = template.clone();
				row.id = Id(id);
				row
			})
			.collect();
		let mut page = coverage(1, None, None, true);
		page.cutoff = Id(100);
		store.save_sync_page(Id(1), Id(2), &rows, &page).unwrap();
		assert_eq!(store.load_channel(Id(1), Id(2)).unwrap().len(), 11);
		let mut invalid = page;
		invalid.request = 2;
		assert!(
			store
				.save_sync_page(Id(1), Id(2), &rows, &invalid)
				.unwrap()
				.stale
		);
		assert!(
			store
				.history_coverage(Id(1), Id(2))
				.unwrap()
				.unwrap()
				.complete
		);
		store.evict_oldest_history(1, &[]).unwrap();
		assert!(
			!store
				.history_coverage(Id(1), Id(2))
				.unwrap()
				.unwrap()
				.complete
		);
		let mut retry = page;
		retry.request = 3;
		assert!(
			store
				.save_sync_page(Id(1), Id(2), &rows, &retry)
				.unwrap()
				.stale
		);
	}
	#[test]
	fn migration_and_account_scoped_checkpoint_preserve_personal_data() {
		let mut store = store();
		store.save_draft(Id(1), Id(2), "unsent").unwrap();
		store.0.execute_batch("PRAGMA user_version=31; DROP TABLE history_channels; DROP TABLE history_tombstones; DROP TABLE history_sync; DROP TABLE sync_checkpoints;").unwrap();
		store = LocalStore::initialize(store.0).unwrap();
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "unsent");
		let checkpoint = SyncCheckpoint {
			job: 5,
			scope: Some(Id(9)),
			started_ms: 2_000_000,
			cutoff_ms: 1_000_000,
			payload: vec![1, 2, 3],
		};
		store.save_sync_checkpoint(Id(1), &checkpoint).unwrap();
		assert_eq!(
			store.load_sync_checkpoint(Id(1)).unwrap(),
			Some(checkpoint.clone())
		);
		assert_eq!(store.load_sync_checkpoint(Id(2)).unwrap(), None);
		store.clear_history(Id(1)).unwrap();
		assert_eq!(store.load_sync_checkpoint(Id(1)).unwrap(), None);
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "unsent");
		let mut oversized = checkpoint;
		oversized.payload = vec![0; MAX_CHECKPOINT_BYTES + 1];
		assert_eq!(
			store.save_sync_checkpoint(Id(1), &oversized),
			Err(StoreError::Capacity)
		);
	}
	#[test]
	fn message_count_is_stable_on_upserts_and_deletions_fence_delayed_pages() {
		let mut store = store();
		let mut row = message(&store, 100, 2);
		store
			.save_channel(Id(1), Id(2), &[row.clone()], false)
			.unwrap();
		row.content = "edited".into();
		store
			.save_channel(Id(1), Id(2), &[row.clone()], false)
			.unwrap();
		assert_eq!(store.history_cache_stats().unwrap().messages, 1);
		store.delete_messages(Id(1), Id(2), &[row.id]).unwrap();
		assert_eq!(store.history_revision(Id(1), Id(2)).unwrap(), 1);
		assert_eq!(store.history_cache_stats().unwrap().messages, 0);
		assert_eq!(
			store
				.missing_history_messages(Id(1), Id(2), &[Id(100), Id(100), Id(101)])
				.unwrap(),
			1
		);
		assert_eq!(
			store.missing_history_messages(Id(1), Id(2), &[Id(101); 501]),
			Err(StoreError::Capacity)
		);
		store.save_channel(Id(1), Id(2), &[row], false).unwrap();
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
	}
	#[test]
	fn page_and_coverage_roll_back_together_when_checkpoint_write_fails() {
		let mut store = store();
		let row = message(&store, 100, 2);
		store
			.save_channel(Id(1), Id(2), std::slice::from_ref(&row), false)
			.unwrap();
		store.0.execute_batch("CREATE TRIGGER reject_sync BEFORE INSERT ON history_sync BEGIN SELECT RAISE(ABORT,'synthetic storage failure'); END;").unwrap();
		assert_eq!(
			store.save_sync_page(Id(1), Id(2), &[], &coverage(1, None, None, true)),
			Err(StoreError::Unavailable)
		);
		assert_eq!(store.load_channel(Id(1), Id(2)).unwrap().len(), 1);
		assert!(store.history_coverage(Id(1), Id(2)).unwrap().is_none());
		store.0.execute_batch("DROP TRIGGER reject_sync;").unwrap();
		store
			.save_sync_page(Id(1), Id(2), &[row], &coverage(1, None, None, true))
			.unwrap();
		store
			.observe_history_change(Id(1), Id(2), Id(100), true)
			.unwrap();
		// Recovery after page commit / before orchestration checkpoint acknowledges
		// the already committed request without replaying its now-deleted body.
		let replay = store
			.save_sync_page(Id(1), Id(2), &[], &coverage(1, None, None, true))
			.unwrap();
		assert_eq!(
			replay,
			SyncPageResult {
				saved: 0,
				stale: false
			}
		);
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
	}
	#[test]
	fn reclamation_protects_active_channels_and_marks_evicted_coverage() {
		let mut store = store();
		let template = message(&store, 100, 2);
		store
			.save_sync_page(
				Id(1),
				Id(2),
				std::slice::from_ref(&template),
				&coverage(1, None, None, true),
			)
			.unwrap();
		let mut other = template;
		other.channel = Id(3);
		other.id = Id(200);
		store
			.save_sync_page(Id(1), Id(3), &[other], &coverage(1, None, None, true))
			.unwrap();
		assert_eq!(store.evict_oldest_history(1, &[(Id(1), Id(2))]).unwrap(), 1);
		assert_eq!(store.load_channel(Id(1), Id(2)).unwrap().len(), 1);
		assert!(store.load_channel(Id(1), Id(3)).unwrap().is_empty());
		assert!(
			!store
				.history_coverage(Id(1), Id(3))
				.unwrap()
				.unwrap()
				.complete
		);
		assert!(
			store
				.history_coverage(Id(1), Id(2))
				.unwrap()
				.unwrap()
				.complete
		);
	}
	#[test]
	fn active_checkpoint_allows_clock_rollback_and_retires_cancelled_jobs() {
		const EPOCH: i64 = 1_420_070_400_000;
		let mut store = store();
		let mut row = message(&store, (5 << 22) + 100, 2);
		activate(&mut store, 10, EPOCH + 5, EPOCH + 5);
		let old = HistoryCoverage {
			job: 10,
			cutoff: Id(5 << 22),
			upper: Id(6 << 22),
			..coverage(1, None, None, true)
		};
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), std::slice::from_ref(&row), &old)
				.unwrap()
				.stale
		);
		activate(&mut store, 5, EPOCH + 2, EPOCH + 2);
		row.id = Id((2 << 22) + 100);
		row.content = "new authorized job after wall-clock rollback".into();
		let current = HistoryCoverage {
			job: 5,
			cutoff: Id(2 << 22),
			upper: Id(3 << 22),
			..coverage(1, None, None, true)
		};
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), std::slice::from_ref(&row), &current)
				.unwrap()
				.stale
		);
		assert!(store.save_sync_page(Id(1), Id(2), &[], &old).unwrap().stale);
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), std::slice::from_ref(&row), &current)
				.unwrap()
				.stale
		);
		assert!(
			store
				.save_sync_page(
					Id(1),
					Id(2),
					std::slice::from_ref(&row),
					&HistoryCoverage {
						request: 2,
						..current
					}
				)
				.unwrap()
				.stale
		);
		store.clear_sync_checkpoint(Id(1)).unwrap();
		assert!(
			store
				.save_sync_page(Id(1), Id(2), &[row], &current)
				.unwrap()
				.stale
		);
	}
	#[test]
	fn restarted_job_id_collision_is_fenced_by_its_frozen_interval() {
		const EPOCH: i64 = 1_420_070_400_000;
		let mut store = store();
		let mut row = message(&store, 100, 2);
		let old = coverage(1, None, None, true);
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), std::slice::from_ref(&row), &old)
				.unwrap()
				.stale
		);
		activate(&mut store, 1, EPOCH + 1, EPOCH + 1);
		row.id = Id((1 << 22) + 100);
		let current = HistoryCoverage {
			cutoff: Id(1 << 22),
			upper: Id(2 << 22),
			..old
		};
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), std::slice::from_ref(&row), &current)
				.unwrap()
				.stale
		);
		assert!(store.save_sync_page(Id(1), Id(2), &[], &old).unwrap().stale);
		assert!(
			!store
				.save_sync_page(Id(1), Id(2), &[row], &current)
				.unwrap()
				.stale
		);
	}
	fn physical_store(label: &str) -> (LocalStore, std::path::PathBuf) {
		let path = std::env::temp_dir().join(format!(
			"serein-vacuum-{label}-{}-{}.sqlite3",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		(LocalStore::open(&path).unwrap(), path)
	}
	fn fill_physical_history(store: &mut LocalStore) {
		let transaction = store.0.transaction().unwrap();
		{
			let mut insert=transaction.prepare("INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES('1','2',?1,'3','synthetic',?2,0,0)").unwrap();
			for id in 1..=1000 {
				insert
					.execute(params![id.to_string(), "x".repeat(4096)])
					.unwrap();
			}
		}
		transaction.commit().unwrap();
		store
			.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
			.unwrap();
	}
	#[test]
	fn fresh_file_enables_incremental_vacuum_and_releases_physical_pages() {
		let (mut store, path) = physical_store("fresh");
		assert_eq!(
			store
				.0
				.pragma_query_value::<i64, _>(None, "auto_vacuum", |row| row.get(0))
				.unwrap(),
			2
		);
		assert_eq!(store.cache_layout_migration_bytes().unwrap(), None);
		fill_physical_history(&mut store);
		store.save_draft(Id(1), Id(2), "preserved draft").unwrap();
		store
			.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
			.unwrap();
		let before = std::fs::metadata(&path).unwrap().len();
		store.clear_channel_history(Id(1), Id(2)).unwrap();
		store.reclaim_history_space().unwrap();
		let after = std::fs::metadata(&path).unwrap().len();
		assert!(
			after < before,
			"physical cache must shrink: {before} -> {after}"
		);
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "preserved draft");
		drop(store);
		std::fs::remove_file(path).unwrap();
	}
	#[test]
	fn legacy_layout_conversion_preserves_records_and_enables_physical_reclamation() {
		let (mut store, path) = physical_store("legacy");
		fill_physical_history(&mut store);
		store.save_draft(Id(1), Id(2), "legacy draft").unwrap();
		store.save_appearance(crate::Appearance::Dark).unwrap();
		store.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA auto_vacuum=NONE; VACUUM; PRAGMA user_version=31; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
		drop(store);
		let mut store = LocalStore::open(&path).unwrap();
		assert_eq!(
			store
				.0
				.pragma_query_value::<i64, _>(None, "auto_vacuum", |row| row.get(0))
				.unwrap(),
			0
		);
		let required = store.cache_layout_migration_bytes().unwrap().unwrap();
		assert!(required >= std::fs::metadata(&path).unwrap().len() * 3 + 8 * 1024 * 1024);
		let version = store
			.0
			.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
			.unwrap();
		store.migrate_cache_layout().unwrap();
		assert_eq!(store.cache_layout_migration_bytes().unwrap(), None);
		assert_eq!(store.load_channel(Id(1), Id(2)).unwrap().len(), 500);
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "legacy draft");
		assert_eq!(store.appearance().unwrap(), crate::Appearance::Dark);
		assert_eq!(
			store
				.0
				.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
				.unwrap(),
			version
		);
		let before = std::fs::metadata(&path).unwrap().len();
		store.clear_history(Id(1)).unwrap();
		assert!(std::fs::metadata(&path).unwrap().len() < before);
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "legacy draft");
		drop(store);
		std::fs::remove_file(path).unwrap();
	}
	#[test]
	fn oversized_legacy_layout_refuses_conversion_without_losing_records() {
		let (store, path) = physical_store("oversized");
		drop(store);
		let connection = Connection::open(&path).unwrap();
		connection.execute_batch("PRAGMA auto_vacuum=NONE; VACUUM; CREATE TABLE synthetic_layout_marker(value TEXT NOT NULL,payload BLOB); INSERT INTO synthetic_layout_marker VALUES('preserved',zeroblob(101711872)); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
		drop(connection);
		let mut store = LocalStore::open(&path).unwrap();
		store
			.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
			.unwrap();
		let before = std::fs::metadata(&path).unwrap().len();
		assert!(before > MAX_LEGACY_LAYOUT_BYTES);
		assert_eq!(
			store.cache_layout_migration_bytes(),
			Err(StoreError::Capacity)
		);
		assert_eq!(store.migrate_cache_layout(), Err(StoreError::Capacity));
		assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
		assert_eq!(
			store
				.0
				.pragma_query_value::<i64, _>(None, "auto_vacuum", |row| row.get(0))
				.unwrap(),
			0
		);
		assert_eq!(
			store
				.0
				.query_row::<String, _, _>("SELECT value FROM synthetic_layout_marker", [], |row| {
					row.get(0)
				})
				.unwrap(),
			"preserved"
		);
		drop(store);
		std::fs::remove_file(path).unwrap();
	}
	#[test]
	fn near_ceiling_schema31_file_survives_schema_growth_before_layout_conversion() {
		let (mut store, path) = physical_store("legacy-ceiling");
		let row = message(&store, 100, 2);
		store.save_channel(Id(1), Id(2), &[row], false).unwrap();
		store
			.save_draft(Id(1), Id(2), "preserved near-ceiling draft")
			.unwrap();
		store.save_appearance(crate::Appearance::Dark).unwrap();
		store.0.execute_batch("DROP TRIGGER history_count_insert; DROP TRIGGER history_count_delete; DROP TRIGGER member_cache_insert; DROP TRIGGER member_cache_delete; DROP TRIGGER member_cache_update; DROP TABLE cache_cleanup; DROP TABLE history_channels; DROP TABLE history_tombstones; DROP TABLE history_sync; DROP TABLE sync_checkpoints; DROP TABLE tombstone_totals; DROP TABLE history_totals; DROP TABLE member_cache_totals; ALTER TABLE member_displays DROP COLUMN search_key; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; PRAGMA auto_vacuum=NONE; VACUUM; PRAGMA user_version=31; PRAGMA max_page_count=16384; CREATE TABLE synthetic_legacy_padding(marker TEXT,payload BLOB); INSERT INTO synthetic_legacy_padding VALUES('preserved',zeroblob(66060288));").unwrap();
		for _ in 0..1024 {
			let pages: i64 = store
				.0
				.pragma_query_value(None, "page_count", |row| row.get(0))
				.unwrap();
			if pages >= 16380 {
				break;
			}
			store
				.0
				.execute(
					"INSERT INTO synthetic_legacy_padding VALUES('padding',zeroblob(4096))",
					[],
				)
				.unwrap();
		}
		let before = std::fs::metadata(&path).unwrap().len();
		assert!((16380 * 4096..=64 * 1024 * 1024).contains(&before));
		drop(store);
		let mut store = LocalStore::open(&path).unwrap();
		store
			.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
			.unwrap();
		let after_schema = std::fs::metadata(&path).unwrap().len();
		assert!(
			after_schema > 64 * 1024 * 1024,
			"schema growth must exercise the old boundary: {before} -> {after_schema}"
		);
		assert!(after_schema <= MAX_LEGACY_LAYOUT_BYTES);
		assert!(store.cache_layout_migration_bytes().unwrap().is_some());
		store.migrate_cache_layout().unwrap();
		assert_eq!(store.cache_layout_migration_bytes().unwrap(), None);
		assert_eq!(store.load_channel(Id(1), Id(2)).unwrap()[0].id, Id(100));
		assert_eq!(
			store.load_drafts(Id(1)).unwrap()[&Id(2)],
			"preserved near-ceiling draft"
		);
		assert_eq!(store.appearance().unwrap(), crate::Appearance::Dark);
		assert_eq!(
			store
				.0
				.query_row::<String, _, _>(
					"SELECT marker FROM synthetic_legacy_padding WHERE marker='preserved'",
					[],
					|row| row.get(0)
				)
				.unwrap(),
			"preserved"
		);
		drop(store);
		std::fs::remove_file(path).unwrap();
	}
	fn save_scoped_fixture(
		store: &mut LocalStore,
		account: Id,
		channel: Id,
		guild: Id,
		parent: Option<Id>,
	) {
		store
			.save_sync_checkpoint(
				account,
				&SyncCheckpoint {
					job: 1,
					scope: Some(guild),
					started_ms: 1_420_070_400_000,
					cutoff_ms: 1_420_070_400_000,
					payload: Vec::new(),
				},
			)
			.unwrap();
		let row = message(store, 100, channel.0);
		let page = HistoryCoverage {
			revision: store.history_revision(account, channel).unwrap(),
			..coverage(1, None, None, true)
		};
		assert!(
			!store
				.save_sync_page_scoped(account, channel, &[row], &page, Some(guild), parent)
				.unwrap()
				.stale
		);
	}
	#[test]
	fn archived_scope_cleanup_handles_uninstalled_parents_and_preserves_other_accounts_guilds() {
		let mut store = store();
		save_scoped_fixture(&mut store, Id(1), Id(5), Id(2), Some(Id(4)));
		save_scoped_fixture(&mut store, Id(1), Id(6), Id(2), Some(Id(4)));
		save_scoped_fixture(&mut store, Id(1), Id(9), Id(8), Some(Id(10)));
		save_scoped_fixture(&mut store, Id(7), Id(5), Id(2), Some(Id(4)));
		// The parent has never had a saved history window or navigation row here.
		assert!(store.load_channel(Id(1), Id(4)).unwrap().is_empty());
		store.clear_channel_history(Id(1), Id(5)).unwrap();
		assert!(store.load_channel(Id(1), Id(5)).unwrap().is_empty());
		assert_eq!(store.load_channel(Id(1), Id(6)).unwrap().len(), 1);
		store.clear_channel_history(Id(1), Id(4)).unwrap();
		assert!(store.load_channel(Id(1), Id(6)).unwrap().is_empty());
		assert_eq!(store.load_channel(Id(1), Id(9)).unwrap().len(), 1);
		assert_eq!(store.load_channel(Id(7), Id(5)).unwrap().len(), 1);
		assert_eq!(store.history_cache_stats().unwrap().messages, 2);
		let scopes: i64 = store
			.0
			.query_row(
				"SELECT count FROM history_scope_totals WHERE singleton=1",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(scopes, 2);
		let stale = store
			.save_sync_page_scoped(
				Id(1),
				Id(6),
				&[message(&store, 100, 6)],
				&coverage(1, None, None, true),
				Some(Id(2)),
				Some(Id(4)),
			)
			.unwrap();
		assert!(stale.stale);
		assert!(store.load_channel(Id(1), Id(6)).unwrap().is_empty());
	}
	#[test]
	fn interrupted_parent_cleanup_keeps_scope_links_until_descendants_are_retired() {
		let mut store = store();
		// A category may have a text/forum parent with archived child history.
		save_scoped_fixture(&mut store, Id(1), Id(4), Id(2), Some(Id(3)));
		save_scoped_fixture(&mut store, Id(1), Id(5), Id(2), Some(Id(4)));
		store.0.execute_batch("CREATE TRIGGER reject_archived_delete BEFORE DELETE ON messages WHEN old.channel='5' BEGIN SELECT RAISE(ABORT,'synthetic interruption'); END;").unwrap();
		assert_eq!(
			store.clear_channel_history(Id(1), Id(3)),
			Err(StoreError::Unavailable)
		);
		let links: i64 = store
			.0
			.query_row(
				"SELECT count(*) FROM history_scopes WHERE account='1'",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(links, 2);
		store
			.0
			.execute_batch("DROP TRIGGER reject_archived_delete;")
			.unwrap();
		store = LocalStore::initialize(store.0).unwrap();
		assert!(store.load_channel(Id(1), Id(4)).unwrap().is_empty());
		assert!(store.load_channel(Id(1), Id(5)).unwrap().is_empty());
		assert_eq!(store.history_cache_stats().unwrap().messages, 0);
		let links: i64 = store
			.0
			.query_row(
				"SELECT count FROM history_scope_totals WHERE singleton=1",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(links, 0);
		assert!(store.history_revision(Id(1), Id(5)).unwrap() > 0);
		assert!(
			store
				.save_sync_page_scoped(
					Id(1),
					Id(5),
					&[message(&store, 100, 5)],
					&coverage(1, None, None, true),
					Some(Id(2)),
					Some(Id(4))
				)
				.unwrap()
				.stale
		);
	}
	#[test]
	fn history_scope_validation_and_failed_commit_do_not_leave_orphan_scope_rows() {
		let mut store = store();
		let row = message(&store, 100, 5);
		let page = coverage(1, None, None, true);
		for (guild, parent) in [
			(None, Some(Id(4))),
			(Some(Id(0)), None),
			(Some(Id(2)), Some(Id(0))),
			(Some(Id(2)), Some(Id(5))),
		] {
			assert_eq!(
				store.save_sync_page_scoped(
					Id(1),
					Id(5),
					std::slice::from_ref(&row),
					&page,
					guild,
					parent
				),
				Err(StoreError::Capacity)
			);
		}
		store.0.execute_batch("CREATE TRIGGER reject_scoped_coverage BEFORE INSERT ON history_sync BEGIN SELECT RAISE(ABORT,'synthetic commit failure'); END;").unwrap();
		assert_eq!(
			store.save_sync_page_scoped(Id(1), Id(5), &[row], &page, Some(Id(2)), Some(Id(4))),
			Err(StoreError::Unavailable)
		);
		assert!(store.load_channel(Id(1), Id(5)).unwrap().is_empty());
		let count: i64 = store
			.0
			.query_row(
				"SELECT count FROM history_scope_totals WHERE singleton=1",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(count, 0);
	}
}
