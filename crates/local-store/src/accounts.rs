//! Saved-account roster and account-scoped GIF preferences.
use super::{Id, LocalStore, Result, StoreError, params};

impl LocalStore {
	/// Saved GIF favorites for one account, newest first. Rejected rows are skipped.
	pub fn gif_favorites(&self, account: Id) -> Result<Vec<model::Gif>> {
		let mut statement = self.0.prepare(
			"SELECT id,title,url,preview,width,height FROM gif_favorites WHERE account=?1 ORDER BY position LIMIT 100",
		)?;
		let rows = statement.query_map([account.to_string()], |row| {
			Ok(model::Gif {
				id: row.get(0)?,
				title: row.get(1)?,
				url: row.get(2)?,
				preview: row.get(3)?,
				width: row.get::<_, u32>(4)?,
				height: row.get::<_, u32>(5)?,
			})
		})?;
		let mut favorites = Vec::new();
		for gif in rows {
			let gif = gif?;
			if gif.valid()
				&& !favorites
					.iter()
					.any(|known: &model::Gif| known.id == gif.id)
			{
				favorites.push(gif);
			}
		}
		Ok(favorites)
	}
	/// Replaces the account's favorites atomically; the list is bounded like the in-memory one.
	pub fn save_gif_favorites(&mut self, account: Id, favorites: &[model::Gif]) -> Result<()> {
		if favorites.len() > model::MAX_GIF_FAVORITES || !favorites.iter().all(model::Gif::valid) {
			return Err(StoreError::Capacity);
		}
		let transaction = self.0.transaction()?;
		transaction.execute(
			"DELETE FROM gif_favorites WHERE account=?1",
			[account.to_string()],
		)?;
		for (position, gif) in favorites.iter().enumerate() {
			transaction.execute(
				"INSERT INTO gif_favorites VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
				rusqlite::params![
					account.to_string(),
					position as i64,
					gif.id,
					gif.title,
					gif.url,
					gif.preview,
					gif.width,
					gif.height
				],
			)?;
		}
		transaction.commit()?;
		Ok(())
	}
	/// Switcher roster, most recently used first. Damaged rows are skipped, never fatal.
	pub fn accounts(&self) -> Result<Vec<model::SavedAccount>> {
		Ok(self.ordered_accounts()?.0)
	}
	/// Valid rows newest first, bounded, plus the IDs of every row that cannot produce a
	/// usable account. Filtering happens before the bound, so a row the switcher could never
	/// offer — a damaged ID or avatar written outside this client — cannot hide a real one.
	pub(super) fn ordered_accounts(&self) -> Result<(Vec<model::SavedAccount>, Vec<String>)> {
		let mut query = self.0.prepare(
			"SELECT account,name,display,avatar,discriminator,has_token FROM accounts ORDER BY touched DESC,account",
		)?;
		let mut rows = query.query([])?;
		let mut accounts = Vec::new();
		let mut damaged = Vec::new();
		while let Some(row) = rows.next()? {
			let stored: String = row.get(0)?;
			let account = row.get::<_, String>(0)?.parse::<Id>().ok().map(|id| {
				Ok::<_, rusqlite::Error>(model::SavedAccount {
					id,
					name: row.get(1)?,
					display: row.get(2)?,
					avatar: row.get(3)?,
					discriminator: row.get::<_, i64>(4)?.clamp(0, 9999) as u16,
					has_token: row.get::<_, i64>(5)? == 1,
				})
			});
			match account {
				Some(account) if account.as_ref().is_ok_and(model::SavedAccount::is_valid) => {
					accounts.push(account?);
				}
				_ => damaged.push(stored),
			}
		}
		accounts.truncate(model::MAX_SAVED_ACCOUNTS);
		Ok((accounts, damaged))
	}
	/// Records whether the credential store holds this account's own entry. Separate from
	/// `save_account` so an identity refresh can never claim a token that was never written.
	pub fn set_account_token(&self, account: Id, has_token: bool) -> Result<()> {
		self.0.execute(
			"UPDATE accounts SET has_token=?2 WHERE account=?1",
			params![account.to_string(), i64::from(has_token)],
		)?;
		Ok(())
	}
	/// Remembers one account and returns the IDs pruned to keep the roster bounded.
	/// Callers own removing the pruned accounts' credential-store entries.
	pub fn save_account(&mut self, account: &model::SavedAccount) -> Result<Vec<Id>> {
		if !account.is_valid() {
			return Err(StoreError::Capacity);
		}
		let transaction = self.0.transaction()?;
		transaction.execute(
			"INSERT INTO accounts(account,name,display,avatar,discriminator,touched) VALUES(?1,?2,?3,?4,?5,
             MAX(unixepoch('subsec')*1000,(SELECT IFNULL(MAX(touched),0)+1 FROM accounts)))
             ON CONFLICT(account) DO UPDATE SET name=excluded.name,display=excluded.display,avatar=excluded.avatar,discriminator=excluded.discriminator,touched=excluded.touched",
			params![
				account.id.to_string(),
				account.name,
				account.display,
				account.avatar,
				account.discriminator
			],
		)?;
		transaction.commit()?;
		// Keep the newest valid rows and drop everything else, damaged rows included, so a row
		// that cannot be offered never costs a real account its place.
		let (keep, damaged) = self.ordered_accounts()?;
		let keep: std::collections::BTreeSet<Id> = keep.into_iter().map(|a| a.id).collect();
		let transaction = self.0.transaction()?;
		let mut pruned = Vec::new();
		{
			let mut query = transaction.prepare("SELECT account FROM accounts")?;
			let mut rows = query.query([])?;
			while let Some(row) = rows.next()? {
				let stored: String = row.get(0)?;
				match stored.parse::<Id>() {
					Ok(id) if keep.contains(&id) => {}
					Ok(id) => pruned.push(id),
					Err(_) => {}
				}
			}
		}
		for id in pruned.iter().map(Id::to_string).chain(damaged) {
			transaction.execute("DELETE FROM accounts WHERE account=?1", [id])?;
		}
		transaction.commit()?;
		Ok(pruned)
	}
	pub fn forget_account(&mut self, account: Id) -> Result<()> {
		let transaction = self.0.transaction()?;
		for table in [
			"messages",
			"member_displays",
			"expression_catalogs",
			"sticker_pack_hints",
			"server_displays",
			"channels",
			"history_starts",
			"drafts",
			"gif_favorites",
			"channel_preferences",
			"account_presence",
			"accounts",
		] {
			transaction.execute(
				&format!("DELETE FROM {table} WHERE account=?1"),
				[account.to_string()],
			)?;
		}
		transaction.commit()?;
		self.0
			.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA incremental_vacuum(64);")?;
		Ok(())
	}
}
