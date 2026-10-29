//! Transactional initialization and the unchanged historical schema migrations.
use super::{Connection, LocalStore, NATIVE_SCHEMA, READABLE_SCHEMA, Result, StoreError};

impl LocalStore {
	pub(super) fn initialize(mut connection: Connection) -> Result<Self> {
		connection.busy_timeout(std::time::Duration::from_secs(2))?;
		let version: u32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
		if version > READABLE_SCHEMA {
			return Err(StoreError::Incompatible);
		}
		connection.execute_batch("PRAGMA page_size=4096; PRAGMA max_page_count=16384; PRAGMA cache_size=-2048; PRAGMA temp_store=MEMORY; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=8388608; PRAGMA secure_delete=ON; PRAGMA auto_vacuum=INCREMENTAL;
            CREATE TABLE IF NOT EXISTS messages(account TEXT NOT NULL,channel TEXT NOT NULL,id TEXT NOT NULL,author TEXT NOT NULL,name TEXT NOT NULL,content TEXT NOT NULL,edited INTEGER NOT NULL,reply TEXT,unsupported INTEGER NOT NULL,PRIMARY KEY(account,channel,id));
            CREATE INDEX IF NOT EXISTS messages_channel_order ON messages(account,channel,length(id),id);
            CREATE TABLE IF NOT EXISTS channels(account TEXT NOT NULL,channel TEXT NOT NULL,touched INTEGER NOT NULL,PRIMARY KEY(account,channel));
            CREATE TABLE IF NOT EXISTS history_starts(account TEXT NOT NULL,channel TEXT NOT NULL,first TEXT NOT NULL CHECK(length(first) BETWEEN 1 AND 20),PRIMARY KEY(account,channel));
            CREATE TABLE IF NOT EXISTS drafts(account TEXT NOT NULL,channel TEXT NOT NULL,content TEXT NOT NULL,PRIMARY KEY(account,channel));
            CREATE TABLE IF NOT EXISTS appearance(singleton INTEGER PRIMARY KEY CHECK(singleton=1),theme TEXT NOT NULL CHECK(theme IN ('light','dark')));
            CREATE TABLE IF NOT EXISTS theme_variant(singleton INTEGER PRIMARY KEY CHECK(singleton=1),variant TEXT NOT NULL CHECK(length(variant) BETWEEN 1 AND 32));
            CREATE TABLE IF NOT EXISTS gif_favorites(account TEXT NOT NULL,position INTEGER NOT NULL CHECK(typeof(position)='integer' AND position BETWEEN 0 AND 99),id TEXT NOT NULL CHECK(length(id) BETWEEN 1 AND 64),title TEXT NOT NULL CHECK(length(title) <= 256),url TEXT NOT NULL CHECK(length(url) BETWEEN 1 AND 512),preview TEXT NOT NULL CHECK(length(preview) BETWEEN 1 AND 512),width INTEGER NOT NULL CHECK(typeof(width)='integer' AND width BETWEEN 1 AND 4096),height INTEGER NOT NULL CHECK(typeof(height)='integer' AND height BETWEEN 1 AND 4096),PRIMARY KEY(account,position));
            ")?;
		let has_avatar: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='avatar')",
			[],
			|row| row.get(0),
		)?;
		if !has_avatar {
			connection.execute_batch("BEGIN; ALTER TABLE messages ADD COLUMN avatar TEXT; ALTER TABLE messages ADD COLUMN discriminator INTEGER NOT NULL DEFAULT 0; PRAGMA user_version=3; COMMIT;")?;
		}
		let has_embeds: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='embeds')",
			[],
			|row| row.get(0),
		)?;
		if !has_embeds {
			connection.execute_batch("BEGIN; ALTER TABLE messages ADD COLUMN embeds TEXT NOT NULL DEFAULT '[]'; ALTER TABLE messages ADD COLUMN embeds_suppressed INTEGER NOT NULL DEFAULT 0; PRAGMA user_version=4; COMMIT;")?;
		}
		let has_attachments: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='attachments')",
			[],
			|row| row.get(0),
		)?;
		if !has_attachments {
			connection.execute_batch("BEGIN; ALTER TABLE messages ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]'; PRAGMA user_version=5; COMMIT;")?;
		} else if version < 5 {
			connection.pragma_update(None, "user_version", 5)?;
		}
		let has_mentions: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='mentions')",
			[],
			|row| row.get(0),
		)?;
		if !has_mentions {
			connection.execute_batch("BEGIN; ALTER TABLE messages ADD COLUMN mentions TEXT NOT NULL DEFAULT '[]'; PRAGMA user_version=6; COMMIT;")?;
		} else if version < 6 {
			connection.pragma_update(None, "user_version", 6)?;
		}
		// Schema 7 was used independently for markers and system-message kinds.
		// Detect both columns and commit their union with reading preferences atomically.
		let has_extra_content: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='extra_content')",
			[],
			|row| row.get(0),
		)?;
		let has_message_kind: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='message_kind')",
			[],
			|row| row.get(0),
		)?;
		let has_reply_deleted: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='reply_deleted')",
			[],
			|row| row.get(0),
		)?;
		let has_account_kind: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='account_kind')",
			[],
			|row| row.get(0),
		)?;
		let has_forwarded: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='forwarded')",
			[],
			|row| row.get(0),
		)?;
		let has_webhook: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='webhook')",
			[],
			|row| row.get(0),
		)?;
		let has_verified_bot: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='verified_bot')",
			[],
			|row| row.get(0),
		)?;
		let has_stickers: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='sticker_items')",
			[],
			|row| row.get(0),
		)?;
		let has_reactions: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='reactions')",
			[],
			|row| row.get(0),
		)?;
		let has_components: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='components')",
			[],
			|row| row.get(0),
		)?;
		let has_application_id: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='application_id')",
			[],
			|row| row.get(0),
		)?;
		let has_original_flags: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='original_flags')",
			[],
			|row| row.get(0),
		)?;
		let has_interaction: bool = connection.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='interaction')",
			[],
			|row| row.get(0),
		)?;
		let transaction = connection.transaction()?;
		if !has_interaction {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN interaction TEXT CHECK(interaction IS NULL OR length(CAST(interaction AS BLOB))<=4096);")?;
		}
		if !has_original_flags {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN original_flags TEXT NOT NULL DEFAULT '0' CHECK(typeof(original_flags)='text' AND length(CAST(original_flags AS BLOB)) BETWEEN 1 AND 20);")?;
		}

		if !has_application_id {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN application_id TEXT;")?;
		}
		if !has_stickers {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN sticker_items TEXT NOT NULL DEFAULT '[]' CHECK(length(CAST(sticker_items AS BLOB))<=32768);")?;
		}
		if !has_reactions {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN reactions TEXT CHECK(reactions IS NULL OR (typeof(reactions)='text' AND length(CAST(reactions AS BLOB))<=16384));")?;
		}
		if !has_components {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN components TEXT NOT NULL DEFAULT '[]' CHECK(length(CAST(components AS BLOB))<=262144);")?;
		}

		if !has_forwarded {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN forwarded INTEGER NOT NULL DEFAULT 0 CHECK(typeof(forwarded)='integer' AND forwarded IN (0,1));")?;
		}
		if !has_account_kind {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN account_kind INTEGER NOT NULL DEFAULT 0 CHECK(typeof(account_kind)='integer' AND account_kind BETWEEN 0 AND 2);")?;
		}
		if !has_webhook {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN webhook INTEGER NOT NULL DEFAULT 0 CHECK(typeof(webhook)='integer' AND webhook IN (0,1));")?;
		}
		if !has_verified_bot {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN verified_bot INTEGER NOT NULL DEFAULT 0 CHECK(typeof(verified_bot)='integer' AND verified_bot IN (0,1));")?;
		}
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS app_preferences(
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            value TEXT NOT NULL CHECK(typeof(value)='text' AND length(CAST(value AS BLOB))<=16384));")?;
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS custom_font(
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            name TEXT NOT NULL CHECK(typeof(name)='text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 128),
            data BLOB NOT NULL CHECK(typeof(data)='blob' AND length(data) BETWEEN 1 AND 33554432));")?;
		if version < 27 {
			transaction.execute_batch("CREATE TABLE custom_font_v27(
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                name TEXT NOT NULL CHECK(typeof(name)='text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 128),
                data BLOB NOT NULL CHECK(typeof(data)='blob' AND length(data) BETWEEN 1 AND 33554432));
                INSERT INTO custom_font_v27 SELECT * FROM custom_font;
                DROP TABLE custom_font;
                ALTER TABLE custom_font_v27 RENAME TO custom_font;")?;
		}
		if !has_reply_deleted {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN reply_deleted INTEGER NOT NULL DEFAULT 0 CHECK(typeof(reply_deleted)='integer' AND reply_deleted IN (0,1));")?;
		}
		if !has_extra_content {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN extra_content INTEGER NOT NULL DEFAULT 0 CHECK(typeof(extra_content)='integer' AND extra_content BETWEEN 0 AND 31);")?;
		}
		if !has_message_kind {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN message_kind INTEGER NOT NULL DEFAULT 0 CHECK(typeof(message_kind)='integer' AND message_kind BETWEEN 0 AND 255); UPDATE messages SET message_kind=255 WHERE unsupported<>0;")?;
		}
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS reading_preferences(
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                zoom_percent INTEGER NOT NULL CHECK(typeof(zoom_percent)='integer' AND zoom_percent BETWEEN 50 AND 150),
                sidebar_width INTEGER NOT NULL CHECK(typeof(sidebar_width)='integer' AND sidebar_width BETWEEN 190 AND 360),
                show_members INTEGER NOT NULL CHECK(typeof(show_members)='integer' AND show_members IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS game_activity(
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                enabled INTEGER NOT NULL CHECK(typeof(enabled)='integer' AND enabled IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS minimize_to_tray(
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                enabled INTEGER NOT NULL CHECK(typeof(enabled)='integer' AND enabled IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS channel_preferences(
                account TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL CHECK(typeof(value)='text' AND length(CAST(value AS BLOB))<=8192)
            );
            CREATE TABLE IF NOT EXISTS account_presence(
                account TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('online','idle','dnd','invisible')),
                custom_status TEXT NOT NULL CHECK(typeof(custom_status)='text' AND length(CAST(custom_status AS BLOB))<=512),
                expires INTEGER CHECK(expires IS NULL OR (typeof(expires)='integer' AND expires>=0))
            );
            CREATE TABLE IF NOT EXISTS accounts(
                account TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL CHECK(typeof(name)='text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 64),
                display TEXT CHECK(display IS NULL OR (typeof(display)='text' AND length(CAST(display AS BLOB)) BETWEEN 1 AND 64)),
                avatar TEXT CHECK(avatar IS NULL OR (typeof(avatar)='text' AND length(avatar) BETWEEN 1 AND 34)),
                discriminator INTEGER NOT NULL CHECK(typeof(discriminator)='integer' AND discriminator BETWEEN 0 AND 9999),
                touched INTEGER NOT NULL CHECK(typeof(touched)='integer'),
                has_token INTEGER NOT NULL DEFAULT 0 CHECK(typeof(has_token)='integer' AND has_token IN (0,1))
            );")?;
		let has_token_column: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('accounts') WHERE name='has_token')",
			[],
			|row| row.get(0),
		)?;
		if !has_token_column {
			// Rows predating the flag came from a build that wrote a per-account entry on every
			// connect, so their entries exist. Claiming otherwise would rewrite each one, which
			// on macOS is an access-controlled keychain operation; a wrong claim self-heals on
			// the next switch instead.
			transaction.execute_batch("ALTER TABLE accounts ADD COLUMN has_token INTEGER NOT NULL DEFAULT 0 CHECK(typeof(has_token)='integer' AND has_token IN (0,1)); UPDATE accounts SET has_token=1;")?;
		}
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS member_displays(
            account TEXT NOT NULL CHECK(length(account) BETWEEN 1 AND 20),
            guild TEXT NOT NULL CHECK(length(guild) BETWEEN 1 AND 20),
            user TEXT NOT NULL CHECK(length(user) BETWEEN 1 AND 20),
            name TEXT NOT NULL CHECK(typeof(name)='text' AND length(CAST(name AS BLOB)) BETWEEN 1 AND 512),
            username TEXT CHECK(username IS NULL OR (typeof(username)='text' AND length(CAST(username AS BLOB))<=512)),
            nick TEXT CHECK(nick IS NULL OR (typeof(nick)='text' AND length(CAST(nick AS BLOB))<=512)),
            touched INTEGER NOT NULL CHECK(typeof(touched)='integer'),
            PRIMARY KEY(account,guild,user));
            CREATE INDEX IF NOT EXISTS member_displays_recency ON member_displays(touched,account,guild,user);")?;
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS expression_catalogs(
            account TEXT NOT NULL CHECK(length(account) BETWEEN 1 AND 20),
            guild TEXT NOT NULL CHECK(length(guild) BETWEEN 1 AND 20),
            emojis TEXT CHECK(emojis IS NULL OR (typeof(emojis)='text' AND length(CAST(emojis AS BLOB))<=2097152)),
            stickers TEXT CHECK(stickers IS NULL OR (typeof(stickers)='text' AND length(CAST(stickers AS BLOB))<=2097152)),
            touched INTEGER NOT NULL CHECK(typeof(touched)='integer'),
            PRIMARY KEY(account,guild));
            CREATE INDEX IF NOT EXISTS expression_catalogs_recency ON expression_catalogs(touched,account,guild);")?;
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS sticker_pack_hints(
            account TEXT PRIMARY KEY NOT NULL CHECK(length(account) BETWEEN 1 AND 20),
            packs TEXT NOT NULL CHECK(typeof(packs)='text' AND length(CAST(packs AS BLOB))<=2097152),
            touched INTEGER NOT NULL CHECK(typeof(touched)='integer'));")?;
		transaction.execute_batch("CREATE TABLE IF NOT EXISTS server_displays(
            account TEXT NOT NULL CHECK(length(account) BETWEEN 1 AND 20),
            guild TEXT NOT NULL CHECK(length(guild) BETWEEN 1 AND 20),
            name TEXT CHECK(name IS NULL OR (typeof(name)='text' AND length(CAST(name AS BLOB))<=512)),
            icon_known INTEGER NOT NULL CHECK(typeof(icon_known)='integer' AND icon_known IN (0,1)),
            icon TEXT CHECK(icon IS NULL OR (typeof(icon)='text' AND length(CAST(icon AS BLOB))<=34)),
            touched INTEGER NOT NULL CHECK(typeof(touched)='integer'),
            CHECK(icon_known=1 OR icon IS NULL),
            PRIMARY KEY(account,guild));
            CREATE INDEX IF NOT EXISTS server_displays_recency ON server_displays(touched,account,guild);")?;
		transaction.pragma_update(None, "user_version", version.max(NATIVE_SCHEMA))?;
		let has_animate_gifs: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='animate_gifs')",
			[],
			|row| row.get(0),
		)?;
		if !has_animate_gifs {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN animate_gifs INTEGER NOT NULL DEFAULT 0 CHECK(typeof(animate_gifs)='integer' AND animate_gifs IN (0,1));")?;
		}
		let has_hide_media_links: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='hide_media_links')", [], |row| row.get(0),
		)?;
		if !has_hide_media_links {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN hide_media_links INTEGER NOT NULL DEFAULT 1 CHECK(typeof(hide_media_links)='integer' AND hide_media_links IN (0,1));")?;
		}
		let has_confirm_external_links: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='confirm_external_links')", [], |row| row.get(0),
        )?;
		if !has_confirm_external_links {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN confirm_external_links INTEGER NOT NULL DEFAULT 1 CHECK(typeof(confirm_external_links)='integer' AND confirm_external_links IN (0,1));")?;
		}
		let has_smooth_scrolling: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='smooth_scrolling')", [], |row| row.get(0),
		)?;
		if !has_smooth_scrolling {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN smooth_scrolling INTEGER NOT NULL DEFAULT 1 CHECK(typeof(smooth_scrolling)='integer' AND smooth_scrolling IN (0,1));")?;
		}
		let has_scroll_speed: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='scroll_speed_percent')", [], |row| row.get(0),
		)?;
		if !has_scroll_speed {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN scroll_speed_percent INTEGER NOT NULL DEFAULT 100 CHECK(typeof(scroll_speed_percent)='integer' AND scroll_speed_percent BETWEEN 25 AND 300);")?;
		}
		let has_show_members_dms: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='show_members_dms')", [], |row| row.get(0),
		)?;
		if !has_show_members_dms {
			// Direct messages start from the single member-list choice saved before the split.
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN show_members_dms INTEGER NOT NULL DEFAULT 1 CHECK(typeof(show_members_dms)='integer' AND show_members_dms IN (0,1)); UPDATE reading_preferences SET show_members_dms=show_members;")?;
		}
		let has_compact_messages: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='compact_messages')", [], |row| row.get(0),
		)?;
		if !has_compact_messages {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN compact_messages INTEGER NOT NULL DEFAULT 0 CHECK(typeof(compact_messages)='integer' AND compact_messages IN (0,1));")?;
		}
		if version < 26 {
			// SQLite cannot alter a CHECK constraint. Preserve all fixed-size settings
			// in the same transaction that upgrades the schema.
			transaction.execute_batch("CREATE TABLE reading_preferences_zoom(
				singleton INTEGER PRIMARY KEY CHECK(singleton=1),
				zoom_percent INTEGER NOT NULL CHECK(typeof(zoom_percent)='integer' AND zoom_percent BETWEEN 50 AND 150),
				sidebar_width INTEGER NOT NULL CHECK(typeof(sidebar_width)='integer' AND sidebar_width BETWEEN 190 AND 360),
				show_members INTEGER NOT NULL CHECK(typeof(show_members)='integer' AND show_members IN (0,1)),
				animate_gifs INTEGER NOT NULL DEFAULT 0 CHECK(typeof(animate_gifs)='integer' AND animate_gifs IN (0,1)),
				hide_media_links INTEGER NOT NULL DEFAULT 1 CHECK(typeof(hide_media_links)='integer' AND hide_media_links IN (0,1)),
				confirm_external_links INTEGER NOT NULL DEFAULT 1 CHECK(typeof(confirm_external_links)='integer' AND confirm_external_links IN (0,1)),
				smooth_scrolling INTEGER NOT NULL DEFAULT 1 CHECK(typeof(smooth_scrolling)='integer' AND smooth_scrolling IN (0,1)),
				scroll_speed_percent INTEGER NOT NULL DEFAULT 100 CHECK(typeof(scroll_speed_percent)='integer' AND scroll_speed_percent BETWEEN 25 AND 300),
				show_members_dms INTEGER NOT NULL DEFAULT 1 CHECK(typeof(show_members_dms)='integer' AND show_members_dms IN (0,1)),
				compact_messages INTEGER NOT NULL DEFAULT 0 CHECK(typeof(compact_messages)='integer' AND compact_messages IN (0,1))
			); INSERT INTO reading_preferences_zoom SELECT singleton,zoom_percent,sidebar_width,show_members,animate_gifs,hide_media_links,confirm_external_links,smooth_scrolling,scroll_speed_percent,show_members_dms,compact_messages FROM reading_preferences;
			DROP TABLE reading_preferences; ALTER TABLE reading_preferences_zoom RENAME TO reading_preferences;")?;
		}
		let has_double_click_reaction: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='double_click_reaction')", [], |row| row.get(0),
		)?;
		if !has_double_click_reaction {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN double_click_reaction INTEGER NOT NULL DEFAULT 0 CHECK(typeof(double_click_reaction)='integer' AND double_click_reaction BETWEEN 0 AND 5);")?;
		}
		let has_double_click_enabled: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('reading_preferences') WHERE name='double_click_reaction_enabled')", [], |row| row.get(0),
		)?;
		if !has_double_click_enabled {
			transaction.execute_batch("ALTER TABLE reading_preferences ADD COLUMN double_click_reaction_enabled INTEGER NOT NULL DEFAULT 0 CHECK(typeof(double_click_reaction_enabled)='integer' AND double_click_reaction_enabled IN (0,1));")?;
		}
		let has_author_roles: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='author_roles')",
			[],
			|row| row.get(0),
		)?;
		if !has_author_roles {
			transaction.execute_batch(
				"ALTER TABLE messages ADD COLUMN author_roles TEXT NOT NULL DEFAULT '[]';",
			)?;
		}
		let has_author_nick: bool = transaction.query_row(
			"SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='author_nick')",
			[],
			|row| row.get(0),
		)?;
		if !has_author_nick {
			transaction.execute_batch("ALTER TABLE messages ADD COLUMN author_nick TEXT;")?;
		}
		transaction.commit()?;
		Ok(Self(connection))
	}
}
