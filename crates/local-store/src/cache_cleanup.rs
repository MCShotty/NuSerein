//! Byte-bounded cache removal. Durable intents prevent interrupted cleanup from
//! exposing the remainder on the next launch.
use super::{Id, LocalStore, OptionalExtension, Result, StoreError, params};

pub(super) const CLEANUP_BYTES: i64 = 4 * 1024 * 1024;
const CACHE_TABLES: &[&str] = &[
	"messages",
	"member_displays",
	"expression_catalogs",
	"sticker_pack_hints",
	"server_displays",
	"channels",
	"history_starts",
	"history_channels",
	"history_tombstones",
	"history_sync",
	"sync_checkpoints",
];
const ACCOUNT_TABLES: &[&str] = &[
	"drafts",
	"gif_favorites",
	"channel_preferences",
	"account_presence",
	"accounts",
];

impl LocalStore {
	pub(super) fn cleanup_cache_scope(
		&mut self,
		account: Id,
		channel: Option<Id>,
		forget: bool,
	) -> Result<()> {
		if account.0 == 0 || channel.is_some_and(|id| id.0 == 0) || forget && channel.is_some() {
			return Err(StoreError::Capacity);
		}
		self.0.execute("INSERT INTO cache_cleanup(account,channel,forget) VALUES(?1,?2,?3) ON CONFLICT(account) DO UPDATE SET channel=CASE WHEN cache_cleanup.channel IS NULL OR excluded.channel IS NULL OR cache_cleanup.channel!=excluded.channel THEN NULL ELSE cache_cleanup.channel END,forget=max(cache_cleanup.forget,excluded.forget)",params![account.to_string(),channel.map(|id|id.to_string()),forget])?;
		self.finish_cache_cleanups()
	}
	pub(super) fn finish_cache_cleanups(&mut self) -> Result<()> {
		loop {
			let pending: Option<(String, Option<String>, bool)> = self
				.0
				.query_row(
					"SELECT account,channel,forget FROM cache_cleanup LIMIT 1",
					[],
					|row| {
						for column in 0..=1 {
							let field = row.get_ref(column)?;
							if !matches!(field, rusqlite::types::ValueRef::Null)
								&& field.as_str()?.len() > 20
							{
								return Err(rusqlite::Error::InvalidQuery);
							}
						}
						Ok((row.get(0)?, row.get(1)?, row.get(2)?))
					},
				)
				.optional()?;
			let Some((account, channel, forget)) = pending else {
				return Ok(());
			};
			let account = account
				.parse::<Id>()
				.map_err(|_| StoreError::Incompatible)?;
			let channel = channel
				.map(|id| id.parse::<Id>())
				.transpose()
				.map_err(|_| StoreError::Incompatible)?;
			for table in CACHE_TABLES {
				let scope = channel.map(|channel| ("channel", channel));
				if channel.is_some()
					&& !matches!(
						*table,
						"messages"
							| "channels" | "history_starts"
							| "history_channels" | "history_tombstones"
							| "history_sync"
					) {
					continue;
				}
				self.delete_cache_rows(table, account, scope)?;
			}
			if forget {
				for table in ACCOUNT_TABLES {
					self.delete_cache_rows(table, account, None)?;
				}
			}
			self.0.execute(
				"DELETE FROM cache_cleanup WHERE account=?1",
				[account.to_string()],
			)?;
			self.reclaim_history_space()?;
		}
	}
	/// The caller owns its access/epoch fence. All dynamic identifiers come from
	/// the fixed table/column lists here, never from persisted or remote text.
	pub(super) fn delete_cache_rows(
		&mut self,
		table: &str,
		account: Id,
		scope: Option<(&str, Id)>,
	) -> Result<usize> {
		if !CACHE_TABLES.contains(&table) && !ACCOUNT_TABLES.contains(&table) {
			return Err(StoreError::Incompatible);
		}
		if scope.is_some_and(|(column, _)| !matches!(column, "channel" | "guild")) {
			return Err(StoreError::Incompatible);
		}
		let charge = self.cache_row_charge(table, None)?;
		let filter = match scope {
			Some((column, _)) => format!("account=?1 AND {column}=?2"),
			None => "account=?1".to_owned(),
		};
		let sql = format!("SELECT rowid,{charge} FROM {table} WHERE {filter} LIMIT 128");
		let mut removed = 0;
		loop {
			let mut query = self.0.prepare_cached(&sql)?;
			let mut values = vec![account.to_string()];
			if let Some((_, id)) = scope {
				values.push(id.to_string());
			}
			let mut rows = query.query(rusqlite::params_from_iter(values))?;
			let mut victims = Vec::new();
			let mut bytes = 0i64;
			while let Some(row) = rows.next()? {
				let row_bytes: i64 = row.get(1)?;
				if row_bytes < 0 {
					return Err(StoreError::Incompatible);
				}
				if !victims.is_empty() && bytes.saturating_add(row_bytes) > CLEANUP_BYTES {
					break;
				}
				victims.push(row.get::<_, i64>(0)?);
				bytes = bytes.saturating_add(row_bytes);
			}
			drop(rows);
			drop(query);
			if victims.is_empty() {
				return Ok(removed);
			}
			let transaction = self.0.transaction()?;
			for rowid in &victims {
				transaction.execute(&format!("DELETE FROM {table} WHERE rowid=?1"), [rowid])?;
			}
			transaction.commit()?;
			removed += victims.len();
			self.reclaim_history_space()?;
		}
	}
	pub(super) fn cache_row_charge(&self, table: &str, alias: Option<&str>) -> Result<String> {
		if !CACHE_TABLES.contains(&table) && !ACCOUNT_TABLES.contains(&table) {
			return Err(StoreError::Incompatible);
		}
		let prefix = alias.map(|alias| format!("{alias}.")).unwrap_or_default();
		let mut query = self.0.prepare(&format!("PRAGMA table_info({table})"))?;
		let mut rows = query.query([])?;
		let mut expression = "256".to_owned();
		while let Some(row) = rows.next()? {
			let name: String = row.get(1)?;
			if !name
				.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
			{
				return Err(StoreError::Incompatible);
			}
			expression.push_str(&format!(
				"+coalesce(length(CAST({prefix}{name} AS BLOB)),0)"
			));
		}
		Ok(expression)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rusqlite::Connection;
	fn store() -> LocalStore {
		LocalStore::initialize(Connection::open_in_memory().unwrap()).unwrap()
	}
	#[test]
	fn interrupted_channel_cleanup_finishes_before_store_is_returned() {
		let mut store = store();
		for (account, channel) in [(1, 2), (1, 3), (9, 2)] {
			store.0.execute("INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES(?1,?2,'10','4','Display','cached',0,0)",params![account.to_string(),channel.to_string()]).unwrap();
		}
		store
			.save_draft(Id(1), Id(2), "draft survives cache cleanup")
			.unwrap();
		store
			.0
			.execute(
				"INSERT INTO cache_cleanup(account,channel,forget) VALUES('1','2',0)",
				[],
			)
			.unwrap();
		store = LocalStore::initialize(store.0).unwrap();
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
		assert_eq!(store.load_channel(Id(1), Id(3)).unwrap().len(), 1);
		assert_eq!(store.load_channel(Id(9), Id(2)).unwrap().len(), 1);
		assert_eq!(
			store.load_drafts(Id(1)).unwrap()[&Id(2)],
			"draft survives cache cleanup"
		);
		let pending: i64 = store
			.0
			.query_row("SELECT count(*) FROM cache_cleanup", [], |row| row.get(0))
			.unwrap();
		assert_eq!(pending, 0);
	}
	#[test]
	fn failed_cleanup_keeps_intent_and_forget_recovers_without_other_account_loss() {
		let mut store = store();
		store.0.execute_batch("INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES('1','2','9','4','Display','cached',0,0); INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES('9','2','9','4','Other','cached',0,0); CREATE TRIGGER reject_cache_delete BEFORE DELETE ON messages WHEN old.account='1' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
		store.save_draft(Id(1), Id(2), "remove on forget").unwrap();
		store
			.save_draft(Id(9), Id(2), "preserve other account")
			.unwrap();
		assert_eq!(store.forget_account(Id(1)), Err(StoreError::Unavailable));
		let pending: i64 = store
			.0
			.query_row(
				"SELECT count(*) FROM cache_cleanup WHERE account='1' AND forget=1",
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert_eq!(pending, 1);
		store
			.0
			.execute_batch("DROP TRIGGER reject_cache_delete;")
			.unwrap();
		store = LocalStore::initialize(store.0).unwrap();
		assert!(store.load_drafts(Id(1)).unwrap().is_empty());
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
		assert_eq!(
			store.load_drafts(Id(9)).unwrap()[&Id(2)],
			"preserve other account"
		);
		assert_eq!(store.load_channel(Id(9), Id(2)).unwrap().len(), 1);
	}
}
