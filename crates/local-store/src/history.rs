//! Bounded, account-isolated message history and drafts.
use super::{
	BTreeMap, BTreeSet, CachedEmbeds, CachedMentions, Id, LocalStore, MAX_MEDIA_JSON,
	MAX_WINDOW_BYTES, Message, OptionalExtension, Result, StoreError, User, params,
	parse_author_roles,
};

impl LocalStore {
	/// Apply a bounded changed-row batch. `retained` describes the display window, not disk retention.
	/// `start`: the saved window reaches the channel's first message.
	pub fn save_changes(
		&mut self,
		account: Id,
		channel: Id,
		messages: &[Message],
		retained: &[Id],
		start: bool,
	) -> Result<()> {
		if retained.len() > 500
			|| messages.len() > 500
			|| messages.iter().map(Message::bytes).sum::<usize>() > MAX_WINDOW_BYTES
		{
			return Err(StoreError::Capacity);
		}
		let retained: std::collections::BTreeSet<_> = retained.iter().copied().collect();
		let existing = self.load_channel(account, channel)?;
		let mut window: BTreeMap<_, _> = existing
			.iter()
			.filter(|m| retained.contains(&m.id))
			.map(|m| (m.id, m))
			.collect();
		for message in messages {
			if !retained.contains(&message.id) {
				return Err(StoreError::Capacity);
			}
			window.insert(message.id, message);
		}
		self.save_channel_loaded(
			account,
			channel,
			&window.into_values().collect::<Vec<_>>(),
			&existing,
			start,
		)
	}
	/// `start`: the saved window reaches the channel's first message.
	pub fn save_channel(
		&mut self,
		account: Id,
		channel: Id,
		messages: &[Message],
		start: bool,
	) -> Result<()> {
		if messages.len() > 500 {
			return Err(StoreError::Capacity);
		}
		let existing = self.load_channel(account, channel)?;
		self.save_channel_loaded(
			account,
			channel,
			&messages.iter().collect::<Vec<_>>(),
			&existing,
			start,
		)
	}
	pub(super) fn save_channel_loaded(
		&mut self,
		account: Id,
		channel: Id,
		messages: &[&Message],
		existing: &[Message],
		start: bool,
	) -> Result<()> {
		let transaction = self.0.transaction()?;
		write_messages(&transaction, account, channel, messages, existing, start)?;
		transaction.commit()?;
		Ok(())
	}

	/// Whether a cached window that begins with `messages` reaches the channel's first message.
	pub fn history_start(&self, account: Id, channel: Id, messages: &[Message]) -> Result<bool> {
		let Some(first) = messages.iter().map(|message| message.id).min() else {
			return Ok(false);
		};
		let saved: Option<String> = self
			.0
			.query_row(
				"SELECT first FROM history_starts WHERE account=?1 AND channel=?2",
				params![account.to_string(), channel.to_string()],
				|row| row.get(0),
			)
			.optional()?;
		Ok(saved.is_some_and(|saved| saved == first.to_string()))
	}
	/// Latest bounded view, in chronological order. Older synchronized pages stay on disk.
	pub fn load_channel(&self, account: Id, channel: Id) -> Result<Vec<Message>> {
		self.load_history_page(account, channel, crate::HistoryPageQuery::default())
	}
	/// Indexed, bounded history read; supplying `around` centers on that target when available.
	pub fn load_history_page(
		&self,
		account: Id,
		channel: Id,
		page: crate::HistoryPageQuery,
	) -> Result<Vec<Message>> {
		if account.0 == 0
			|| channel.0 == 0
			|| page.limit == 0
			|| page.limit > 500
			|| usize::from(page.before.is_some())
				+ usize::from(page.after.is_some())
				+ usize::from(page.around.is_some())
				> 1 || page.before.or(page.around).is_some_and(|id| id.0 == 0)
		{
			return Err(StoreError::Capacity);
		}
		use rusqlite::types::Value;
		let columns = "SELECT id,author,name,content,edited,reply,unsupported,avatar,discriminator,embeds,embeds_suppressed,attachments,mentions,extra_content,message_kind,reply_deleted,webhook,account_kind,forwarded,author_roles,author_nick,components,application_id,original_flags,sticker_items,interaction,reactions,verified_bot FROM messages WHERE account=?1 AND channel=?2";
		let mut values = vec![
			Value::Text(account.to_string()),
			Value::Text(channel.to_string()),
		];
		let sql = if let Some(around) = page.around {
			values.extend([
				Value::Text(around.to_string()),
				Value::Integer(page.limit.div_ceil(2) as i64),
				Value::Integer((page.limit / 2) as i64),
				Value::Integer(page.limit as i64),
			]);
			format!(
				"{columns} AND id IN (SELECT id FROM (SELECT id FROM messages WHERE account=?1 AND channel=?2 AND (length(id),id)<=(length(?3),?3) ORDER BY length(id) DESC,id DESC LIMIT ?4) UNION ALL SELECT id FROM (SELECT id FROM messages WHERE account=?1 AND channel=?2 AND (length(id),id)>(length(?3),?3) ORDER BY length(id),id LIMIT ?5)) ORDER BY length(id) DESC,id DESC LIMIT ?6"
			)
		} else if let Some(after) = page.after {
			values.extend([
				Value::Text(after.to_string()),
				Value::Integer(page.limit as i64),
			]);
			format!("{columns} AND (length(id),id)>(length(?3),?3) ORDER BY length(id),id LIMIT ?4")
		} else if let Some(before) = page.before {
			values.extend([
				Value::Text(before.to_string()),
				Value::Integer(page.limit as i64),
			]);
			format!(
				"{columns} AND (length(id),id)<(length(?3),?3) ORDER BY length(id) DESC,id DESC LIMIT ?4"
			)
		} else {
			values.push(Value::Integer(page.limit as i64));
			format!("{columns} ORDER BY length(id) DESC,id DESC LIMIT ?3")
		};
		let mut query = self.0.prepare_cached(&sql)?;
		let mut rows = query.query(rusqlite::params_from_iter(values))?;
		let mut messages = Vec::new();
		let mut bytes = 0;
		while let Some(row) = rows.next()? {
			let mut account_kind = match row.get_ref(17)? {
				rusqlite::types::ValueRef::Integer(0) => model::AccountKind::Human,
				rusqlite::types::ValueRef::Integer(1) => model::AccountKind::Bot,
				rusqlite::types::ValueRef::Integer(2) => model::AccountKind::App,
				_ => return Err(StoreError::Incompatible),
			};
			match row.get_ref(27)? {
				rusqlite::types::ValueRef::Integer(0) => {}
				rusqlite::types::ValueRef::Integer(1)
					if account_kind == model::AccountKind::Bot =>
				{
					account_kind = model::AccountKind::VerifiedBot;
				}
				_ => return Err(StoreError::Incompatible),
			}
			let webhook = match row.get_ref(16)? {
				rusqlite::types::ValueRef::Integer(0) => false,
				rusqlite::types::ValueRef::Integer(1) => true,
				_ => return Err(StoreError::Incompatible),
			};
			let reply_deleted = match row.get_ref(15)? {
				rusqlite::types::ValueRef::Integer(0) => false,
				rusqlite::types::ValueRef::Integer(1) => true,
				_ => return Err(StoreError::Incompatible),
			};
			let extra_content = row
				.get_ref(13)?
				.as_i64()
				.ok()
				.and_then(|bits| u8::try_from(bits).ok())
				.and_then(model::ExtraContent::from_bits)
				.ok_or(StoreError::Incompatible)?;
			// Inspect borrowed SQLite fields before allocating attacker-controlled cache strings.
			for (column, maximum) in [
				(0, 20),
				(1, 20),
				(2, 512),
				(3, 64 * 1024),
				(9, MAX_MEDIA_JSON),
				(11, MAX_MEDIA_JSON),
				(12, 128 * 1024),
				(19, 16 * 1024),
				(21, MAX_MEDIA_JSON),
				(23, 20),
				(24, 32768),
			] {
				if row
					.get_ref(column)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?
					.len() > maximum
				{
					return Err(StoreError::Capacity);
				}
			}
			for (column, maximum) in [(5, 20), (7, 34), (20, 512), (22, 20), (25, 4096)] {
				if !matches!(row.get_ref(column)?, rusqlite::types::ValueRef::Null)
					&& row
						.get_ref(column)?
						.as_str()
						.map_err(|_| StoreError::Incompatible)?
						.len() > maximum
				{
					return Err(StoreError::Capacity);
				}
			}
			let embeds = serde_json::from_str::<CachedEmbeds>(
				row.get_ref(9)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?,
			)
			.map_err(|_| StoreError::Incompatible)?
			.0;
			let attachments = serde_json::from_str::<model::AttachmentList>(
				row.get_ref(11)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?,
			)
			.map_err(|_| StoreError::Incompatible)?
			.0;
			let mentions = serde_json::from_str::<CachedMentions>(
				row.get_ref(12)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?,
			)
			.map_err(|_| StoreError::Incompatible)?
			.0;
			if !model::valid_mentions(&mentions)
				|| !model::valid_embeds(&embeds)
				|| !model::valid_attachments(&attachments)
			{
				return Err(StoreError::Capacity);
			}
			let parse = |value: String| value.parse::<Id>().map_err(|_| StoreError::Incompatible);
			let author_roles = parse_author_roles(
				row.get_ref(19)?
					.as_str()
					.map_err(|_| StoreError::Incompatible)?,
			)?;
			let author_nick = match row.get_ref(20)? {
				rusqlite::types::ValueRef::Null => None,
				rusqlite::types::ValueRef::Text(bytes) => {
					let nick = std::str::from_utf8(bytes).map_err(|_| StoreError::Incompatible)?;
					if nick.is_empty() {
						None
					} else {
						Some(nick.chars().take(128).collect())
					}
				}
				_ => return Err(StoreError::Incompatible),
			};
			let flags_text = row
				.get_ref(23)?
				.as_str()
				.map_err(|_| StoreError::Incompatible)?;
			if flags_text.is_empty() || !flags_text.bytes().all(|b| b.is_ascii_digit()) {
				return Err(StoreError::Incompatible);
			}
			let flags = flags_text
				.parse::<u64>()
				.map_err(|_| StoreError::Incompatible)?;
			if flags & 64 != 0 {
				return Err(StoreError::Incompatible);
			}
			let interaction = match row.get_ref(25)? {
				rusqlite::types::ValueRef::Null => None,
				rusqlite::types::ValueRef::Text(bytes) => {
					let interaction = serde_json::from_slice::<model::Interaction>(bytes)
						.map_err(|_| StoreError::Incompatible)?;
					if interaction.user.name.len() > 512 || interaction.command.len() > 256 {
						return Err(StoreError::Capacity);
					}
					Some(Box::new(interaction))
				}
				_ => return Err(StoreError::Incompatible),
			};
			let reactions = match row.get_ref(26)? {
				rusqlite::types::ValueRef::Null => None,
				rusqlite::types::ValueRef::Text(bytes) => {
					let text = std::str::from_utf8(bytes).map_err(|_| StoreError::Incompatible)?;
					if text.len() > 16384 {
						return Err(StoreError::Capacity);
					}
					let reactions = serde_json::from_str::<Vec<model::Reaction>>(text)
						.map_err(|_| StoreError::Incompatible)?;
					if !model::valid_reactions(&reactions) {
						return Err(StoreError::Incompatible);
					}
					Some(reactions)
				}
				_ => return Err(StoreError::Incompatible),
			};
			let message = Message {
				reply_preview: None,
				poll: None,
				sticker_items: serde_json::from_str::<
					model::StickerList<{ model::MAX_MESSAGE_STICKERS }>,
				>(
					row.get_ref(24)?
						.as_str()
						.map_err(|_| StoreError::Incompatible)?,
				)
				.map_err(|_| StoreError::Incompatible)?
				.0,
				flags,
				ephemeral: false,
				application_id: row.get::<_, Option<String>>(22)?.map(parse).transpose()?,
				components: serde_json::from_str::<model::ComponentList>(
					row.get_ref(21)?
						.as_str()
						.map_err(|_| StoreError::Incompatible)?,
				)
				.map_err(|_| StoreError::Incompatible)?
				.0,
				reactions,
				id: parse(row.get(0)?)?,
				channel,
				author: User {
					member_avatar: None,
					username: None,
					primary_guild: None,
					cosmetics: None,
					id: parse(row.get(1)?)?,
					name: row.get(2)?,
					avatar: row.get(7)?,
					webhook,
					kind: account_kind,
					discriminator: row.get(8)?,
				},
				content: row.get(3)?,
				edited: row.get(4)?,
				edited_at: None,
				reply_to: row.get::<_, Option<String>>(5)?.map(parse).transpose()?,
				unsupported: row.get(6)?,
				extra_content,
				kind: row.get(14)?,
				reply_deleted,
				forwarded: row.get(18)?,
				interaction,
				nonce: None,
				revision: 0,
				embeds,
				author_nick,
				author_roles,
				mention_roles: vec![],
				mention_everyone: false,
				suppress_notifications: false,
				mentions,
				embeds_suppressed: row.get(10)?,
				attachments,
			};
			if message.reply_deleted
				&& (!matches!(message.kind, 19 | 23)
					|| !message
						.reply_to
						.is_some_and(|id| id.0 > 0 && id < message.id))
			{
				return Err(StoreError::Incompatible);
			}
			bytes += message.bytes();
			if bytes > MAX_WINDOW_BYTES {
				if messages.is_empty() {
					return Err(StoreError::Capacity);
				}
				break;
			}
			messages.push(message);
		}
		if page.after.is_none() {
			messages.reverse();
		}
		Ok(messages)
	}
	pub fn save_draft(&mut self, account: Id, channel: Id, content: &str) -> Result<()> {
		if content.len() > model::message_options::MAX_DRAFT_CONTENT_BYTES {
			return Err(StoreError::Capacity);
		}
		let transaction = self.0.transaction()?;
		transaction.execute(
			"DELETE FROM drafts WHERE account=?1 AND channel=?2",
			params![account.to_string(), channel.to_string()],
		)?;
		if !content.is_empty() {
			transaction.execute(
				"INSERT INTO drafts VALUES(?1,?2,?3)",
				params![account.to_string(), channel.to_string(), content],
			)?;
		}
		let (count, bytes): (i64, i64) = transaction.query_row(
			"SELECT count(*),coalesce(sum(length(CAST(content AS BLOB))),0) FROM drafts",
			[],
			|r| Ok((r.get(0)?, r.get(1)?)),
		)?;
		if count > 64 || bytes > 2 * 1024 * 1024 {
			return Err(StoreError::Capacity);
		}
		transaction.commit()?;
		Ok(())
	}
	pub fn load_drafts(&self, account: Id) -> Result<BTreeMap<Id, String>> {
		let mut query = self
			.0
			.prepare("SELECT channel,content FROM drafts WHERE account=?1 LIMIT 64")?;
		let rows = query.query_map([account.to_string()], |r| {
			Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
		})?;
		let mut drafts = BTreeMap::new();
		let mut bytes = 0;
		for row in rows {
			let (id, content) = row?;
			bytes += content.len();
			if bytes > 2 * 1024 * 1024
				|| content.len() > model::message_options::MAX_DRAFT_CONTENT_BYTES
			{
				return Err(StoreError::Capacity);
			}
			drafts.insert(id.parse().map_err(|_| StoreError::Incompatible)?, content);
		}
		Ok(drafts)
	}
	pub fn clear_history(&mut self, account: Id) -> Result<()> {
		self.cleanup_cache_scope(account, None, false)
	}
	/// Remove only this channel's cached data. Drafts and other channels survive.
	pub fn clear_channel_history(&mut self, account: Id, channel: Id) -> Result<()> {
		self.cleanup_cache_scope(account, Some(channel), false)
	}

	pub fn delete_messages(&mut self, account: Id, channel: Id, ids: &[Id]) -> Result<()> {
		if ids.len() > 100 || account.0 == 0 || channel.0 == 0 || ids.iter().any(|id| id.0 == 0) {
			return Err(StoreError::Capacity);
		}
		let charge = self.cache_row_charge("messages", None)?;
		let mut statement = self.0.prepare_cached(&format!(
			"SELECT {charge} FROM messages WHERE account=?1 AND channel=?2 AND id=?3"
		))?;
		let mut chunks = Vec::new();
		let mut chunk = Vec::new();
		let mut bytes = 0i64;
		for id in ids {
			let charge: Option<i64> = statement
				.query_row(
					params![account.to_string(), channel.to_string(), id.to_string()],
					|row| row.get(0),
				)
				.optional()?;
			let charge = charge.unwrap_or(256);
			if charge < 0 {
				return Err(StoreError::Incompatible);
			}
			if !chunk.is_empty()
				&& bytes.saturating_add(charge) > crate::cache_cleanup::CLEANUP_BYTES
			{
				chunks.push(std::mem::take(&mut chunk));
				bytes = 0;
			}
			chunk.push(*id);
			bytes = bytes.saturating_add(charge);
		}
		drop(statement);
		if !chunk.is_empty() {
			chunks.push(chunk);
		}
		for chunk in chunks {
			let transaction = self.0.transaction()?;
			for id in chunk {
				crate::sync_history::observe_change(&transaction, account, channel, id, true)?;
			}
			transaction.commit()?;
			self.reclaim_history_space()?;
		}
		Ok(())
	}
}

/// Shared transaction writer for ordinary windows and Force Sync pages. Absence from a
/// small display window is not deletion evidence; only verified ranges or live deletes prune.
pub(super) fn write_messages(
	transaction: &rusqlite::Transaction<'_>,
	account: Id,
	channel: Id,
	messages: &[&Message],
	existing: &[Message],
	start: bool,
) -> Result<usize> {
	if messages.len() > 500
		|| messages
			.iter()
			.map(|message| message.bytes())
			.sum::<usize>()
			> MAX_WINDOW_BYTES
		|| messages.iter().any(|m| {
			m.channel != channel
				|| (m.reply_deleted
					&& (!matches!(m.kind, 19 | 23)
						|| !m.reply_to.is_some_and(|id| id.0 > 0 && id < m.id)))
				|| !model::valid_mentions(&m.mentions)
				|| m.ephemeral
				|| m.flags & 64 != 0
				|| m.application_id.is_some_and(|id| id.0 == 0)
				|| !model::valid_stickers(&m.sticker_items, model::MAX_MESSAGE_STICKERS)
				|| !model::valid_components(&m.components)
				|| !model::valid_embeds(&m.embeds)
				|| !model::valid_attachments(&m.attachments)
				|| m.reactions
					.as_ref()
					.is_some_and(|reactions| !model::valid_reactions(reactions))
		}) {
		return Err(StoreError::Capacity);
	}
	let account = account.to_string();
	let channel = channel.to_string();
	let previous: BTreeMap<_, _> = existing.iter().map(|m| (m.id, m)).collect();
	let mut written = 0;
	let mut insert = transaction.prepare_cached(
            "INSERT INTO messages(account,channel,id,author,name,content,edited,reply,unsupported,avatar,discriminator,embeds,embeds_suppressed,attachments,mentions,extra_content,message_kind,reply_deleted,webhook,account_kind,forwarded,author_roles,author_nick,components,application_id,original_flags,sticker_items,interaction,reactions,verified_bot) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30) ON CONFLICT(account,channel,id) DO UPDATE SET author=excluded.author,name=excluded.name,content=excluded.content,edited=excluded.edited,reply=excluded.reply,unsupported=excluded.unsupported,avatar=excluded.avatar,discriminator=excluded.discriminator,embeds=excluded.embeds,embeds_suppressed=excluded.embeds_suppressed,attachments=excluded.attachments,mentions=excluded.mentions,extra_content=excluded.extra_content,message_kind=excluded.message_kind,reply_deleted=excluded.reply_deleted,webhook=excluded.webhook,account_kind=excluded.account_kind,forwarded=excluded.forwarded,author_roles=excluded.author_roles,author_nick=excluded.author_nick,components=excluded.components,application_id=excluded.application_id,original_flags=excluded.original_flags,sticker_items=excluded.sticker_items,interaction=excluded.interaction,reactions=excluded.reactions,verified_bot=excluded.verified_bot")?;
	for message in messages {
		let tombstone: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM history_tombstones WHERE account=?1 AND channel=?2 AND id=?3)",
			params![account, channel, message.id.to_string()],
			|row| row.get(0),
		)?;
		if tombstone {
			continue;
		}
		if previous
			.get(&message.id)
			.is_some_and(|old| same_persisted_message(old, message))
		{
			continue;
		}
		let mentions =
			serde_json::to_string(&message.mentions).map_err(|_| StoreError::Incompatible)?;
		if mentions.len() > 128 * 1024 {
			return Err(StoreError::Capacity);
		}
		if message.author_roles.len() > model::permissions::MAX_MEMBER_ROLES
			|| message.author_roles.iter().any(|role| role.0 == 0)
		{
			return Err(StoreError::Capacity);
		}
		{
			let mut seen = BTreeSet::new();
			if message.author_roles.iter().any(|role| !seen.insert(*role)) {
				return Err(StoreError::Capacity);
			}
		}
		let author_roles = serde_json::to_string(
			&message
				.author_roles
				.iter()
				.map(|role| role.to_string())
				.collect::<Vec<_>>(),
		)
		.map_err(|_| StoreError::Incompatible)?;
		if author_roles.len() > 16 * 1024 {
			return Err(StoreError::Capacity);
		}
		if message
			.author_nick
			.as_ref()
			.is_some_and(|nick| nick.len() > 512)
		{
			return Err(StoreError::Capacity);
		}
		let sticker_items =
			serde_json::to_string(&message.sticker_items).map_err(|_| StoreError::Incompatible)?;
		if sticker_items.len() > 32768 {
			return Err(StoreError::Capacity);
		}
		let interaction = message
			.interaction
			.as_deref()
			.map(serde_json::to_string)
			.transpose()
			.map_err(|_| StoreError::Incompatible)?;
		if interaction.as_ref().is_some_and(|json| json.len() > 4096) {
			return Err(StoreError::Capacity);
		}
		let reactions = message
			.reactions
			.as_ref()
			.map(serde_json::to_string)
			.transpose()
			.map_err(|_| StoreError::Incompatible)?;
		if reactions.as_ref().is_some_and(|json| json.len() > 16384) {
			return Err(StoreError::Capacity);
		}
		let components =
			serde_json::to_string(&message.components).map_err(|_| StoreError::Incompatible)?;
		if components.len() > MAX_MEDIA_JSON {
			return Err(StoreError::Capacity);
		}
		let embeds =
			serde_json::to_string(&message.embeds).map_err(|_| StoreError::Incompatible)?;
		let attachments =
			serde_json::to_string(&message.attachments).map_err(|_| StoreError::Incompatible)?;
		if embeds.len() > MAX_MEDIA_JSON || attachments.len() > MAX_MEDIA_JSON {
			return Err(StoreError::Capacity);
		}
		let verified_bot = message.author.kind == model::AccountKind::VerifiedBot;
		let account_kind = if verified_bot {
			model::AccountKind::Bot
		} else {
			message.author.kind
		};
		written += insert.execute(params![
			account,
			channel,
			message.id.to_string(),
			message.author.id.to_string(),
			message.author.name,
			message.content,
			message.edited,
			message.reply_to.map(|id| id.to_string()),
			message.unsupported,
			message.author.avatar,
			message.author.discriminator,
			embeds,
			message.embeds_suppressed,
			attachments,
			mentions,
			message.extra_content.bits(),
			message.kind,
			message.reply_deleted,
			message.author.webhook,
			account_kind as u8,
			message.forwarded,
			author_roles,
			message.author_nick.as_deref(),
			components,
			message.application_id.map(|id| id.to_string()),
			message.flags.to_string(),
			sticker_items,
			interaction,
			reactions,
			verified_bot,
		])?;
	}
	drop(insert);
	transaction.execute("INSERT INTO channels VALUES(?1,?2,unixepoch('subsec')*1000) ON CONFLICT(account,channel) DO UPDATE SET touched=excluded.touched",params![account,channel])?;
	// The start holds only while the window still begins at the same first message.
	match messages
		.iter()
		.map(|message| message.id)
		.min()
		.filter(|_| start)
	{
		Some(first) => transaction.execute(
			"INSERT OR REPLACE INTO history_starts VALUES(?1,?2,?3)",
			params![account, channel, first.to_string()],
		)?,
		None => 0,
	};
	let count: i64 = transaction.query_row(
		"SELECT count FROM history_totals WHERE singleton=1",
		[],
		|row| row.get(0),
	)?;
	if count > crate::sync_history::MAX_CACHED_MESSAGES as i64 {
		return Err(StoreError::Capacity);
	}
	Ok(written)
}

/// Compare exactly the fields written by this store. Service revisions, reply previews,
/// usernames, presence and cosmetics have no representation in the history table.
fn same_persisted_user(a: &User, b: &User) -> bool {
	a.id == b.id
		&& a.name == b.name
		&& a.avatar == b.avatar
		&& a.discriminator == b.discriminator
		&& a.kind == b.kind
		&& a.webhook == b.webhook
}
fn same_persisted_message(a: &Message, b: &Message) -> bool {
	a.id == b.id
		&& a.channel == b.channel
		&& same_persisted_user(&a.author, &b.author)
		&& a.content == b.content
		&& a.edited == b.edited
		&& a.reply_to == b.reply_to
		&& a.unsupported == b.unsupported
		&& a.extra_content == b.extra_content
		&& a.kind == b.kind
		&& a.reply_deleted == b.reply_deleted
		&& a.forwarded == b.forwarded
		&& a.embeds == b.embeds
		&& a.embeds_suppressed == b.embeds_suppressed
		&& a.attachments == b.attachments
		&& a.author_roles == b.author_roles
		&& a.author_nick == b.author_nick
		&& a.components == b.components
		&& a.application_id == b.application_id
		&& a.flags == b.flags
		&& a.sticker_items == b.sticker_items
		&& a.reactions == b.reactions
		&& a.mentions.len() == b.mentions.len()
		&& a.mentions
			.iter()
			.zip(&b.mentions)
			.all(|(a, b)| same_persisted_user(a, b))
		&& match (&a.interaction, &b.interaction) {
			(None, None) => true,
			(Some(a), Some(b)) => a.command == b.command && same_persisted_user(&a.user, &b.user),
			_ => false,
		}
}

#[cfg(test)]
mod projection_tests {
	use super::*;
	use rusqlite::Connection;
	fn fixture() -> (LocalStore, Message) {
		let store = LocalStore::initialize(Connection::open_in_memory().unwrap()).unwrap();
		store.0.execute("INSERT INTO messages(account,channel,id,author,name,content,edited,unsupported) VALUES('1','2','30','4','Display','body',0,0)",[]).unwrap();
		let m = store.load_channel(Id(1), Id(2)).unwrap().remove(0);
		store.0.execute_batch("CREATE TABLE write_count(n INTEGER); INSERT INTO write_count VALUES(0); CREATE TRIGGER count_messages AFTER INSERT ON messages BEGIN UPDATE write_count SET n=n+1; END; CREATE TRIGGER count_message_updates AFTER UPDATE ON messages BEGIN UPDATE write_count SET n=n+1; END;").unwrap();
		(store, m)
	}
	fn writes(store: &LocalStore) -> i64 {
		store
			.0
			.query_row("SELECT n FROM write_count", [], |r| r.get(0))
			.unwrap()
	}
	#[test]
	fn session_only_metadata_does_not_rewrite_identical_persisted_rows() {
		let (mut store, mut message) = fixture();
		message.author.username = Some("actual_username".into());
		message.nonce = Some("not persisted".into());
		message.revision = 99;
		message.edited_at = Some(123);
		message.mention_roles = vec![Id(9)];
		message.mention_everyone = true;
		message.suppress_notifications = true;
		store
			.save_channel(Id(1), Id(2), std::slice::from_ref(&message), false)
			.unwrap();
		store
			.save_changes(Id(1), Id(2), &[message], &[Id(30)], false)
			.unwrap();
		assert_eq!(writes(&store), 0);
		let mut message = store.load_channel(Id(1), Id(2)).unwrap().remove(0);
		message.mentions = vec![message.author.clone()];
		message.interaction = Some(Box::new(model::Interaction {
			user: message.author.clone(),
			command: "command".into(),
		}));
		store
			.save_channel(Id(1), Id(2), std::slice::from_ref(&message), false)
			.unwrap();
		assert_eq!(writes(&store), 1);
		message.mentions[0].username = Some("session name".into());
		message.interaction.as_mut().unwrap().user.username = Some("session name".into());
		store
			.save_channel(Id(1), Id(2), std::slice::from_ref(&message), false)
			.unwrap();
		assert_eq!(writes(&store), 1);
	}
	#[test]
	fn each_persisted_field_still_writes_a_changed_row() {
		for field in 0..29 {
			let (mut store, original) = fixture();
			let mut m = original.clone();
			match field {
				0 => m.id = Id(31),
				1 => m.channel = Id(3),
				2 => m.author.id = Id(5),
				3 => m.author.name = "changed".into(),
				4 => m.author.avatar = Some("a".repeat(32)),
				5 => m.author.discriminator = 1234,
				6 => m.author.kind = model::AccountKind::VerifiedBot,
				7 => m.author.webhook = true,
				8 => m.content = "changed".into(),
				9 => m.edited = true,
				10 => m.reply_to = Some(Id(10)),
				11 => m.unsupported = true,
				12 => m.extra_content = model::ExtraContent::from_bits(1).unwrap(),
				13 => m.kind = 19,
				14 => {
					m.kind = 19;
					m.reply_to = Some(Id(10));
					m.reply_deleted = true;
				}
				15 => m.forwarded = true,
				16 => m.author_roles = vec![Id(10)],
				17 => m.author_nick = Some("nickname".into()),
				18 => {
					m.components = vec![model::Component {
						kind: 2,
						label: Some("Press".into()),
						style: Some(1),
						custom_id: Some("button".into()),
						..Default::default()
					}]
				}
				19 => m.application_id = Some(Id(10)),
				20 => m.flags = 4,
				21 => {
					m.sticker_items = vec![model::Sticker {
						id: Id(10),
						name: "Wave".into(),
						description: String::new(),
						tags: String::new(),
						format_type: 1,
						guild_id: None,
						pack_id: None,
						available: true,
					}]
				}
				22 => {
					m.interaction = Some(Box::new(model::Interaction {
						user: m.author.clone(),
						command: "changed".into(),
					}))
				}
				23 => {
					m.reactions = Some(vec![model::Reaction {
						emoji: model::ReactionEmoji {
							id: None,
							name: Some("👍".into()),
						},
						count: 1,
						me: false,
						me_burst: false,
					}])
				}
				24 => m.mentions = vec![m.author.clone()],
				25 => {
					m.embeds = vec![model::Embed {
						title: Some("title".into()),
						..Default::default()
					}]
				}
				26 => m.embeds_suppressed = true,
				27 => {
					m.attachments = vec![model::Attachment {
						id: Id(11),
						filename: "image.png".into(),
						description: None,
						content_type: Some("image/png".into()),
						size: 100,
						media: Default::default(),
						spoiler: false,
						duration_ms: None,
						waveform: vec![],
					}]
				}
				28 => m.author.kind = model::AccountKind::App,
				_ => unreachable!(),
			}
			assert!(!same_persisted_message(&original, &m), "field {field}");
			store
				.save_channel(Id(1), m.channel, std::slice::from_ref(&m), false)
				.unwrap();
			assert_eq!(writes(&store), 1, "field {field}");
			let loaded = store.load_channel(Id(1), m.channel).unwrap();
			assert!(
				same_persisted_message(loaded.iter().find(|row| row.id == m.id).unwrap(), &m),
				"field {field} roundtrip"
			);
		}
	}
	#[test]
	fn scoped_history_cleanup_keeps_unrelated_channels_and_accounts() {
		let (mut store, message) = fixture();
		let mut another = message.clone();
		another.channel = Id(3);
		store
			.save_channel(Id(1), Id(3), std::slice::from_ref(&another), true)
			.unwrap();
		store
			.save_channel(Id(9), Id(2), std::slice::from_ref(&message), true)
			.unwrap();
		store.save_draft(Id(1), Id(2), "draft survives").unwrap();
		store.clear_channel_history(Id(1), Id(2)).unwrap();
		assert!(store.load_channel(Id(1), Id(2)).unwrap().is_empty());
		assert_eq!(store.load_channel(Id(1), Id(3)).unwrap().len(), 1);
		assert_eq!(store.load_channel(Id(9), Id(2)).unwrap().len(), 1);
		assert_eq!(store.load_drafts(Id(1)).unwrap()[&Id(2)], "draft survives");
	}
}
