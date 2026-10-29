//! Bounded, presentation-only disk hints. Live catalogs remain the sole send authority.
use crate::{State, auth::AuthState};
use model::{CustomEmoji, ExpressionCatalog, Id, Sticker, StickerPack, StickerPackHints};
use std::collections::BTreeMap;

const MAX_HINT_GUILDS: usize = 128;
const MAX_HINT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct Hints {
	catalogs: BTreeMap<Id, ExpressionCatalog>,
	revision: u64,
	epoch: u64,
	guild_revisions: BTreeMap<Id, u64>,
	packs: Option<StickerPackHints>,
	pack_revision: u64,
}
impl Hints {
	pub(crate) fn bytes(&self) -> usize {
		self.catalogs
			.values()
			.map(ExpressionCatalog::bytes)
			.sum::<usize>()
			+ self.catalogs.len() * 64
			+ self.guild_revisions.len() * 64
			+ self.packs.as_ref().map_or(0, StickerPackHints::bytes)
	}
	fn reset_catalogs(&mut self) -> bool {
		let existed = !self.catalogs.is_empty() || !self.guild_revisions.is_empty();
		self.catalogs.clear();
		self.guild_revisions.clear();
		self.revision = self.revision.wrapping_add(1);
		self.epoch = self.revision;
		existed
	}
	fn observe_guild(&mut self, guild: Id) {
		if !self.guild_revisions.contains_key(&guild)
			&& self.guild_revisions.len() >= MAX_HINT_GUILDS
		{
			self.reset_catalogs();
		}
		self.revision = self.revision.wrapping_add(1);
		self.guild_revisions.insert(guild, self.revision);
	}
}

impl State {
	/// Retire cached previews without changing the authoritative live catalog.
	pub fn clear_expression_hints(&mut self) {
		let catalog = self.expression_hints.reset_catalogs();
		let packs = self.expression_hints.packs.take().is_some();
		if catalog || packs {
			self.navigation_index.bytes.set(None);
			if packs {
				self.expression_hints.pack_revision =
					self.expression_hints.pack_revision.wrapping_add(1);
			}
			self.revision = self.revision.wrapping_add(1);
		}
	}
	fn pack_scope(&self) -> bool {
		self.auth == AuthState::Authenticated && self.gateway_connected
	}
	pub fn sticker_pack_cache_revision(&self) -> u64 {
		self.expression_hints.pack_revision
	}
	pub fn cached_sticker_pack_hints(&self) -> Option<StickerPackHints> {
		if !self.pack_scope() || !self.stickers.loaded {
			return None;
		}
		let mut packs = self.stickers.packs.clone();
		for sticker in packs.iter_mut().flat_map(|pack| &mut pack.stickers) {
			sticker.available = false;
		}
		let hints = StickerPackHints { packs };
		hints.valid().then_some(hints)
	}
	pub fn hydrate_sticker_pack_hints(&mut self, started: u64, hints: StickerPackHints) -> bool {
		if !self.pack_scope() || started != self.sticker_pack_cache_revision() || !hints.valid() {
			return false;
		}
		if self.stickers.loaded {
			return true;
		}
		let prior = self.expression_hints.packs.take();
		self.navigation_index.bytes.set(None);
		if self
			.navigation_bytes()
			.saturating_add(self.permissions.bytes())
			.saturating_add(hints.bytes())
			> model::account::MAX_BYTES
		{
			self.expression_hints.packs = prior;
			self.navigation_index.bytes.set(None);
			return true;
		}
		self.expression_hints.packs = Some(hints);
		self.navigation_index.bytes.set(None);
		self.revision = self.revision.wrapping_add(1);
		true
	}
	pub fn display_sticker_packs(&self) -> &[StickerPack] {
		if self.stickers.loaded || !self.stickers.packs.is_empty() {
			return &self.stickers.packs;
		}
		if self.pack_scope() {
			return self
				.expression_hints
				.packs
				.as_ref()
				.map_or(&[], |h| h.packs.as_slice());
		}
		&[]
	}
	fn expression_scope(&self, guild: Id) -> bool {
		self.auth == AuthState::Authenticated
			&& self.gateway_connected
			&& self.guild(guild).is_some()
			&& self.selected.is_some_and(|channel| self.can_view(channel))
	}
	pub fn expression_cache_revision(&self) -> u64 {
		self.expression_hints.revision
	}
	/// Advances when the per-guild revision table is retired and hints must be reloaded.
	pub fn expression_cache_epoch(&self) -> u64 {
		self.expression_hints.epoch
	}
	pub fn expression_guild_cache_revision(&self, guild: Id) -> u64 {
		self.expression_hints
			.guild_revisions
			.get(&guild)
			.copied()
			.unwrap_or(self.expression_hints.epoch)
	}
	/// Only service-provided catalogs may be written back; hints cannot become authoritative.
	pub fn cached_expression_catalog(&self, guild: Id) -> Option<ExpressionCatalog> {
		if !self.expression_scope(guild) {
			return None;
		}
		let current = self.guild(guild)?;
		if current.emojis.is_none() && current.stickers.is_none() {
			return None;
		}
		let emojis = current.emojis.as_ref().map(|items| {
			items
				.iter()
				.cloned()
				.map(|mut item| {
					item.available = false;
					item.managed = false;
					item.roles = None;
					item
				})
				.collect()
		});
		let stickers = current.stickers.as_ref().map(|items| {
			items
				.iter()
				.cloned()
				.map(|mut item| {
					item.available = false;
					item
				})
				.collect()
		});
		Some(ExpressionCatalog {
			guild,
			emojis,
			stickers,
		})
	}
	/// Reject disk reads which outlive their account, view, permission or live-catalog revision.
	pub fn hydrate_expression_catalog(
		&mut self,
		guild: Id,
		started: u64,
		mut catalog: ExpressionCatalog,
	) -> bool {
		if catalog.guild != guild
			|| !catalog.valid()
			|| !self.expression_scope(guild)
			|| self.expression_guild_cache_revision(guild) != started
		{
			return false;
		}
		let current = self.guild(guild).expect("scope checked above");
		if current.emojis.is_some() {
			catalog.emojis = None;
		}
		if current.stickers.is_some() {
			catalog.stickers = None;
		}
		let prior = self.expression_hints.catalogs.remove(&guild);
		self.navigation_index.bytes.set(None);
		if catalog.emojis.is_some() || catalog.stickers.is_some() {
			if self.expression_hints.catalogs.len() >= MAX_HINT_GUILDS
				|| self
					.expression_hints
					.bytes()
					.saturating_add(catalog.bytes())
					> MAX_HINT_BYTES
				|| self
					.navigation_bytes()
					.saturating_add(self.permissions.bytes())
					.saturating_add(catalog.bytes())
					> model::account::MAX_BYTES
			{
				if let Some(prior) = prior {
					self.expression_hints.catalogs.insert(guild, prior);
				}
				self.navigation_index.bytes.set(None);
				// The current account budget cannot admit this snapshot; do not retry on repaint.
				return true;
			}
			self.expression_hints.catalogs.insert(guild, catalog);
		}
		self.navigation_index.bytes.set(None);
		self.revision = self.revision.wrapping_add(1);
		true
	}
	pub fn display_guild_emojis(&self, guild: Id) -> Option<&[CustomEmoji]> {
		self.expression_emojis_for(self.guild(guild)?)
	}
	pub fn expression_emojis_for<'a>(
		&'a self,
		current: &'a model::Guild,
	) -> Option<&'a [CustomEmoji]> {
		current.emojis.as_deref().or_else(|| {
			let hint = self.expression_hints.catalogs.get(&current.id)?;
			self.expression_scope(current.id)
				.then_some(hint.emojis.as_deref())
				.flatten()
		})
	}
	pub fn display_guild_stickers(&self, guild: Id) -> Option<&[Sticker]> {
		self.expression_stickers_for(self.guild(guild)?)
	}
	pub fn expression_stickers_for<'a>(
		&'a self,
		current: &'a model::Guild,
	) -> Option<&'a [Sticker]> {
		current.stickers.as_deref().or_else(|| {
			let hint = self.expression_hints.catalogs.get(&current.id)?;
			self.expression_scope(current.id)
				.then_some(hint.stickers.as_deref())
				.flatten()
		})
	}
	pub(crate) fn invalidate_expression_event(&mut self, event: &crate::Event) {
		if event.invalidates_expression_catalogs() {
			let catalogs = self.expression_hints.reset_catalogs();
			let packs = self.expression_hints.packs.take().is_some();
			if catalogs || packs {
				self.navigation_index.bytes.set(None);
			}
			self.expression_hints.pack_revision =
				self.expression_hints.pack_revision.wrapping_add(1);
			return;
		}
		if matches!(event, crate::Event::StickerPacks(Ok(_))) && self.stickers.loading {
			self.expression_hints.pack_revision =
				self.expression_hints.pack_revision.wrapping_add(1);
			if self.expression_hints.packs.take().is_some() {
				self.navigation_index.bytes.set(None);
			}
			return;
		}
		let (guild, emoji) = match event {
			crate::Event::GuildEmojis { guild, .. } => (*guild, true),
			crate::Event::GuildStickers { guild, .. } => (*guild, false),
			_ => return,
		};
		self.expression_hints.observe_guild(guild);
		self.navigation_index.bytes.set(None);
		if let Some(hint) = self.expression_hints.catalogs.get_mut(&guild) {
			if emoji {
				hint.emojis = None;
			} else {
				hint.stickers = None;
			}
			self.navigation_index.bytes.set(None);
		}
	}
}
