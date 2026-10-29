//! Account-scoped, bounded server presentation hints; never a membership or permission source.
use super::{Id, LocalStore, OptionalExtension, Result, StoreError, params};
use std::collections::BTreeSet;

const LIMIT: usize = 4096;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const CHARGE: usize = 256;
const ROW_CHARGE: &str =
	"256+coalesce(length(CAST(name AS BLOB)),0)+coalesce(length(CAST(icon AS BLOB)),0)";

/// A sparse observation. `None` means absent; an explicit icon removal is `Some(None)`.
/// An explicit null name is represented by `Some(String::new())`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerDisplay {
	pub guild: Id,
	pub name: Option<String>,
	pub icon: Option<Option<String>>,
}
impl ServerDisplay {
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self
				.name
				.as_ref()
				.is_none_or(|name| name.len() <= 512 && name.chars().count() <= 128)
			&& self
				.icon
				.as_ref()
				.is_none_or(|icon| icon.as_deref().is_none_or(model::valid_avatar_hash))
	}
	pub fn bytes(&self) -> usize {
		CHARGE
			+ self.name.as_ref().map_or(0, String::len)
			+ self
				.icon
				.as_ref()
				.and_then(|icon| icon.as_ref())
				.map_or(0, String::len)
	}
}

impl LocalStore {
	/// Merge only fields present in service observations, preserving other known fields.
	/// Callers serialize this operation with guild retirement and cache invalidation.
	pub fn save_server_displays(&mut self, account: Id, entries: &[ServerDisplay]) -> Result<()> {
		let mut seen = BTreeSet::new();
		if account.0 == 0
			|| entries.len() > LIMIT
			|| entries
				.iter()
				.any(|entry| !entry.valid() || !seen.insert(entry.guild))
			|| entries.iter().map(ServerDisplay::bytes).sum::<usize>() > MAX_BYTES
		{
			return Err(StoreError::Capacity);
		}
		if entries.is_empty() {
			return Ok(());
		}
		let tx = self.0.transaction()?;
		let (mut count, mut bytes, touched): (i64, i64, i64) = tx.query_row(
			&format!(
				"SELECT count(*),coalesce(sum({ROW_CHARGE}),0),coalesce(max(touched),0) FROM server_displays"
			),
			[],
			|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
		)?;
		let touched = touched.checked_add(1).ok_or(StoreError::Incompatible)?;
		let account = account.to_string();
		for entry in entries {
			if entry.name.is_none() && entry.icon.is_none() {
				continue;
			}
			let guild = entry.guild.to_string();
			let previous = tx.query_row("SELECT name,icon_known,icon FROM server_displays WHERE account=?1 AND guild=?2 AND (name IS NULL OR (typeof(name)='text' AND length(CAST(name AS BLOB))<=512)) AND (icon IS NULL OR (typeof(icon)='text' AND length(CAST(icon AS BLOB))<=34))", params![account,guild], |r| Ok((r.get::<_,Option<String>>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
			let old = match previous {
				Some((name, known, icon)) if known == 0 && icon.is_none() => Some(ServerDisplay {
					guild: entry.guild,
					name,
					icon: None,
				}),
				Some((name, 1, icon)) => Some(ServerDisplay {
					guild: entry.guild,
					name,
					icon: Some(icon),
				}),
				Some(_) => return Err(StoreError::Incompatible),
				None => None,
			};
			if old.as_ref().is_some_and(|old| !old.valid()) {
				return Err(StoreError::Incompatible);
			}
			let merged = ServerDisplay {
				guild: entry.guild,
				name: entry
					.name
					.clone()
					.or_else(|| old.as_ref().and_then(|old| old.name.clone())),
				icon: entry
					.icon
					.clone()
					.or_else(|| old.as_ref().and_then(|old| old.icon.clone())),
			};
			if old.as_ref() == Some(&merged) {
				continue;
			}
			if let Some(old) = old {
				tx.execute(
					"DELETE FROM server_displays WHERE account=?1 AND guild=?2",
					params![account, guild],
				)?;
				count -= 1;
				bytes -= old.bytes() as i64;
			}
			let charge = merged.bytes() as i64;
			while count >= LIMIT as i64 || bytes + charge > MAX_BYTES as i64 {
				let (a,g,old_bytes): (String,String,i64) = tx.query_row(
                    &format!("SELECT account,guild,{ROW_CHARGE} FROM server_displays ORDER BY touched,account,guild LIMIT 1"),
                    [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
				tx.execute(
					"DELETE FROM server_displays WHERE account=?1 AND guild=?2",
					params![a, g],
				)?;
				count -= 1;
				bytes -= old_bytes;
			}
			tx.execute("INSERT INTO server_displays(account,guild,name,icon_known,icon,touched) VALUES(?1,?2,?3,?4,?5,?6)",
                params![account,guild,merged.name,merged.icon.is_some(),merged.icon.flatten(),touched])?;
			count += 1;
			bytes += charge;
		}
		tx.commit()?;
		Ok(())
	}
	/// Returns hints only; the caller must first establish current membership from Discord.
	pub fn load_server_displays(&self, account: Id) -> Result<Vec<ServerDisplay>> {
		if account.0 == 0 {
			return Err(StoreError::Capacity);
		}
		let mut statement = self.0.prepare_cached("SELECT guild,name,icon_known,icon FROM server_displays WHERE account=?1 ORDER BY touched DESC,guild LIMIT 4097")?;
		let mut rows = statement.query([account.to_string()])?;
		let mut result = Vec::new();
		let mut bytes = 0;
		while let Some(row) = rows.next()? {
			if result.len() == LIMIT {
				return Err(StoreError::Capacity);
			}
			let display = read_display(row)?;
			bytes += display.bytes();
			if bytes > MAX_BYTES {
				return Err(StoreError::Capacity);
			}
			result.push(display);
		}
		Ok(result)
	}
	/// Load only requested account-scoped hints via the `(account, guild)` primary key.
	/// Callers must establish current membership independently of this cache.
	pub fn load_server_displays_for(
		&self,
		account: Id,
		guilds: &[Id],
	) -> Result<Vec<ServerDisplay>> {
		let mut seen = BTreeSet::new();
		if account.0 == 0
			|| guilds.len() > LIMIT
			|| guilds
				.iter()
				.any(|guild| guild.0 == 0 || !seen.insert(*guild))
		{
			return Err(StoreError::Capacity);
		}
		let mut statement = self.0.prepare_cached(
			"SELECT guild,name,icon_known,icon FROM server_displays WHERE account=?1 AND guild=?2",
		)?;
		let mut result = Vec::new();
		let mut bytes = 0;
		let account = account.to_string();
		for guild in guilds {
			let mut rows = statement.query(params![account, guild.to_string()])?;
			if let Some(row) = rows.next()? {
				let display = read_display(row)?;
				if display.guild != *guild {
					return Err(StoreError::Incompatible);
				}
				bytes += display.bytes();
				if bytes > MAX_BYTES {
					return Err(StoreError::Capacity);
				}
				result.push(display);
			}
		}
		Ok(result)
	}
	pub fn clear_server_display(&mut self, account: Id, guild: Id) -> Result<()> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		self.0.execute(
			"DELETE FROM server_displays WHERE account=?1 AND guild=?2",
			params![account.to_string(), guild.to_string()],
		)?;
		Ok(())
	}
}

// Validate borrowed SQL values before allocating strings, including when the database is corrupt.
fn read_display(row: &rusqlite::Row<'_>) -> Result<ServerDisplay> {
	for (column, maximum) in [(0, 20), (1, 512), (3, 34)] {
		let value = row.get_ref(column)?;
		if !matches!(value, rusqlite::types::ValueRef::Null)
			&& value.as_str().map_err(|_| StoreError::Incompatible)?.len() > maximum
		{
			return Err(StoreError::Capacity);
		}
	}
	let guild = row
		.get_ref(0)?
		.as_str()
		.map_err(|_| StoreError::Incompatible)?
		.parse()
		.map_err(|_| StoreError::Incompatible)?;
	let known: i64 = row.get(2)?;
	let icon: Option<String> = row.get(3)?;
	let icon = match (known, icon) {
		(0, None) => None,
		(1, icon) => Some(icon),
		_ => return Err(StoreError::Incompatible),
	};
	let display = ServerDisplay {
		guild,
		name: row.get(1)?,
		icon,
	};
	if !display.valid() || (display.name.is_none() && display.icon.is_none()) {
		return Err(StoreError::Incompatible);
	}
	Ok(display)
}

#[cfg(test)]
mod tests {
	use super::*;
	use rusqlite::Connection;

	fn store() -> LocalStore {
		LocalStore::initialize(Connection::open_in_memory().unwrap()).unwrap()
	}
	fn entry(guild: u64, name: Option<&str>, icon: Option<Option<&str>>) -> ServerDisplay {
		ServerDisplay {
			guild: Id(guild),
			name: name.map(str::to_owned),
			icon: icon.map(|value| value.map(str::to_owned)),
		}
	}
	#[test]
	fn merges_sparse_fields_and_preserves_explicit_removal() {
		let mut db = store();
		let hash = "0123456789abcdef0123456789abcdef";
		db.save_server_displays(Id(1), &[entry(7, Some("Original"), Some(Some(hash)))])
			.unwrap();
		db.save_server_displays(Id(1), &[entry(7, None, Some(None))])
			.unwrap();
		assert_eq!(
			db.load_server_displays(Id(1)).unwrap(),
			vec![entry(7, Some("Original"), Some(None))]
		);
		db.save_server_displays(Id(1), &[entry(7, Some("Renamed"), None)])
			.unwrap();
		assert_eq!(
			db.load_server_displays(Id(1)).unwrap(),
			vec![entry(7, Some("Renamed"), Some(None))]
		);
		db.save_server_displays(Id(1), &[entry(8, None, None)])
			.unwrap();
		assert_eq!(db.load_server_displays(Id(1)).unwrap().len(), 1);
	}
	#[test]
	fn cache_is_account_scoped_and_clears_with_history_and_logout() {
		let mut db = store();
		db.save_server_displays(
			Id(1),
			&[entry(7, Some("First"), None), entry(8, Some("Other"), None)],
		)
		.unwrap();
		db.save_server_displays(Id(2), &[entry(7, Some("Second"), None)])
			.unwrap();
		db.clear_server_display(Id(1), Id(7)).unwrap();
		assert_eq!(
			db.load_server_displays(Id(1)).unwrap(),
			vec![entry(8, Some("Other"), None)]
		);
		assert_eq!(
			db.load_server_displays(Id(2)).unwrap(),
			vec![entry(7, Some("Second"), None)]
		);
		db.clear_history(Id(1)).unwrap();
		assert!(db.load_server_displays(Id(1)).unwrap().is_empty());
		db.forget_account(Id(2)).unwrap();
		assert!(db.load_server_displays(Id(2)).unwrap().is_empty());
	}
	#[test]
	fn rejects_invalid_entries_and_corrupt_stored_values() {
		let mut db = store();
		assert_eq!(
			db.save_server_displays(Id(1), &[entry(7, Some("valid"), Some(Some("invalid")))]),
			Err(StoreError::Capacity)
		);
		assert_eq!(
			db.save_server_displays(
				Id(1),
				&[entry(7, Some("same"), None), entry(7, Some("again"), None)]
			),
			Err(StoreError::Capacity)
		);
		db.save_server_displays(Id(1), &[entry(7, Some("ok"), None)])
			.unwrap();
		db.0.execute_batch(
			"PRAGMA ignore_check_constraints=ON; UPDATE server_displays SET name=zeroblob(1048576);",
		)
		.unwrap();
		assert!(db.load_server_displays(Id(1)).is_err());
		assert!(
			db.save_server_displays(Id(1), &[entry(7, Some("replacement"), None)])
				.is_err()
		);
	}
	#[test]
	fn enforces_global_entry_cap_and_migrates_existing_databases() {
		let mut db = store();
		db.0.execute_batch("DROP TABLE server_displays; PRAGMA user_version=30; INSERT INTO drafts VALUES('1','9','unchanged');").unwrap();
		db = LocalStore::initialize(db.0).unwrap();
		let rows: Vec<_> = (1..=LIMIT)
			.map(|guild| entry(guild as u64, Some("Name"), None))
			.collect();
		db.save_server_displays(Id(1), &rows).unwrap();
		db.save_server_displays(Id(2), &[entry(1, Some("New"), None)])
			.unwrap();
		let total: i64 =
			db.0.query_row("SELECT count(*) FROM server_displays", [], |r| r.get(0))
				.unwrap();
		assert_eq!(total, LIMIT as i64);
		assert_eq!(
			db.load_server_displays(Id(2)).unwrap(),
			vec![entry(1, Some("New"), None)]
		);
		let draft: String =
			db.0.query_row(
				"SELECT content FROM drafts WHERE account='1' AND channel='9'",
				[],
				|r| r.get(0),
			)
			.unwrap();
		assert_eq!(draft, "unchanged");
	}
	#[test]
	fn enforces_global_byte_budget_across_batches() {
		let mut db = store();
		let name = "😺".repeat(128);
		for batch in 0..3 {
			let rows: Vec<_> = (1..=1000)
				.map(|id| entry(batch * 1000 + id, Some(&name), None))
				.collect();
			db.save_server_displays(Id(1), &rows).unwrap();
		}
		let (count, bytes): (i64, i64) =
			db.0.query_row(
				&format!("SELECT count(*), sum({ROW_CHARGE}) FROM server_displays"),
				[],
				|r| Ok((r.get(0)?, r.get(1)?)),
			)
			.unwrap();
		assert!(bytes <= MAX_BYTES as i64);
		assert!(count < 3000);
		assert_eq!(
			db.load_server_displays(Id(1)).unwrap().len(),
			count as usize
		);
	}
	#[test]
	fn selective_load_is_bounded_account_scoped_and_rejects_corruption() {
		let mut db = store();
		db.save_server_displays(
			Id(1),
			&[
				entry(7, Some("Seven"), None),
				entry(8, Some("Eight"), Some(None)),
			],
		)
		.unwrap();
		db.save_server_displays(Id(2), &[entry(7, Some("Other"), None)])
			.unwrap();
		assert_eq!(
			db.load_server_displays_for(Id(1), &[Id(8), Id(999), Id(7)])
				.unwrap(),
			vec![
				entry(8, Some("Eight"), Some(None)),
				entry(7, Some("Seven"), None)
			]
		);
		assert_eq!(
			db.load_server_displays_for(Id(2), &[Id(7), Id(8)]).unwrap(),
			vec![entry(7, Some("Other"), None)]
		);
		assert_eq!(
			db.load_server_displays_for(Id(1), &[Id(7), Id(7)]),
			Err(StoreError::Capacity)
		);
		assert_eq!(
			db.load_server_displays_for(Id(1), &[Id(0)]),
			Err(StoreError::Capacity)
		);
		assert_eq!(
			db.load_server_displays_for(Id(1), &vec![Id(1); LIMIT + 1]),
			Err(StoreError::Capacity)
		);
		db.0.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE server_displays SET name=zeroblob(1048576) WHERE account='1' AND guild='7';").unwrap();
		assert!(db.load_server_displays_for(Id(1), &[Id(7)]).is_err());
		assert_eq!(
			db.load_server_displays_for(Id(1), &[Id(8)]).unwrap(),
			vec![entry(8, Some("Eight"), Some(None))]
		);
	}
}
