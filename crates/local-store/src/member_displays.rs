//! Bounded, account-isolated presentation hints. No permission, presence or role state is stored.
use super::{Id, LocalStore, OptionalExtension, Result, StoreError, params};
use model::MemberDisplay;
use std::collections::BTreeSet;

const LIMIT: usize = 4096;
const MAX_BYTES: usize = 2 * 1024 * 1024;
// Includes IDs, the DTO, per-row bookkeeping and field allocations; SQLite pages retain
// their existing independent 64-MiB ceiling.
const CHARGE: usize = 256;
const ROW_CHARGE: &str = "256+length(CAST(name AS BLOB))+coalesce(length(CAST(username AS BLOB)),0)+coalesce(length(CAST(nick AS BLOB)),0)";

impl LocalStore {
	/// Sparse authoritative display updates; callers serialize them with departures/cleanup.
	pub fn save_member_displays(&mut self, account: Id, entries: &[MemberDisplay]) -> Result<()> {
		let mut seen = BTreeSet::new();
		if account.0 == 0
			|| entries.len() > LIMIT
			|| entries
				.iter()
				.any(|entry| !entry.valid() || !seen.insert((entry.guild, entry.user)))
			|| entries
				.iter()
				.map(|entry| {
					CHARGE
						+ entry.name.len() + entry.username.as_deref().map_or(0, str::len)
						+ entry.nick.as_ref().map_or(0, String::len)
				})
				.sum::<usize>()
				> MAX_BYTES
		{
			return Err(StoreError::Capacity);
		}
		if entries.is_empty() {
			return Ok(());
		}
		let transaction = self.0.transaction()?;
		let (mut count, mut bytes, touched): (i64, i64, i64) = transaction.query_row(
			&format!(
				"SELECT count(*),coalesce(sum({ROW_CHARGE}),0),coalesce(max(touched),0) FROM member_displays"
			),
			[],
			|row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
		)?;
		let touched = touched.checked_add(1).ok_or(StoreError::Incompatible)?;
		let account = account.to_string();
		for entry in entries {
			let guild = entry.guild.to_string();
			let user = entry.user.to_string();
			let old: Option<(i64,bool)> = transaction.query_row(
				&format!("SELECT {ROW_CHARGE},name=?4 AND username IS ?5 AND nick IS ?6 FROM member_displays WHERE account=?1 AND guild=?2 AND user=?3"),
				params![account,guild,user,entry.name,entry.username.as_deref(),entry.nick],
				|row|Ok((row.get(0)?,row.get(1)?)),
			).optional()?;
			if let Some((_, true)) = old {
				// A snapshot can contain hundreds of unchanged peers after one identity changes.
				// Keep this read-only; eviction recency follows changed observations.
				continue;
			}
			if let Some((old_bytes, _)) = old {
				transaction.execute(
					"DELETE FROM member_displays WHERE account=?1 AND guild=?2 AND user=?3",
					params![account, guild, user],
				)?;
				count -= 1;
				bytes -= old_bytes;
			}
			let charge = (CHARGE
				+ entry.name.len()
				+ entry.username.as_deref().map_or(0, str::len)
				+ entry.nick.as_ref().map_or(0, String::len)) as i64;
			while count >= LIMIT as i64 || bytes + charge > MAX_BYTES as i64 {
				let (a,g,u,old_bytes):(String,String,String,i64) = transaction.query_row(
					&format!("SELECT account,guild,user,{ROW_CHARGE} FROM member_displays ORDER BY touched,account,guild,user LIMIT 1"),
					[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
				)?;
				transaction.execute(
					"DELETE FROM member_displays WHERE account=?1 AND guild=?2 AND user=?3",
					params![a, g, u],
				)?;
				count -= 1;
				bytes -= old_bytes;
			}
			transaction.execute("INSERT INTO member_displays(account,guild,user,name,username,nick,touched) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![account,guild,user,entry.name,entry.username.as_deref(),entry.nick,touched])?;
			count += 1;
			bytes += charge;
		}
		transaction.commit()?;
		Ok(())
	}
	/// Display hints only. Current membership/access must be checked before hydrating them.
	pub fn load_member_displays(&self, account: Id, guild: Id) -> Result<Vec<MemberDisplay>> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		let mut statement = self.0.prepare_cached("SELECT user,name,username,nick FROM member_displays WHERE account=?1 AND guild=?2 ORDER BY touched DESC,user LIMIT 4097")?;
		let mut rows = statement.query(params![account.to_string(), guild.to_string()])?;
		let mut entries = Vec::new();
		let mut bytes = 0;
		while let Some(row) = rows.next()? {
			if entries.len() == LIMIT {
				return Err(StoreError::Capacity);
			}
			for (column, maximum) in [(0, 20), (1, 512), (2, 512), (3, 512)] {
				let value = row.get_ref(column)?;
				if !matches!(value, rusqlite::types::ValueRef::Null) {
					let value = value.as_str().map_err(|_| StoreError::Incompatible)?;
					if value.len() > maximum {
						return Err(StoreError::Capacity);
					}
				}
			}
			let entry = MemberDisplay {
				guild,
				user: row
					.get_ref(0)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?
					.parse()
					.map_err(|_| StoreError::Incompatible)?,
				name: row.get(1)?,
				username: row.get::<_, Option<String>>(2)?.map(String::into_boxed_str),
				nick: row.get(3)?,
			};
			if !entry.valid() {
				return Err(StoreError::Incompatible);
			}
			bytes += CHARGE
				+ entry.name.len()
				+ entry.username.as_deref().map_or(0, str::len)
				+ entry.nick.as_ref().map_or(0, String::len);
			if bytes > MAX_BYTES {
				return Err(StoreError::Capacity);
			}
			entries.push(entry);
		}
		Ok(entries)
	}
	pub fn remove_member_display(&mut self, account: Id, guild: Id, user: Id) -> Result<()> {
		if [account, guild, user].iter().any(|id| id.0 == 0) {
			return Err(StoreError::Capacity);
		}
		self.0.execute(
			"DELETE FROM member_displays WHERE account=?1 AND guild=?2 AND user=?3",
			params![account.to_string(), guild.to_string(), user.to_string()],
		)?;
		Ok(())
	}
	pub fn clear_member_guild(&mut self, account: Id, guild: Id) -> Result<()> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		self.0.execute(
			"DELETE FROM member_displays WHERE account=?1 AND guild=?2",
			params![account.to_string(), guild.to_string()],
		)?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rusqlite::Connection;
	fn store() -> LocalStore {
		LocalStore::initialize(Connection::open_in_memory().unwrap()).unwrap()
	}
	fn entry(guild: u64, user: u64) -> MemberDisplay {
		MemberDisplay {
			guild: Id(guild),
			user: Id(user),
			name: format!("Display {user}"),
			username: Some(format!("user_{user}").into()),
			nick: Some(format!("Nickname {user}")),
		}
	}
	#[test]
	fn display_cache_migrates_and_preserves_settings_and_account_scope() {
		let mut store = store();
		store.0.execute_batch("DROP TABLE member_displays; PRAGMA user_version=29; INSERT INTO drafts VALUES('1','2','unchanged draft');").unwrap();
		store = LocalStore::initialize(store.0).unwrap();
		assert_eq!(
			store
				.0
				.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
				.unwrap(),
			30
		);
		let one = entry(10, 20);
		let other = entry(10, 21);
		store
			.save_member_displays(Id(1), std::slice::from_ref(&one))
			.unwrap();
		store
			.save_member_displays(Id(2), std::slice::from_ref(&other))
			.unwrap();
		assert_eq!(
			store.load_member_displays(Id(1), Id(10)).unwrap(),
			vec![one]
		);
		assert_eq!(
			store.load_member_displays(Id(2), Id(10)).unwrap(),
			vec![other]
		);
		assert!(
			store
				.load_member_displays(Id(1), Id(11))
				.unwrap()
				.is_empty()
		);
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "unchanged draft");
	}
	#[test]
	fn explicit_nickname_removal_departure_and_clear_are_scoped() {
		let mut store = store();
		store
			.save_member_displays(Id(1), &[entry(10, 20), entry(10, 21), entry(11, 20)])
			.unwrap();
		store.save_member_displays(Id(2), &[entry(10, 20)]).unwrap();
		let mut removed = entry(10, 20);
		removed.nick = None;
		removed.username = None;
		store
			.save_member_displays(Id(1), std::slice::from_ref(&removed))
			.unwrap();
		assert!(
			store
				.load_member_displays(Id(1), Id(10))
				.unwrap()
				.iter()
				.any(|e| e.user == Id(20) && e.nick.is_none() && e.username.is_none())
		);
		store.remove_member_display(Id(1), Id(10), Id(21)).unwrap();
		assert_eq!(
			store.load_member_displays(Id(1), Id(10)).unwrap(),
			vec![removed]
		);
		store.clear_member_guild(Id(1), Id(10)).unwrap();
		assert!(
			store
				.load_member_displays(Id(1), Id(10))
				.unwrap()
				.is_empty()
		);
		assert_eq!(store.load_member_displays(Id(1), Id(11)).unwrap().len(), 1);
		store.clear_history(Id(1)).unwrap();
		assert!(
			store
				.load_member_displays(Id(1), Id(11))
				.unwrap()
				.is_empty()
		);
		assert_eq!(store.load_member_displays(Id(2), Id(10)).unwrap().len(), 1);
		store.forget_account(Id(2)).unwrap();
		assert!(
			store
				.load_member_displays(Id(2), Id(10))
				.unwrap()
				.is_empty()
		);
	}
	#[test]
	fn display_cache_global_count_byte_eviction_and_rejected_batch_rollback() {
		let mut store = store();
		let rows: Vec<_> = (1..=4096).map(|id| entry(10, id)).collect();
		store.save_member_displays(Id(1), &rows).unwrap();
		store.save_member_displays(Id(2), &[entry(11, 1)]).unwrap();
		let count: i64 = store
			.0
			.query_row("SELECT count(*) FROM member_displays", [], |r| r.get(0))
			.unwrap();
		assert_eq!(count, 4096);
		assert_eq!(
			store.load_member_displays(Id(1), Id(10)).unwrap().len(),
			4095
		);
		let large: Vec<_> = (1..=1000)
			.map(|id| MemberDisplay {
				guild: Id(12),
				user: Id(id),
				name: "😀".repeat(128),
				username: Some("😀".repeat(128).into()),
				nick: Some("😀".repeat(128)),
			})
			.collect();
		store.save_member_displays(Id(3), &large).unwrap();
		let bytes: i64 = store
			.0
			.query_row(
				&format!("SELECT sum({ROW_CHARGE}) FROM member_displays"),
				[],
				|r| r.get(0),
			)
			.unwrap();
		assert!(bytes <= MAX_BYTES as i64);
		let retained = store.load_member_displays(Id(3), Id(12)).unwrap();
		assert_eq!(retained.len(), 1000);
		let invalid = MemberDisplay {
			name: "x".repeat(513),
			..entry(12, 2000)
		};
		assert_eq!(
			store.save_member_displays(Id(3), &[entry(12, 2001), invalid]),
			Err(StoreError::Capacity)
		);
		assert_eq!(store.load_member_displays(Id(3), Id(12)).unwrap(), retained);
		assert_eq!(
			store.save_member_displays(Id(3), &[entry(12, 20), entry(12, 20)]),
			Err(StoreError::Capacity)
		);
	}
	#[test]
	fn display_cache_rejects_corruption_before_large_field_allocation() {
		let mut store = store();
		store.save_member_displays(Id(1), &[entry(10, 20)]).unwrap();
		store
			.0
			.execute_batch(
				"PRAGMA ignore_check_constraints=ON; UPDATE member_displays SET name=zeroblob(1048576);",
			)
			.unwrap();
		assert_eq!(
			store.load_member_displays(Id(1), Id(10)),
			Err(StoreError::Incompatible)
		);
	}
	#[test]
	fn repeated_identity_snapshots_do_not_write_unchanged_peers() {
		let mut store = store();
		let mut entries: Vec<_> = (1..=512).map(|user| entry(10, user)).collect();
		store.save_member_displays(Id(1), &entries).unwrap();
		store.0.execute_batch("CREATE TABLE display_writes(n INTEGER); INSERT INTO display_writes VALUES(0);
            CREATE TRIGGER count_display_inserts AFTER INSERT ON member_displays BEGIN UPDATE display_writes SET n=n+1; END;
            CREATE TRIGGER count_display_updates AFTER UPDATE ON member_displays BEGIN UPDATE display_writes SET n=n+1; END;
            CREATE TRIGGER count_display_deletes AFTER DELETE ON member_displays BEGIN UPDATE display_writes SET n=n+1; END;").unwrap();
		for _ in 0..3 {
			store.save_member_displays(Id(1), &entries).unwrap();
		}
		let writes = |store: &LocalStore| {
			store
				.0
				.query_row::<i64, _, _>("SELECT n FROM display_writes", [], |r| r.get(0))
				.unwrap()
		};
		assert_eq!(writes(&store), 0);
		entries[250].nick = Some("Changed nickname".into());
		store.save_member_displays(Id(1), &entries).unwrap();
		// The changed identity is replaced; its 511 peers remain untouched.
		assert_eq!(writes(&store), 2);
		store.save_member_displays(Id(1), &entries).unwrap();
		assert_eq!(writes(&store), 2);
		assert!(
			store
				.load_member_displays(Id(1), Id(10))
				.unwrap()
				.iter()
				.any(|entry| entry.user == Id(251)
					&& entry.nick.as_deref() == Some("Changed nickname"))
		);
	}
	#[test]
	fn display_cache_survives_file_reopen() {
		let root = std::env::temp_dir().join(format!(
			"serein-member-cache-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		std::fs::create_dir(&root).unwrap();
		let path = root.join("test.sqlite3");
		let expected = entry(10, 20);
		let mut store = LocalStore::open(&path).unwrap();
		store
			.save_member_displays(Id(1), std::slice::from_ref(&expected))
			.unwrap();
		drop(store);
		let mut store = LocalStore::open(&path).unwrap();
		assert_eq!(
			store.load_member_displays(Id(1), Id(10)).unwrap(),
			vec![expected]
		);
		store.forget_account(Id(1)).unwrap();
		drop(store);
		assert!(
			LocalStore::open(&path)
				.unwrap()
				.load_member_displays(Id(1), Id(10))
				.unwrap()
				.is_empty()
		);
		std::fs::remove_dir_all(root).unwrap();
	}
}
