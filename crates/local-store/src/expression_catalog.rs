//! Bounded account-scoped display hints; these never establish service permissions.
use super::{Id, LocalStore, Result, StoreError, params};
use model::{CustomEmoji, ExpressionCatalog, Sticker, StickerList, StickerPack, StickerPackHints};
use serde::de::{Error, SeqAccess, Visitor};
use std::{collections::BTreeSet, fmt};

const LIMIT: i64 = 128;
const MAX_FIELD: usize = 2 * 1024 * 1024;
const MAX_TOTAL: i64 = 8 * 1024 * 1024;
const MAX_PACK_TOTAL: i64 = 3 * 1024 * 1024;
const ROW_BYTES: &str =
	"128+coalesce(length(CAST(emojis AS BLOB)),0)+coalesce(length(CAST(stickers AS BLOB)),0)";
type CatalogFields = (Option<String>, Option<String>);

struct CachedEmojis(Vec<CustomEmoji>);
struct CachedRoles(Vec<Id>);
impl<'de> serde::Deserialize<'de> for CachedRoles {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
		struct Roles;
		impl<'de> Visitor<'de> for Roles {
			type Value = CachedRoles;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("at most 256 role identifiers")
			}
			fn visit_seq<A: SeqAccess<'de>>(
				self,
				mut seq: A,
			) -> std::result::Result<Self::Value, A::Error> {
				let mut values = Vec::new();
				while let Some(id) = seq.next_element::<Id>()? {
					if values.len() == 256 || id.0 == 0 {
						return Err(A::Error::custom("Invalid emoji roles"));
					}
					values.push(id);
				}
				Ok(CachedRoles(values))
			}
		}
		d.deserialize_seq(Roles)
	}
}
#[derive(serde::Deserialize)]
struct CachedEmoji {
	id: Id,
	#[serde(deserialize_with = "cached_name")]
	name: String,
	#[serde(default)]
	animated: bool,
	#[serde(default)]
	available: bool,
	#[serde(default)]
	managed: bool,
	#[serde(default)]
	roles: Option<CachedRoles>,
}
fn cached_name<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
	struct Name;
	impl<'de> Visitor<'de> for Name {
		type Value = String;
		fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
			f.write_str("a bounded emoji name")
		}
		fn visit_str<E: Error>(self, value: &str) -> std::result::Result<String, E> {
			if value.len() > 32 {
				return Err(E::custom("Emoji name capacity"));
			}
			Ok(value.to_owned())
		}
	}
	d.deserialize_str(Name)
}
fn cached_pack_name<'de, D: serde::Deserializer<'de>>(
	d: D,
) -> std::result::Result<String, D::Error> {
	struct Name;
	impl<'de> Visitor<'de> for Name {
		type Value = String;
		fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
			f.write_str("a bounded sticker pack name")
		}
		fn visit_str<E: Error>(self, value: &str) -> std::result::Result<String, E> {
			if value.len() > 256 {
				return Err(E::custom("Sticker pack name capacity"));
			}
			Ok(value.to_owned())
		}
	}
	d.deserialize_str(Name)
}

#[derive(serde::Deserialize)]
struct CachedPack {
	id: Id,
	#[serde(deserialize_with = "cached_pack_name")]
	name: String,
	stickers: StickerList<{ model::MAX_GUILD_STICKERS }>,
}
struct CachedPacks(Vec<StickerPack>);
impl<'de> serde::Deserialize<'de> for CachedPacks {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
		struct Packs;
		impl<'de> Visitor<'de> for Packs {
			type Value = CachedPacks;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a bounded sticker pack list")
			}
			fn visit_seq<A: SeqAccess<'de>>(
				self,
				mut seq: A,
			) -> std::result::Result<Self::Value, A::Error> {
				let mut values: Vec<StickerPack> = Vec::new();
				let mut ids = BTreeSet::new();
				let mut heap = 0usize;
				while let Some(pack) = seq.next_element::<CachedPack>()? {
					if values.len() == 128
						|| pack.id.0 == 0 || !ids.insert(pack.id)
						|| pack.stickers.0.iter().any(|s| {
							s.available || s.guild_id.is_some() || s.pack_id != Some(pack.id)
						}) {
						return Err(A::Error::custom("Invalid sticker pack"));
					}
					heap = heap
						.saturating_add(pack.name.capacity())
						.saturating_add(model::sticker_bytes(&pack.stickers.0));
					values.push(StickerPack {
						id: pack.id,
						name: pack.name,
						stickers: pack.stickers.0,
					});
					if heap.saturating_add(values.capacity() * size_of::<StickerPack>())
						> 1024 * 1024
					{
						return Err(A::Error::custom("Sticker pack capacity"));
					}
				}
				Ok(CachedPacks(values))
			}
		}
		d.deserialize_seq(Packs)
	}
}

fn decode_packs(raw: &str) -> Result<StickerPackHints> {
	if raw.len() > MAX_FIELD {
		return Err(StoreError::Capacity);
	}
	let packs = serde_json::from_str::<CachedPacks>(raw)
		.map_err(|_| StoreError::Incompatible)?
		.0;
	let hints = StickerPackHints { packs };
	if !hints.valid() {
		return Err(StoreError::Incompatible);
	}
	Ok(hints)
}
fn raw_packs(db: &rusqlite::Connection, account: Id) -> Result<Option<String>> {
	let mut stmt = db.prepare_cached("SELECT packs FROM sticker_pack_hints WHERE account=?1")?;
	let mut rows = stmt.query([account.to_string()])?;
	let Some(row) = rows.next()? else {
		return Ok(None);
	};
	read_text(row, 0)?.ok_or(StoreError::Incompatible).map(Some)
}
impl From<CachedEmoji> for CustomEmoji {
	fn from(value: CachedEmoji) -> Self {
		Self {
			id: value.id,
			name: value.name,
			animated: value.animated,
			available: value.available,
			managed: value.managed,
			roles: value.roles.map(|r| r.0),
		}
	}
}
impl<'de> serde::Deserialize<'de> for CachedEmojis {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
		struct List;
		impl<'de> Visitor<'de> for List {
			type Value = CachedEmojis;
			fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
				f.write_str("a bounded emoji catalog")
			}
			fn visit_seq<A: SeqAccess<'de>>(
				self,
				mut seq: A,
			) -> std::result::Result<Self::Value, A::Error> {
				let mut values = Vec::new();
				let mut ids = BTreeSet::new();
				while let Some(emoji) = seq.next_element::<CachedEmoji>()? {
					let emoji = CustomEmoji::from(emoji);
					if values.len() >= model::MAX_GUILD_EMOJIS
						|| !emoji.valid() || !ids.insert(emoji.id)
					{
						return Err(A::Error::custom("Invalid emoji catalog"));
					}
					values.push(emoji);
					if model::custom_emoji_bytes(&values) > model::MAX_GUILD_EMOJI_BYTES {
						return Err(A::Error::custom("Emoji catalog capacity"));
					}
				}
				Ok(CachedEmojis(values))
			}
		}
		d.deserialize_seq(List)
	}
}

fn decode_emojis(raw: &str) -> Result<Vec<CustomEmoji>> {
	if raw.len() > MAX_FIELD {
		return Err(StoreError::Capacity);
	}
	serde_json::from_str::<CachedEmojis>(raw)
		.map(|v| v.0)
		.map_err(|_| StoreError::Incompatible)
}
fn decode_stickers(raw: &str) -> Result<Vec<Sticker>> {
	if raw.len() > MAX_FIELD {
		return Err(StoreError::Capacity);
	}
	serde_json::from_str::<StickerList<{ model::MAX_GUILD_STICKERS }>>(raw)
		.map(|v| v.0)
		.map_err(|_| StoreError::Incompatible)
}
fn read_text(row: &rusqlite::Row<'_>, column: usize) -> Result<Option<String>> {
	match row.get_ref(column)? {
		rusqlite::types::ValueRef::Null => Ok(None),
		rusqlite::types::ValueRef::Text(bytes) if bytes.len() <= MAX_FIELD => {
			std::str::from_utf8(bytes)
				.map(|s| Some(s.to_owned()))
				.map_err(|_| StoreError::Incompatible)
		}
		rusqlite::types::ValueRef::Text(_) => Err(StoreError::Capacity),
		_ => Err(StoreError::Incompatible),
	}
}
fn raw_catalog(db: &rusqlite::Connection, account: Id, guild: Id) -> Result<Option<CatalogFields>> {
	let mut stmt = db.prepare_cached(
		"SELECT emojis,stickers FROM expression_catalogs WHERE account=?1 AND guild=?2",
	)?;
	let mut rows = stmt.query(params![account.to_string(), guild.to_string()])?;
	rows.next()?
		.map(|row| Ok((read_text(row, 0)?, read_text(row, 1)?)))
		.transpose()
}

/// Recover each field independently so damage to one kind cannot erase the other.
fn recover_catalog(
	db: &rusqlite::Connection,
	account: Id,
	guild: Id,
) -> Result<(Option<CatalogFields>, bool)> {
	let mut stmt = db.prepare_cached(
		"SELECT emojis,stickers FROM expression_catalogs WHERE account=?1 AND guild=?2",
	)?;
	let mut rows = stmt.query(params![account.to_string(), guild.to_string()])?;
	let Some(row) = rows.next()? else {
		return Ok((None, false));
	};
	let field = |index| match read_text(row, index) {
		Ok(value) => Ok((value, false)),
		Err(StoreError::Capacity | StoreError::Incompatible) => Ok((None, true)),
		Err(error) => Err(error),
	};
	let (emojis, bad_emojis) = field(0)?;
	let (stickers, bad_stickers) = field(1)?;
	Ok((Some((emojis, stickers)), bad_emojis || bad_stickers))
}

impl LocalStore {
	/// Save display-only standard sticker packs without making them sendable.
	pub fn save_sticker_pack_hints(&mut self, account: Id, hints: &StickerPackHints) -> Result<()> {
		if account.0 == 0 || !hints.valid() {
			return Err(StoreError::Capacity);
		}
		let raw = serde_json::to_string(&hints.packs).map_err(|_| StoreError::Incompatible)?;
		if raw.len() > MAX_FIELD {
			return Err(StoreError::Capacity);
		}
		let tx = self.0.transaction()?;
		let previous = match raw_packs(&tx, account) {
			Ok(value) => value,
			Err(StoreError::Capacity | StoreError::Incompatible) => None,
			Err(error) => return Err(error),
		};
		if previous.as_deref() == Some(raw.as_str()) {
			return Ok(());
		}
		let touched: i64 = tx.query_row(
			"SELECT coalesce(max(touched),0) FROM sticker_pack_hints",
			[],
			|row| row.get(0),
		)?;
		let touched = touched.checked_add(1).ok_or(StoreError::Incompatible)?;
		tx.execute(
			"DELETE FROM sticker_pack_hints WHERE account=?1",
			[account.to_string()],
		)?;
		let (mut count, mut bytes): (i64, i64) = tx.query_row(
			"SELECT count(*),coalesce(sum(96+length(CAST(packs AS BLOB))),0) FROM sticker_pack_hints",
			[],
			|row| Ok((row.get(0)?, row.get(1)?)),
		)?;
		let charge = 96 + raw.len() as i64;
		while count >= model::MAX_SAVED_ACCOUNTS as i64 || bytes + charge > MAX_PACK_TOTAL {
			let (old, size):(String,i64) = tx.query_row("SELECT account,96+length(CAST(packs AS BLOB)) FROM sticker_pack_hints ORDER BY touched,account LIMIT 1", [], |row|Ok((row.get(0)?,row.get(1)?)))?;
			tx.execute("DELETE FROM sticker_pack_hints WHERE account=?1", [old])?;
			count -= 1;
			bytes -= size;
		}
		tx.execute(
			"INSERT INTO sticker_pack_hints(account,packs,touched) VALUES(?1,?2,?3)",
			params![account.to_string(), raw, touched],
		)?;
		tx.commit()?;
		Ok(())
	}
	pub fn load_sticker_pack_hints(&self, account: Id) -> Result<Option<StickerPackHints>> {
		if account.0 == 0 {
			return Err(StoreError::Capacity);
		}
		raw_packs(&self.0, account)?
			.as_deref()
			.map(decode_packs)
			.transpose()
	}
	pub fn clear_sticker_pack_hints(&mut self, account: Id) -> Result<()> {
		if account.0 == 0 {
			return Err(StoreError::Capacity);
		}
		self.0.execute(
			"DELETE FROM sticker_pack_hints WHERE account=?1",
			[account.to_string()],
		)?;
		Ok(())
	}
	/// Merge supplied catalog fields; absence never erases a separately observed field.
	pub fn save_expression_catalog(
		&mut self,
		account: Id,
		catalog: &ExpressionCatalog,
	) -> Result<()> {
		if account.0 == 0 || !catalog.valid() {
			return Err(StoreError::Capacity);
		}
		if catalog.emojis.is_none() && catalog.stickers.is_none() {
			return Ok(());
		}
		let tx = self.0.transaction()?;
		let (old, corrupt_field) = recover_catalog(&tx, account, catalog.guild)?;
		let emojis = if let Some(values) = &catalog.emojis {
			Some(serde_json::to_string(values).map_err(|_| StoreError::Incompatible)?)
		} else {
			old.as_ref()
				.and_then(|(emojis, _)| emojis.as_ref())
				.filter(|raw| {
					decode_emojis(raw).is_ok_and(|values| {
						values
							.iter()
							.all(|e| !e.available && !e.managed && e.roles.is_none())
					})
				})
				.cloned()
		};
		let stickers = if let Some(values) = &catalog.stickers {
			Some(serde_json::to_string(values).map_err(|_| StoreError::Incompatible)?)
		} else {
			old.as_ref()
				.and_then(|(_, stickers)| stickers.as_ref())
				.filter(|raw| {
					decode_stickers(raw).is_ok_and(|values| {
						values
							.iter()
							.all(|s| !s.available && s.guild_id == Some(catalog.guild))
					})
				})
				.cloned()
		};
		if emojis.as_ref().is_some_and(|s| s.len() > MAX_FIELD)
			|| stickers.as_ref().is_some_and(|s| s.len() > MAX_FIELD)
		{
			return Err(StoreError::Capacity);
		}
		let merged = ExpressionCatalog {
			guild: catalog.guild,
			emojis: emojis.as_deref().map(decode_emojis).transpose()?,
			stickers: stickers.as_deref().map(decode_stickers).transpose()?,
		};
		if !merged.valid() {
			return Err(StoreError::Capacity);
		}
		if !corrupt_field
			&& old
				.as_ref()
				.is_some_and(|(e, s)| e == &emojis && s == &stickers)
		{
			return Ok(());
		}
		let touched: i64 = tx.query_row(
			"SELECT coalesce(max(touched),0) FROM expression_catalogs",
			[],
			|row| row.get(0),
		)?;
		let touched = touched.checked_add(1).ok_or(StoreError::Incompatible)?;
		tx.execute(
			"DELETE FROM expression_catalogs WHERE account=?1 AND guild=?2",
			params![account.to_string(), catalog.guild.to_string()],
		)?;
		let (mut count, mut bytes): (i64, i64) = tx.query_row(
			&format!("SELECT count(*),coalesce(sum({ROW_BYTES}),0) FROM expression_catalogs"),
			[],
			|row| Ok((row.get(0)?, row.get(1)?)),
		)?;
		let charge = 128
			+ emojis.as_ref().map_or(0, String::len) as i64
			+ stickers.as_ref().map_or(0, String::len) as i64;
		while count >= LIMIT || bytes + charge > MAX_TOTAL {
			let (a,g,size):(String,String,i64) = tx.query_row(&format!("SELECT account,guild,{ROW_BYTES} FROM expression_catalogs ORDER BY touched,account,guild LIMIT 1"), [], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
			tx.execute(
				"DELETE FROM expression_catalogs WHERE account=?1 AND guild=?2",
				params![a, g],
			)?;
			count -= 1;
			bytes -= size;
		}
		tx.execute(
			"INSERT INTO expression_catalogs(account,guild,emojis,stickers,touched) VALUES(?1,?2,?3,?4,?5)",
			params![
				account.to_string(),
				catalog.guild.to_string(),
				emojis,
				stickers,
				touched
			],
		)?;
		tx.commit()?;
		Ok(())
	}
	pub fn load_expression_catalog(
		&self,
		account: Id,
		guild: Id,
	) -> Result<Option<ExpressionCatalog>> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		let Some((emojis, stickers)) = raw_catalog(&self.0, account, guild)? else {
			return Ok(None);
		};
		let catalog = ExpressionCatalog {
			guild,
			emojis: emojis.as_deref().map(decode_emojis).transpose()?,
			stickers: stickers.as_deref().map(decode_stickers).transpose()?,
		};
		if !catalog.valid() {
			return Err(StoreError::Incompatible);
		}
		Ok(Some(catalog))
	}
	pub fn clear_expression_guild(&mut self, account: Id, guild: Id) -> Result<()> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		self.0.execute(
			"DELETE FROM expression_catalogs WHERE account=?1 AND guild=?2",
			params![account.to_string(), guild.to_string()],
		)?;
		Ok(())
	}
	/// Forget only the invalidated kind while retaining the other catalog.
	pub fn clear_expression_field(&mut self, account: Id, guild: Id, emoji: bool) -> Result<()> {
		if account.0 == 0 || guild.0 == 0 {
			return Err(StoreError::Capacity);
		}
		let tx = self.0.transaction()?;
		let sql = if emoji {
			"UPDATE expression_catalogs SET emojis=NULL WHERE account=?1 AND guild=?2 AND emojis IS NOT NULL"
		} else {
			"UPDATE expression_catalogs SET stickers=NULL WHERE account=?1 AND guild=?2 AND stickers IS NOT NULL"
		};
		tx.execute(sql, params![account.to_string(), guild.to_string()])?;
		tx.execute(
			"DELETE FROM expression_catalogs WHERE account=?1 AND guild=?2 AND emojis IS NULL AND stickers IS NULL",
			params![account.to_string(), guild.to_string()],
		)?;
		tx.commit()?;
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
	fn emoji(id: u64) -> CustomEmoji {
		CustomEmoji {
			id: Id(id),
			name: "example".into(),
			animated: false,
			available: false,
			managed: false,
			roles: None,
		}
	}
	fn sticker(guild: u64, id: u64) -> Sticker {
		Sticker {
			id: Id(id),
			name: "Sticker".into(),
			description: String::new(),
			tags: String::new(),
			format_type: 1,
			guild_id: Some(Id(guild)),
			pack_id: None,
			available: false,
		}
	}
	fn catalog(guild: u64) -> ExpressionCatalog {
		ExpressionCatalog {
			guild: Id(guild),
			emojis: Some(vec![emoji(1)]),
			stickers: Some(vec![sticker(guild, 2)]),
		}
	}
	fn pack_hints() -> StickerPackHints {
		let mut item = sticker(10, 5);
		item.guild_id = None;
		item.pack_id = Some(Id(8));
		StickerPackHints {
			packs: vec![StickerPack {
				id: Id(8),
				name: "Standard".into(),
				stickers: vec![item],
			}],
		}
	}
	#[test]
	fn pack_hints_roundtrip_empty_and_account_clear_are_scoped() {
		let mut db = store();
		let hints = pack_hints();
		db.save_sticker_pack_hints(Id(1), &hints).unwrap();
		db.save_sticker_pack_hints(Id(2), &hints).unwrap();
		assert_eq!(
			db.load_sticker_pack_hints(Id(1)).unwrap(),
			Some(hints.clone())
		);
		let empty = StickerPackHints { packs: vec![] };
		db.save_sticker_pack_hints(Id(1), &empty).unwrap();
		assert_eq!(db.load_sticker_pack_hints(Id(1)).unwrap(), Some(empty));
		assert_eq!(db.load_sticker_pack_hints(Id(2)).unwrap(), Some(hints));
		db.clear_history(Id(1)).unwrap();
		assert!(db.load_sticker_pack_hints(Id(1)).unwrap().is_none());
		assert!(db.load_sticker_pack_hints(Id(2)).unwrap().is_some());
		db.forget_account(Id(2)).unwrap();
		assert!(db.load_sticker_pack_hints(Id(2)).unwrap().is_none());
	}
	#[test]
	fn pack_hints_reject_forged_authority_corruption_and_excess() {
		let mut db = store();
		let mut hints = pack_hints();
		hints.packs[0].stickers[0].available = true;
		assert_eq!(
			db.save_sticker_pack_hints(Id(1), &hints),
			Err(StoreError::Capacity)
		);
		hints.packs[0].stickers[0].available = false;
		hints.packs[0].stickers[0].guild_id = Some(Id(10));
		assert_eq!(
			db.save_sticker_pack_hints(Id(1), &hints),
			Err(StoreError::Capacity)
		);
		let hints = pack_hints();
		db.save_sticker_pack_hints(Id(1), &hints).unwrap();
		db.0.execute_batch(
			"PRAGMA ignore_check_constraints=ON; UPDATE sticker_pack_hints SET packs=zeroblob(2097153);",
		)
		.unwrap();
		assert_eq!(
			db.load_sticker_pack_hints(Id(1)),
			Err(StoreError::Incompatible)
		);
		db.save_sticker_pack_hints(Id(1), &hints).unwrap();
		assert_eq!(db.load_sticker_pack_hints(Id(1)).unwrap(), Some(hints));
		let excess = (1..=129)
			.map(|id| format!("{{\"id\":\"{id}\",\"name\":\"pack\",\"stickers\":[]}}"))
			.collect::<Vec<_>>()
			.join(",");
		assert!(decode_packs(&format!("[{excess}]")).is_err());
	}
	#[test]
	fn pack_hints_obey_combined_byte_budget_without_cross_account_leaks() {
		let mut db = store();
		for account in 1..=8 {
			let mut hints = pack_hints();
			hints.packs[0].stickers = (1..=200)
				.map(|id| {
					let mut item = sticker(10, id);
					item.guild_id = None;
					item.pack_id = Some(Id(8));
					item.description = "a".repeat(2000);
					item
				})
				.collect();
			db.save_sticker_pack_hints(Id(account), &hints).unwrap();
		}
		let (count, total): (i64, i64) =
			db.0.query_row(
				"SELECT count(*),sum(96+length(CAST(packs AS BLOB))) FROM sticker_pack_hints",
				[],
				|row| Ok((row.get(0)?, row.get(1)?)),
			)
			.unwrap();
		assert!(count < model::MAX_SAVED_ACCOUNTS as i64);
		assert!(total <= MAX_PACK_TOTAL);
		assert!(db.load_sticker_pack_hints(Id(1)).unwrap().is_none());
		assert!(db.load_sticker_pack_hints(Id(8)).unwrap().is_some());
	}
	#[test]
	fn upgrade_roundtrip_and_partial_updates_preserve_account_scope() {
		let mut db = store();
		db.0.execute_batch("DROP TABLE expression_catalogs; PRAGMA user_version=30; INSERT INTO drafts VALUES('1','2','keep draft');").unwrap();
		db = LocalStore::initialize(db.0).unwrap();
		assert_eq!(
			db.0.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
				.unwrap(),
			31
		);
		let original = catalog(10);
		db.save_expression_catalog(Id(1), &original).unwrap();
		db.save_expression_catalog(Id(2), &original).unwrap();
		db.save_expression_catalog(
			Id(1),
			&ExpressionCatalog {
				guild: Id(10),
				emojis: Some(vec![]),
				stickers: None,
			},
		)
		.unwrap();
		let updated = db.load_expression_catalog(Id(1), Id(10)).unwrap().unwrap();
		assert_eq!(updated.emojis, Some(vec![]));
		assert_eq!(updated.stickers, original.stickers);
		assert_eq!(
			db.load_expression_catalog(Id(2), Id(10)).unwrap(),
			Some(original)
		);
		assert_eq!(db.load_drafts(Id(1)).unwrap()[&Id(2)], "keep draft");
	}
	#[test]
	fn scoped_deletion_and_clear_cannot_touch_other_accounts() {
		let mut db = store();
		for (account, guild) in [(1, 10), (1, 11), (2, 10)] {
			db.save_expression_catalog(Id(account), &catalog(guild))
				.unwrap();
		}
		db.clear_expression_guild(Id(1), Id(10)).unwrap();
		assert!(db.load_expression_catalog(Id(1), Id(10)).unwrap().is_none());
		assert!(db.load_expression_catalog(Id(1), Id(11)).unwrap().is_some());
		db.clear_history(Id(1)).unwrap();
		assert!(db.load_expression_catalog(Id(1), Id(11)).unwrap().is_none());
		assert!(db.load_expression_catalog(Id(2), Id(10)).unwrap().is_some());
		db.forget_account(Id(2)).unwrap();
		assert!(db.load_expression_catalog(Id(2), Id(10)).unwrap().is_none());
	}
	#[test]
	fn clearing_one_field_preserves_the_other_and_other_scopes() {
		let mut db = store();
		let first = catalog(10);
		let other = catalog(11);
		db.save_expression_catalog(Id(1), &first).unwrap();
		db.save_expression_catalog(Id(1), &other).unwrap();
		db.save_expression_catalog(Id(2), &first).unwrap();
		db.clear_expression_field(Id(1), Id(10), true).unwrap();
		let after = db.load_expression_catalog(Id(1), Id(10)).unwrap().unwrap();
		assert_eq!(after.emojis, None);
		assert_eq!(after.stickers, first.stickers);
		db.save_expression_catalog(Id(1), &first).unwrap();
		db.clear_expression_field(Id(1), Id(10), false).unwrap();
		let after = db.load_expression_catalog(Id(1), Id(10)).unwrap().unwrap();
		assert_eq!(after.stickers, None);
		assert_eq!(after.emojis, first.emojis);
		db.clear_expression_field(Id(1), Id(10), true).unwrap();
		assert!(db.load_expression_catalog(Id(1), Id(10)).unwrap().is_none());
		assert_eq!(
			db.load_expression_catalog(Id(1), Id(11)).unwrap(),
			Some(other)
		);
		assert_eq!(
			db.load_expression_catalog(Id(2), Id(10)).unwrap(),
			Some(first)
		);
	}
	#[test]
	fn malformed_and_oversized_rows_fail_closed_and_fresh_updates_recover() {
		let mut db = store();
		db.save_expression_catalog(Id(1), &catalog(10)).unwrap();
		db.0.execute_batch(
			"PRAGMA ignore_check_constraints=ON; UPDATE expression_catalogs SET emojis=zeroblob(2097153);",
		)
		.unwrap();
		assert_eq!(
			db.load_expression_catalog(Id(1), Id(10)),
			Err(StoreError::Incompatible)
		);
		db.save_expression_catalog(
			Id(1),
			&ExpressionCatalog {
				guild: Id(10),
				emojis: Some(vec![emoji(3)]),
				stickers: None,
			},
		)
		.unwrap();
		assert_eq!(
			db.load_expression_catalog(Id(1), Id(10))
				.unwrap()
				.unwrap()
				.emojis,
			Some(vec![emoji(3)])
		);
		assert_eq!(
			db.load_expression_catalog(Id(1), Id(10))
				.unwrap()
				.unwrap()
				.stickers,
			Some(vec![sticker(10, 2)]),
			"repairing a damaged emoji field must preserve the intact sticker catalog"
		);
		assert!(
			decode_emojis(&format!(
				"[{{\"id\":\"1\",\"name\":\"ok\",\"roles\":[{}]}}]",
				vec!["\"1\""; 257].join(",")
			))
			.is_err()
		);
		let excess = (1..=1001)
			.map(|id| format!("{{\"id\":\"{id}\",\"name\":\"ok\"}}"))
			.collect::<Vec<_>>()
			.join(",");
		assert!(decode_emojis(&format!("[{excess}]")).is_err());
		db.0.execute(
			"UPDATE expression_catalogs SET emojis=?1",
			["[{\"id\":\"1\",\"name\":\"ok\",\"available\":true}]"],
		)
		.unwrap();
		assert_eq!(
			db.load_expression_catalog(Id(1), Id(10)),
			Err(StoreError::Incompatible)
		);
	}
	#[test]
	fn cache_evicts_oldest_and_does_not_rewrite_unchanged_rows() {
		let mut db = store();
		db.save_expression_catalog(Id(1), &catalog(1)).unwrap();
		db.0.execute_batch("CREATE TABLE writes(n INTEGER NOT NULL); INSERT INTO writes VALUES(0); CREATE TRIGGER ec_insert AFTER INSERT ON expression_catalogs BEGIN UPDATE writes SET n=n+1; END; CREATE TRIGGER ec_delete AFTER DELETE ON expression_catalogs BEGIN UPDATE writes SET n=n+1; END;").unwrap();
		db.save_expression_catalog(Id(1), &catalog(1)).unwrap();
		assert_eq!(
			db.0.query_row("SELECT n FROM writes", [], |r| r.get::<_, i64>(0))
				.unwrap(),
			0
		);
		for guild in 2..=129 {
			db.save_expression_catalog(Id(1), &catalog(guild)).unwrap();
		}
		assert!(db.load_expression_catalog(Id(1), Id(1)).unwrap().is_none());
		assert!(
			db.load_expression_catalog(Id(1), Id(129))
				.unwrap()
				.is_some()
		);
		assert_eq!(
			db.0.query_row("SELECT count(*) FROM expression_catalogs", [], |r| r
				.get::<_, i64>(0))
				.unwrap(),
			LIMIT
		);
	}
	#[test]
	fn aggregate_byte_limit_evicts_and_cached_hints_cannot_assert_authority() {
		let mut db = store();
		let mut forged = catalog(1);
		forged.emojis.as_mut().unwrap()[0].available = true;
		assert_eq!(
			db.save_expression_catalog(Id(1), &forged),
			Err(StoreError::Capacity)
		);
		let mut forged = catalog(1);
		forged.stickers.as_mut().unwrap()[0].available = true;
		assert_eq!(
			db.save_expression_catalog(Id(1), &forged),
			Err(StoreError::Capacity)
		);
		for guild in 1..=40 {
			let values = (1..=80)
				.map(|id| {
					let mut value = sticker(guild, id);
					value.description = "a".repeat(3000);
					value
				})
				.collect();
			db.save_expression_catalog(
				Id(1),
				&ExpressionCatalog {
					guild: Id(guild),
					emojis: None,
					stickers: Some(values),
				},
			)
			.unwrap();
		}
		let total: i64 =
			db.0.query_row(
				&format!("SELECT sum({ROW_BYTES}) FROM expression_catalogs"),
				[],
				|row| row.get(0),
			)
			.unwrap();
		assert!(total <= MAX_TOTAL);
		assert!(db.load_expression_catalog(Id(1), Id(1)).unwrap().is_none());
		assert!(db.load_expression_catalog(Id(1), Id(40)).unwrap().is_some());
	}
}
