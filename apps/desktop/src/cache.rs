use eframe::egui;
use local_store::{Appearance, LocalStore, StoreError};
use model::{Id, Message};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{
		Arc, Condvar, Mutex,
		atomic::{AtomicBool, AtomicU64, Ordering},
		mpsc::{self, Receiver, SyncSender},
	},
};
// One full CJK font plus the existing 16 MiB of storage work and metadata.
const QUEUE_BYTES: usize = 48 * 1024 * 1024;
const WINDOW_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Budget {
	used: Mutex<usize>,
	available: Condvar,
}
pub struct Reservation {
	budget: Arc<Budget>,
	bytes: usize,
}
impl Budget {
	fn reserve(self: &Arc<Self>, bytes: usize, wait: bool) -> Option<Reservation> {
		if bytes > QUEUE_BYTES {
			return None;
		}
		let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
		while *used + bytes > QUEUE_BYTES {
			if !wait {
				return None;
			}
			used = self.available.wait(used).unwrap_or_else(|e| e.into_inner());
		}
		*used += bytes;
		Some(Reservation {
			budget: self.clone(),
			bytes,
		})
	}
}
impl Drop for Reservation {
	fn drop(&mut self) {
		*self.budget.used.lock().unwrap_or_else(|e| e.into_inner()) -= self.bytes;
		self.budget.available.notify_one();
	}
}
fn message_bytes(messages: &Vec<Message>) -> usize {
	messages.iter().map(Message::bytes).sum::<usize>()
		+ messages.capacity().saturating_sub(messages.len()) * size_of::<Message>()
}
#[allow(clippy::large_enum_variant)]
pub enum Operation {
	LoadCustomFont,
	SaveCustomFont(Option<ui::fonts::CustomFont>),
	SaveAppPreferences(Box<local_store::AppPreferences>),
	LoadAppearance,
	SaveAppearance(Appearance),
	SaveThemeVariant(Option<String>),
	LoadReadingPreferences,
	SaveReadingPreferences(model::ReadingPreferences),
	LoadGameActivity,
	SaveGameActivity(bool),
	LoadMinimizeToTray,
	SaveMinimizeToTray(bool),
	LoadDrafts,
	LoadGifFavorites,
	SaveGifFavorites(Vec<model::Gif>),
	LoadChannelPreferences,
	SaveChannelPreferences(model::ChannelPreferences),
	LoadAccountPresences,
	SaveAccountPresence(model::OwnPresence),
	LoadAccounts,
	SaveAccount(model::SavedAccount),
	SetAccountToken {
		account: Id,
		has_token: bool,
	},
	LoadChannel {
		channel: Id,
		request: u64,
	},
	LoadMembers {
		guild: Id,
		revision: u64,
	},
	LoadExpressions {
		guild: Id,
		revision: u64,
	},
	SaveExpressions(Box<model::ExpressionCatalog>),
	LoadStickerPackHints {
		revision: u64,
	},
	SaveStickerPackHints(Box<model::StickerPackHints>),
	ClearExpressions {
		guild: Id,
		/// None clears the guild; Some clears only the affected catalog.
		emoji: Option<bool>,
	},
	SaveMembers(Vec<model::MemberDisplay>),
	LoadServers {
		revision: u64,
		guilds: Vec<Id>,
	},
	SaveServers {
		rows: Vec<local_store::ServerDisplay>,
		revision: u64,
	},
	RemoveServer {
		guild: Id,
	},
	RemoveMember {
		guild: Id,
		user: Option<Id>,
	},
	ClearChannel {
		channel: Id,
	},
	SaveDraft {
		channel: Id,
		content: String,
	},
	/// `start`: the window reaches the channel's first message.
	SaveChannel {
		channel: Id,
		messages: Vec<Message>,
		start: bool,
	},
	SaveChanges {
		channel: Id,
		messages: Vec<Message>,
		retained: Vec<Id>,
		start: bool,
	},
	DeleteMessages {
		channel: Id,
		ids: Vec<Id>,
	},
	ClearHistory,
	Forget,
}
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
	CustomFont(Result<Option<ui::fonts::CustomFont>, &'static str>),
	AppPreferencesSaved(Result<(), StoreError>),
	/// Saved appearance plus the saved theme preset key, if any.
	Appearance(Appearance, Option<String>),
	ReadingPreferences(Result<model::ReadingPreferences, StoreError>),
	ReadingPreferencesSaved(Result<(), StoreError>),
	GameActivity(Result<bool, StoreError>),
	GameActivitySaved(Result<(), StoreError>),
	MinimizeToTray(Result<bool, StoreError>),
	MinimizeToTraySaved(Result<(), StoreError>),
	Drafts(BTreeMap<Id, String>),
	GifFavorites(Vec<model::Gif>),
	ChannelPreferences(Result<model::ChannelPreferences, StoreError>),
	ChannelPreferencesSaved(Result<(), StoreError>),
	AccountPresences(Result<std::collections::BTreeMap<model::Id, model::OwnPresence>, StoreError>),
	AccountPresenceSaved(Result<(), StoreError>),
	/// The whole switcher roster, plus any accounts pruned to keep it bounded. Pruning is
	/// already committed when this is produced, so the IDs travel outside the roster result:
	/// a failed re-read must not strand their saved secrets and cached data.
	Accounts {
		roster: Result<Vec<model::SavedAccount>, StoreError>,
		pruned: Vec<Id>,
	},
	Channel {
		channel: Id,
		request: u64,
		messages: Vec<Message>,
		/// The cached window reaches the channel's first message.
		start: bool,
		epoch: u64,
	},
	Saved,
	HistoryCleared,
	Members {
		guild: Id,
		revision: u64,
		epoch: u64,
		result: Result<Vec<model::MemberDisplay>, StoreError>,
	},
	Expressions {
		account: Id,
		guild: Id,
		revision: u64,
		epoch: u64,
		result: Result<Option<model::ExpressionCatalog>, StoreError>,
	},
	StickerPackHints {
		account: Id,
		revision: u64,
		epoch: u64,
		result: Result<Option<model::StickerPackHints>, StoreError>,
	},
	Servers {
		revision: u64,
		epoch: u64,
		result: Result<Vec<local_store::ServerDisplay>, StoreError>,
	},
	ServersSaved {
		revision: u64,
		epoch: u64,
		ids: Vec<Id>,
		result: Option<Result<(), StoreError>>,
	},
	ScopedCleanupFailed {
		account: Id,
	},
	Failed {
		error: StoreError,
		message: &'static str,
		draft_restore: bool,
		history_cleanup: bool,
	},
}
pub struct Cache {
	send: SyncSender<(u64, Id, u64, Operation, Reservation)>,
	pub receive: Receiver<(u64, Outcome, Reservation)>,
	budget: Arc<Budget>,
	pub history: Arc<HistorySafety>,
}
/// Saves in these lanes remain pending after queue backpressure and retry on progress.
pub fn coalesced_retry(operation: &Operation) -> bool {
	matches!(
		operation,
		Operation::SaveGifFavorites(_)
			| Operation::SaveServers { .. }
			| Operation::SaveExpressions(_)
			| Operation::SaveStickerPackHints(_)
	)
}
/// Coalesced work for up to the number of guild catalog rows the store can retain.
#[derive(Default)]
pub struct ExpressionCacheSchedule {
	scope: Option<(u64, Id, u64)>,
	core_epoch: Option<u64>,
	selected: Option<Id>,
	pending: std::collections::VecDeque<(Id, u8)>,
	waiting: bool,
}
impl ExpressionCacheSchedule {
	const LIMIT: usize = 128;
	pub const LOAD: u8 = 1;
	pub const SAVE: u8 = 2;
	pub fn scope(&mut self, scope: (u64, Id, u64), guilds: impl Iterator<Item = Id>) {
		if self.scope == Some(scope) {
			return;
		}
		*self = Self::default();
		self.scope = Some(scope);
		for guild in guilds.take(Self::LIMIT) {
			self.enqueue(guild, Self::LOAD | Self::SAVE);
		}
	}
	/// Reload only once when core invalidates all guild hint revisions, including
	/// rare revision-map overflow. Unchanged authoritative catalogs need no writes.
	pub fn observe_epoch(&mut self, epoch: u64, guilds: impl Iterator<Item = Id>) {
		let previous = self.core_epoch.replace(epoch);
		if previous.is_some_and(|previous| previous != epoch) {
			self.refresh(guilds);
		}
	}
	pub fn refresh(&mut self, guilds: impl Iterator<Item = Id>) {
		for guild in guilds.take(Self::LIMIT) {
			self.enqueue(guild, Self::LOAD);
		}
		if let Some(guild) = self.selected {
			self.prioritize(guild, Self::LOAD);
		}
	}
	pub fn enqueue(&mut self, guild: Id, operations: u8) {
		if guild.0 == 0 || operations == 0 {
			return;
		}
		if let Some((_, flags)) = self.pending.iter_mut().find(|(id, _)| *id == guild) {
			*flags |= operations;
			return;
		}
		if self.pending.len() == Self::LIMIT {
			let victim = self
				.pending
				.iter()
				.position(|(id, _)| Some(*id) != self.selected)
				.unwrap_or(0);
			self.pending.remove(victim);
		}
		self.pending.push_back((guild, operations));
	}
	fn prioritize(&mut self, guild: Id, operations: u8) {
		self.enqueue(guild, operations);
		if let Some(index) = self.pending.iter().position(|(id, _)| *id == guild)
			&& let Some(item) = self.pending.remove(index)
		{
			self.pending.push_front(item);
		}
	}
	pub fn selected(&mut self, guild: Option<Id>) {
		if self.selected != guild {
			self.selected = guild;
			if let Some(guild) = guild {
				self.prioritize(guild, Self::LOAD | Self::SAVE);
			}
		}
	}
	pub fn next(&self) -> Option<(Id, u8)> {
		if self.waiting {
			return None;
		}
		self.pending.front().map(|&(guild, flags)| {
			(
				guild,
				if flags & Self::LOAD != 0 {
					Self::LOAD
				} else {
					Self::SAVE
				},
			)
		})
	}
	pub fn completed(&mut self, guild: Id, operation: u8) {
		if let Some((id, flags)) = self.pending.front_mut()
			&& *id == guild
		{
			*flags &= !operation;
			if *flags == 0 {
				self.pending.pop_front();
			}
		}
	}
	pub fn blocked(&mut self) {
		self.waiting = true;
	}
	pub fn capacity_changed(&mut self) {
		self.waiting = false;
	}
	pub fn remove(&mut self, guild: Id) {
		self.pending.retain(|(id, _)| *id != guild);
	}
}
/// Retry a rejected read only after storage has made progress or its scope changes.
#[derive(Default)]
pub struct HistoryLoad {
	attempted: Option<(u64, Id, u64, u64)>,
	accepted: Option<(u64, Id, u64, u64)>,
}
impl HistoryLoad {
	pub fn attempt(&mut self, key: (u64, Id, u64, u64)) -> bool {
		if self.accepted == Some(key) || self.attempted == Some(key) {
			return false;
		}
		self.attempted = Some(key);
		true
	}
	pub fn accepted(&mut self, key: (u64, Id, u64, u64)) {
		self.accepted = Some(key);
	}
	pub fn was_accepted(&self, key: (u64, Id, u64, u64)) -> bool {
		self.accepted == Some(key)
	}
	pub fn capacity_changed(&mut self) {
		self.attempted = None;
	}
	/// A completed read discarded by its consumer may retry after that worker progress.
	pub fn retry(&mut self, key: (u64, Id, u64, u64)) {
		if self.accepted == Some(key) {
			*self = Self::default();
		}
	}
	/// Called only for a nonempty completed read whose account/epoch fence still holds.
	/// Navigation can make a result ineligible without changing its observation revision.
	pub fn member_result(
		&mut self,
		key: (u64, Id, u64, u64),
		revision: u64,
		state: &client_core::State,
	) {
		if key.0 == state.generation
			&& (revision != state.member_display_revision()
				|| !state.gateway_connected
				|| state.auth != client_core::auth::AuthState::Authenticated
				|| !state.selected.is_some_and(|channel| {
					state.can_view(channel)
						&& state
							.channel(channel)
							.is_some_and(|c| c.guild == Some(key.1))
				})) {
			self.retry(key);
		}
	}
	/// RAM identities are retired on these accepted session transitions even when the
	/// account generation and disk-safety epoch survive a successful RESUME.
	pub fn observe_session(&mut self, generation: u64, event: &client_core::Envelope) -> bool {
		if event.generation == generation
			&& (event.event.ready_navigation().is_some()
				|| matches!(
					event.event,
					client_core::Event::Disconnected
						| client_core::Event::Resync
						| client_core::Event::PermissionsChanged
				)) {
			*self = Self::default();
			true
		} else {
			false
		}
	}
}

/// A scoped cleanup can retire a previously accepted save without changing the
/// display revision, so the safety epoch is part of the save admission key.
#[derive(Default)]
pub struct MemberCacheClear {
	generation: u64,
	blocked: std::collections::BTreeSet<Id>,
}
impl MemberCacheClear {
	pub fn reset(&mut self, state: &client_core::State) {
		self.generation = state.generation;
		self.blocked = state.cached_member_guilds().into_iter().collect();
	}
	pub fn observe(&mut self, state: &client_core::State, guild: Option<Id>, before: u64) {
		if self.generation == state.generation
			&& before != state.member_display_revision()
			&& let Some(guild) = guild
		{
			self.blocked.remove(&guild);
		}
	}
}

pub fn member_save_key(
	state: &client_core::State,
	guild: Id,
	epoch: u64,
	cleared: &MemberCacheClear,
) -> Option<(u64, Id, u64, u64)> {
	let revision = state.member_display_cache_revision();
	(cleared.generation != state.generation || !cleared.blocked.contains(&guild)).then_some((
		state.generation,
		guild,
		revision,
		epoch,
	))
}

pub fn member_observation_guild(event: &client_core::Event) -> Option<Id> {
	match event {
		client_core::Event::MemberIdentity(update) => Some(update.guild),
		client_core::Event::Typing(signal) => signal.member.as_ref().map(|member| member.guild),
		client_core::Event::MemberSearch { request, .. } => Some(request.guild),
		client_core::Event::Members(list) => list.guild,
		_ => None,
	}
}

/// Preserve only a bounded target while the reducer validates the event.
pub fn nickname_target(event: &client_core::Event) -> Option<(Id, Id)> {
	let update = match event {
		client_core::Event::MemberIdentity(update) => Some(update.as_ref()),
		client_core::Event::Typing(signal) => signal.member.as_deref(),
		_ => None,
	}?;
	(!update.departed && update.valid() && !matches!(update.nick, model::Patch::Absent))
		.then_some((update.guild, update.user))
}

pub fn departed_member_target(
	state: &client_core::State,
	envelope: &client_core::Envelope,
) -> Option<(Id, Id)> {
	match &envelope.event {
		client_core::Event::MemberIdentity(update)
			if envelope.generation == state.generation && update.departed && update.valid() =>
		{
			Some((update.guild, update.user))
		}
		_ => None,
	}
}

/// A partial accepted nickname update cannot leave a stale complete row on disk.
pub fn incomplete_nickname_cleanup(
	state: &client_core::State,
	target: Option<(Id, Id)>,
	before: u64,
) -> Option<(Id, Id)> {
	let (guild, user) = target?;
	(before != state.member_display_cache_revision()
		&& !state.has_cached_member_display(guild, user))
	.then_some((guild, user))
}

/// Admission state only: GIF contents remain in the bounded account-scoped state.
#[derive(Default)]
pub struct GifCache {
	pub load: HistoryLoad,
	blocked_save: Option<(u64, Id)>,
	loaded: Option<(u64, Id)>,
}
impl GifCache {
	pub fn loaded(&mut self, scope: (u64, Id)) {
		if self.load.was_accepted((scope.0, scope.1, 0, 0)) {
			self.loaded = Some(scope);
		}
	}
	pub fn may_save(&self, scope: (u64, Id)) -> bool {
		self.loaded == Some(scope) && self.blocked_save != Some(scope)
	}
	pub fn reject_save(&mut self, scope: (u64, Id)) {
		self.blocked_save = Some(scope);
	}
	pub fn capacity_changed(&mut self) {
		self.load.capacity_changed();
		self.blocked_save = None;
	}
}
#[derive(Default)]
pub struct HistorySafety {
	epoch: AtomicU64,
	blocked: AtomicBool,
	failed: AtomicBool,
}
impl HistorySafety {
	pub fn invalidate(&self) {
		self.epoch.fetch_add(1, Ordering::SeqCst);
	}
	pub fn block(&self) {
		self.blocked.store(true, Ordering::SeqCst);
	}
	pub fn fail(&self) {
		self.failed.store(true, Ordering::SeqCst);
		self.block();
	}
	pub fn cleared(&self) {
		if !self.failed.load(Ordering::SeqCst) {
			self.blocked.store(false, Ordering::SeqCst);
		}
	}
	pub fn epoch(&self) -> u64 {
		self.epoch.load(Ordering::SeqCst)
	}
	pub fn allows(&self, epoch: u64) -> bool {
		!self.failed.load(Ordering::SeqCst)
			&& !self.blocked.load(Ordering::SeqCst)
			&& self.epoch() == epoch
	}
}
/// At most sixteen distinct account cleanups await queue space; drafts are unaffected.
#[derive(Default)]
pub struct HistoryClears {
	accounts: BTreeSet<Id>,
	queued: usize,
}
impl HistoryClears {
	pub fn pending(&self) -> bool {
		!self.accounts.is_empty() || self.queued != 0
	}
	pub fn request(&mut self, account: Id) -> bool {
		if self.accounts.len() == 16 && !self.accounts.contains(&account) {
			return false;
		}
		self.accounts.insert(account);
		true
	}
	pub fn next(&self) -> Option<Id> {
		self.accounts.first().copied()
	}
	pub fn queued(&mut self, account: Id) {
		if self.accounts.remove(&account) {
			self.queued += 1;
		}
	}
	pub fn acknowledge(&mut self, safety: &HistorySafety) -> bool {
		self.queued = self.queued.saturating_sub(1);
		let complete = self.accounts.is_empty() && self.queued == 0;
		if complete {
			safety.cleared();
		}
		complete
	}
}

/// Successful credential removals waiting for storage queue space, bounded by the roster.
#[derive(Default)]
pub struct AccountForgets {
	accounts: BTreeMap<Id, u64>,
}
impl AccountForgets {
	pub fn pending(&self) -> bool {
		!self.accounts.is_empty()
	}
	pub fn contains(&self, account: Id) -> bool {
		self.accounts.contains_key(&account)
	}
	pub fn request(&mut self, generation: u64, account: Id) -> bool {
		if self.accounts.len() == model::MAX_SAVED_ACCOUNTS && !self.accounts.contains_key(&account)
		{
			return false;
		}
		self.accounts.insert(account, generation);
		true
	}
	pub fn next(&mut self, generation: u64, active_account: Option<Id>) -> Option<Id> {
		// An explicit sign-in to the same account supersedes its old deletion. Other
		// accounts' device-scoped cleanup survives ordinary session switches.
		self.accounts.retain(|account, requested| {
			crate::credentials::account_removal_applies(
				*requested,
				generation,
				*account,
				active_account,
			)
		});
		self.accounts.first_key_value().map(|(account, _)| *account)
	}
	pub fn queued(&mut self, account: Id) {
		self.accounts.remove(&account);
	}
}
impl Cache {
	/// FIFO cleanup plus the epoch fence prevents earlier reads/writes restoring removed data.
	pub fn invalidate_scope(&self, generation: u64, account: Id, operation: Operation) -> bool {
		let blocked = !self.history.allows(self.history.epoch());
		self.history.invalidate();
		if blocked || !self.queue(generation, account, operation) {
			self.history.block();
			false
		} else {
			true
		}
	}
	pub fn delete_messages(&self, generation: u64, account: Id, channel: Id, ids: Vec<Id>) -> bool {
		let blocked = !self.history.allows(self.history.epoch());
		self.history.invalidate();
		if blocked
			|| ids.len() > 100
			|| !self.queue(
				generation,
				account,
				Operation::DeleteMessages { channel, ids },
			) {
			self.history.block();
			false
		} else {
			true
		}
	}
	pub fn queue(&self, generation: u64, account: Id, operation: Operation) -> bool {
		if matches!(&operation, Operation::SaveChannelPreferences(value) if !value.is_valid()) {
			return false;
		}
		let payload = match &operation {
			// The bounded JSON columns and decoded catalog can coexist during a read.
			Operation::LoadExpressions { .. } | Operation::LoadStickerPackHints { .. } => {
				8 * 1024 * 1024
			}
			Operation::SaveExpressions(catalog) => {
				if !catalog.valid() {
					return false;
				}
				catalog.bytes() + 4 * 1024 * 1024
			}
			Operation::SaveStickerPackHints(packs) => {
				if !packs.valid() {
					return false;
				}
				packs.bytes() + 2 * 1024 * 1024
			}
			Operation::SaveServers { rows, .. } => {
				if rows.len() > 4096
					|| rows.iter().any(|row| !row.valid())
					|| rows
						.iter()
						.map(local_store::ServerDisplay::bytes)
						.sum::<usize>() > 2 * 1024 * 1024
				{
					return false;
				}
				rows.iter()
					.map(local_store::ServerDisplay::bytes)
					.sum::<usize>() + rows.capacity() * size_of::<local_store::ServerDisplay>()
			}
			Operation::SaveMembers(rows) => {
				if rows.len() > 4096
					|| rows.iter().any(|row| !row.valid())
					|| rows.iter().map(model::MemberDisplay::bytes).sum::<usize>() > 2 * 1024 * 1024
				{
					return false;
				}
				rows.iter().map(model::MemberDisplay::bytes).sum::<usize>()
					+ rows.capacity().saturating_sub(rows.len()) * size_of::<model::MemberDisplay>()
			}
			Operation::SaveCustomFont(font) => font
				.as_ref()
				.map_or(0, |font| font.bytes().len() + font.name.capacity()),
			Operation::SaveChannel { messages, .. } | Operation::SaveChanges { messages, .. } => {
				if messages.len() > 500
					|| messages.iter().map(Message::bytes).sum::<usize>() > WINDOW_BYTES
				{
					return false;
				}
				message_bytes(messages)
			}
			Operation::SaveDraft { content, .. } => {
				if content.len() > model::message_options::MAX_DRAFT_CONTENT_BYTES {
					return false;
				}
				content.capacity()
			}
			Operation::SaveGifFavorites(favorites) => {
				if favorites.len() > 100 {
					return false;
				}
				favorites.iter().map(model::Gif::bytes).sum::<usize>()
					+ favorites.capacity() * size_of::<model::Gif>()
			}
			Operation::SaveAppPreferences(value) => {
				if !value.is_valid() {
					return false;
				}
				value.voice_input.as_ref().map_or(0, String::capacity)
					+ value.voice_output.as_ref().map_or(0, String::capacity)
					+ value.media_output.as_ref().map_or(0, String::capacity)
					+ value.expanded_folders.capacity() * size_of::<u64>()
					+ model::KeybindAction::ALL
						.into_iter()
						.map(|action| value.keybinds.chord(action).key.capacity())
						.sum::<usize>()
			}
			Operation::SaveThemeVariant(value) => value.as_ref().map_or(0, String::capacity),
			Operation::SaveAccount(account) => {
				if !account.is_valid() {
					return false;
				}
				account.heap_bytes()
			}
			_ => 0,
		};
		let ids = match &operation {
			Operation::LoadServers { guilds, .. } => {
				if guilds.len() > 4096 {
					return false;
				}
				guilds.capacity() * size_of::<Id>()
			}
			Operation::SaveChanges { retained, .. } => {
				if retained.len() > 500 {
					return false;
				}
				retained.capacity() * size_of::<Id>()
			}
			Operation::DeleteMessages { ids, .. } => {
				if ids.len() > 100 {
					return false;
				}
				ids.capacity() * size_of::<Id>()
			}
			_ => 0,
		};
		// Small metadata and channel preferences fit in the reserved 8 KiB overhead.
		let Some(reservation) = self
			.budget
			.reserve(payload.saturating_add(ids).saturating_add(8192), false)
		else {
			return false;
		};
		let epoch = self.history.epoch();
		if matches!(
			operation,
			Operation::LoadChannel { .. }
				| Operation::SaveChannel { .. }
				| Operation::SaveChanges { .. }
				| Operation::LoadMembers { .. }
				| Operation::SaveMembers(_)
				| Operation::LoadExpressions { .. }
				| Operation::SaveExpressions(_)
				| Operation::LoadStickerPackHints { .. }
				| Operation::SaveStickerPackHints(_)
				| Operation::LoadServers { .. }
				| Operation::SaveServers { .. }
		) && !self.history.allows(epoch)
		{
			return false;
		}
		self.send
			.try_send((generation, account, epoch, operation, reservation))
			.is_ok()
	}
	pub fn start(ctx: egui::Context) -> Self {
		let (send, commands) = mpsc::sync_channel::<(u64, Id, u64, Operation, Reservation)>(16);
		let (events, receive) = mpsc::sync_channel(16);
		let budget = Arc::new(Budget::default());
		let history = Arc::new(HistorySafety::default());
		let worker_history = history.clone();
		std::thread::spawn(move || {
			let mut store = LocalStore::open_default();
			let results = Arc::new(Budget::default());
			while let Ok((generation, account, epoch, operation, reservation)) = commands.recv() {
				let outcome = execute(&mut store, &worker_history, account, epoch, operation);
				drop(reservation);
				let bytes = outcome_bytes(&outcome);
				let reservation = results
					.reserve(bytes + 64 * 1024, true)
					.expect("bounded storage outcome fits queue");
				if events.send((generation, outcome, reservation)).is_err() {
					break;
				}
				ctx.request_repaint();
			}
		});
		Self {
			send,
			receive,
			budget,
			history,
		}
	}
}

/// Heap retained by a completed result while the UI has not consumed it.
fn outcome_bytes(outcome: &Outcome) -> usize {
	match outcome {
		Outcome::CustomFont(Ok(Some(font))) => font.bytes().len() + font.name.capacity(),
		Outcome::Channel { messages, .. } => message_bytes(messages),
		Outcome::Members { result, .. } => result.as_ref().map_or(0, |rows| {
			rows.iter().map(model::MemberDisplay::bytes).sum::<usize>()
				+ rows.capacity().saturating_sub(rows.len()) * size_of::<model::MemberDisplay>()
		}),
		Outcome::Expressions { result, .. } => result
			.as_ref()
			.ok()
			.and_then(Option::as_ref)
			.map_or(0, model::ExpressionCatalog::bytes),
		Outcome::StickerPackHints { result, .. } => result
			.as_ref()
			.ok()
			.and_then(Option::as_ref)
			.map_or(0, model::StickerPackHints::bytes),
		Outcome::Servers { result, .. } => result.as_ref().map_or(0, |rows| {
			rows.iter()
				.map(local_store::ServerDisplay::bytes)
				.sum::<usize>()
				+ rows.capacity() * size_of::<local_store::ServerDisplay>()
		}),
		Outcome::ServersSaved { ids, .. } => ids.capacity() * size_of::<Id>(),
		Outcome::Drafts(drafts) => drafts.values().map(String::capacity).sum(),
		Outcome::GifFavorites(favorites) => {
			favorites.iter().map(model::Gif::bytes).sum::<usize>()
				+ favorites.capacity() * size_of::<model::Gif>()
		}
		Outcome::Accounts { roster, pruned } => {
			roster.as_ref().map_or(0, |accounts| {
				accounts
					.iter()
					.map(model::SavedAccount::heap_bytes)
					.sum::<usize>() + accounts.capacity() * size_of::<model::SavedAccount>()
			}) + pruned.capacity() * size_of::<Id>()
		}
		_ => 0,
	}
}

fn execute(
	store: &mut Result<LocalStore, StoreError>,
	history: &HistorySafety,
	account: Id,
	epoch: u64,
	operation: Operation,
) -> Outcome {
	// Settings completions have their own pending/error state, independent of history.
	match &operation {
		Operation::LoadCustomFont => {
			return Outcome::CustomFont((|| {
				let store = store
					.as_ref()
					.map_err(|_| "Could not load the saved font.")?;
				store
					.custom_font()
					.map_err(|_| "Could not load the saved font.")?
					.map(|(name, bytes)| ui::fonts::CustomFont::new(name, bytes))
					.transpose()
			})());
		}
		Operation::SaveCustomFont(font) => {
			return Outcome::CustomFont((|| {
				let store = store
					.as_ref()
					.map_err(|_| "Could not save the font. Try importing it again.")?;
				store
					.save_custom_font(font.as_ref().map(|font| (font.name.as_str(), font.bytes())))
					.map_err(|_| "Could not save the font. Try again.")?;
				Ok(font.clone())
			})());
		}
		Operation::LoadChannelPreferences => {
			return Outcome::ChannelPreferences(match store {
				Ok(store) => store.channel_preferences(account),
				Err(error) => Err(*error),
			});
		}
		Operation::SaveChannelPreferences(value) => {
			return Outcome::ChannelPreferencesSaved(match store {
				Ok(store) => store.save_channel_preferences(account, value),
				Err(error) => Err(*error),
			});
		}
		Operation::LoadAccountPresences => {
			return Outcome::AccountPresences(match store {
				Ok(store) => store.account_presences(),
				Err(error) => Err(*error),
			});
		}
		Operation::SaveAccountPresence(presence) => {
			return Outcome::AccountPresenceSaved(match store {
				Ok(store) => store.save_account_presence(account, presence),
				Err(error) => Err(*error),
			});
		}
		Operation::LoadAccounts => {
			return Outcome::Accounts {
				roster: match store {
					Ok(store) => store.accounts(),
					Err(error) => Err(*error),
				},
				pruned: Vec::new(),
			};
		}
		Operation::SaveAccount(account) => {
			let (roster, pruned) = match store {
				Ok(store) => match store.save_account(account) {
					// Report the committed pruning even when the re-read fails.
					Ok(pruned) => (store.accounts(), pruned),
					Err(error) => (Err(error), Vec::new()),
				},
				Err(error) => (Err(*error), Vec::new()),
			};
			return Outcome::Accounts { roster, pruned };
		}
		Operation::SetAccountToken {
			account: id,
			has_token,
		} => {
			return Outcome::Accounts {
				roster: match store {
					Ok(store) => store
						.set_account_token(*id, *has_token)
						.and(store.accounts()),
					Err(error) => Err(*error),
				},
				pruned: Vec::new(),
			};
		}
		Operation::SaveAppPreferences(value) => {
			return Outcome::AppPreferencesSaved(match store {
				Ok(store) => store.save_app_preferences(value),
				Err(error) => Err(*error),
			});
		}
		Operation::LoadMinimizeToTray => {
			return Outcome::MinimizeToTray(match store {
				Ok(store) => store.minimize_to_tray(),
				Err(error) => Err(*error),
			});
		}
		Operation::SaveMinimizeToTray(value) => {
			return Outcome::MinimizeToTraySaved(match store {
				Ok(store) => store.save_minimize_to_tray(*value),
				Err(error) => Err(*error),
			});
		}
		Operation::LoadGameActivity => {
			return Outcome::GameActivity(match store {
				Ok(store) => store.game_activity_enabled(),
				Err(error) => Err(*error),
			});
		}
		Operation::SaveGameActivity(value) => {
			return Outcome::GameActivitySaved(match store {
				Ok(store) => store.save_game_activity_enabled(*value),
				Err(error) => Err(*error),
			});
		}
		Operation::LoadReadingPreferences => {
			return Outcome::ReadingPreferences(match store {
				Ok(store) => store.reading_preferences(),
				Err(error) => Err(*error),
			});
		}
		Operation::SaveReadingPreferences(value) => {
			return Outcome::ReadingPreferencesSaved(match store {
				Ok(store) => store.save_reading_preferences(*value),
				Err(error) => Err(*error),
			});
		}
		_ => {}
	}
	if let Operation::LoadServers { revision, guilds } = operation {
		return Outcome::Servers {
			revision,
			epoch,
			result: if history.allows(epoch) {
				match store {
					Ok(store) => store.load_server_displays_for(account, &guilds),
					Err(error) => Err(*error),
				}
			} else {
				Ok(Vec::new())
			},
		};
	}
	if let Operation::SaveServers { rows, revision } = operation {
		let ids = rows.iter().map(|row| row.guild).collect();
		return Outcome::ServersSaved {
			revision,
			epoch,
			ids,
			result: if history.allows(epoch) {
				Some(match store {
					Ok(store) => store.save_server_displays(account, &rows),
					Err(error) => Err(*error),
				})
			} else {
				None
			},
		};
	}
	if let Operation::LoadMembers { guild, revision } = operation {
		return Outcome::Members {
			guild,
			revision,
			epoch,
			result: if history.allows(epoch) {
				match store {
					Ok(store) => store.load_member_displays(account, guild),
					Err(error) => Err(*error),
				}
			} else {
				Ok(Vec::new())
			},
		};
	}
	if let Operation::LoadExpressions { guild, revision } = operation {
		return Outcome::Expressions {
			account,
			guild,
			revision,
			epoch,
			result: if history.allows(epoch) {
				match store {
					Ok(store) => store.load_expression_catalog(account, guild),
					Err(error) => Err(*error),
				}
			} else {
				Ok(None)
			},
		};
	}
	if let Operation::LoadStickerPackHints { revision } = operation {
		return Outcome::StickerPackHints {
			account,
			revision,
			epoch,
			result: if history.allows(epoch) {
				match store {
					Ok(store) => store.load_sticker_pack_hints(account),
					Err(error) => Err(*error),
				}
			} else {
				Ok(None)
			},
		};
	}
	if matches!(
		operation,
		Operation::LoadChannel { .. }
			| Operation::SaveChannel { .. }
			| Operation::SaveChanges { .. }
			| Operation::SaveMembers(_)
			| Operation::SaveExpressions(_)
			| Operation::SaveStickerPackHints(_)
	) && !history.allows(epoch)
	{
		return Outcome::Saved;
	}
	let draft_restore = matches!(operation, Operation::LoadDrafts);
	let history_cleanup = matches!(operation, Operation::ClearHistory);
	let scoped_cleanup = matches!(
		operation,
		Operation::ClearChannel { .. }
			| Operation::RemoveMember { .. }
			| Operation::ClearExpressions { .. }
			| Operation::RemoveServer { .. }
	);
	let history_unsafe = matches!(
		operation,
		Operation::DeleteMessages { .. }
			| Operation::ClearHistory
			| Operation::Forget
			| Operation::ClearChannel { .. }
			| Operation::RemoveMember { .. }
			| Operation::ClearExpressions { .. }
			| Operation::RemoveServer { .. }
	);
	let failure_message = match &operation {
		Operation::LoadAppearance => "Could not load saved appearance; using system theme",
		Operation::SaveAppearance(_) | Operation::SaveThemeVariant(_) => {
			"Could not save appearance; change exists only in this session"
		}
		Operation::Forget => {
			"Could not remove local account data; history and drafts may remain on disk"
		}
		Operation::ClearHistory => {
			"Could not clear cached history; history cache disabled until restart; messages may remain on disk"
		}
		Operation::ClearChannel { .. }
		| Operation::RemoveMember { .. }
		| Operation::ClearExpressions { .. } => {
			"Could not remove inaccessible cached data; history cache disabled until restart"
		}
		Operation::SaveMembers(_) => {
			"Could not save member names; current names remain available in this session"
		}
		Operation::SaveExpressions(_) => {
			"Could not save cached emoji and stickers; live catalogs remain available"
		}
		Operation::SaveStickerPackHints(_) => {
			"Could not save cached sticker packs; live packs remain available"
		}
		Operation::RemoveServer { .. } => "Could not remove cached server data",
		Operation::DeleteMessages { .. } => {
			"Could not remove deleted cached messages; history cache disabled until restart; messages may remain on disk"
		}
		Operation::SaveDraft { .. } => {
			"Could not save a draft; latest text may exist only in memory"
		}
		Operation::SaveChannel { .. } | Operation::SaveChanges { .. } => {
			"Could not save cached history"
		}
		Operation::LoadDrafts => "Could not restore drafts from local storage",
		Operation::LoadGifFavorites => "Could not restore GIF favorites from local storage",
		Operation::SaveGifFavorites(_) => {
			"Could not save GIF favorites; the change exists only in this session"
		}
		Operation::LoadChannel { .. } => "Could not read cached history",
		Operation::LoadCustomFont
		| Operation::LoadMembers { .. }
		| Operation::LoadExpressions { .. }
		| Operation::LoadStickerPackHints { .. }
		| Operation::LoadServers { .. }
		| Operation::SaveServers { .. }
		| Operation::SaveCustomFont(_)
		| Operation::LoadAccounts
		| Operation::SaveAccount(_)
		| Operation::SetAccountToken { .. }
		| Operation::LoadChannelPreferences
		| Operation::SaveChannelPreferences(_)
		| Operation::SaveAppPreferences(_)
		| Operation::LoadReadingPreferences
		| Operation::SaveReadingPreferences(_)
		| Operation::LoadGameActivity
		| Operation::SaveGameActivity(_)
		| Operation::LoadMinimizeToTray
		| Operation::SaveMinimizeToTray(_)
		| Operation::LoadAccountPresences
		| Operation::SaveAccountPresence(_) => unreachable!(),
	};
	let result = match store {
		Ok(store) => match operation {
			Operation::LoadCustomFont
			| Operation::LoadMembers { .. }
			| Operation::LoadExpressions { .. }
			| Operation::LoadStickerPackHints { .. }
			| Operation::LoadServers { .. }
			| Operation::SaveServers { .. }
			| Operation::SaveCustomFont(_)
			| Operation::LoadAccounts
			| Operation::SaveAccount(_)
			| Operation::SetAccountToken { .. }
			| Operation::LoadChannelPreferences
			| Operation::SaveChannelPreferences(_)
			| Operation::SaveAppPreferences(_)
			| Operation::LoadReadingPreferences
			| Operation::SaveReadingPreferences(_)
			| Operation::LoadGameActivity
			| Operation::SaveGameActivity(_)
			| Operation::LoadMinimizeToTray
			| Operation::SaveMinimizeToTray(_)
			| Operation::LoadAccountPresences
			| Operation::SaveAccountPresence(_) => {
				unreachable!()
			}
			Operation::LoadAppearance => store
				.appearance()
				.and_then(|appearance| Ok(Outcome::Appearance(appearance, store.theme_variant()?))),
			Operation::SaveAppearance(appearance) => {
				store.save_appearance(appearance).map(|_| Outcome::Saved)
			}
			Operation::SaveThemeVariant(variant) => store
				.save_theme_variant(variant.as_deref())
				.map(|_| Outcome::Saved),
			Operation::LoadDrafts => store.load_drafts(account).map(Outcome::Drafts),
			Operation::LoadGifFavorites => store.gif_favorites(account).map(Outcome::GifFavorites),
			Operation::SaveGifFavorites(favorites) => store
				.save_gif_favorites(account, &favorites)
				.map(|_| Outcome::Saved),
			Operation::LoadChannel { channel, request } => {
				store.load_channel(account, channel).and_then(|messages| {
					Ok(Outcome::Channel {
						channel,
						request,
						start: store.history_start(account, channel, &messages)?,
						messages,
						epoch,
					})
				})
			}
			Operation::SaveMembers(rows) => store
				.save_member_displays(account, &rows)
				.map(|_| Outcome::Saved),
			Operation::SaveExpressions(catalog) => store
				.save_expression_catalog(account, &catalog)
				.map(|_| Outcome::Saved),
			Operation::SaveStickerPackHints(packs) => store
				.save_sticker_pack_hints(account, &packs)
				.map(|_| Outcome::Saved),
			Operation::ClearExpressions { guild, emoji } => match emoji {
				Some(emoji) => store.clear_expression_field(account, guild, emoji),
				None => store.clear_expression_guild(account, guild),
			}
			.map(|_| Outcome::Saved),
			Operation::RemoveServer { guild } => store
				.clear_server_display(account, guild)
				.map(|_| Outcome::Saved),
			Operation::RemoveMember { guild, user } => match user {
				Some(user) => store.remove_member_display(account, guild, user),
				None => store
					.clear_member_guild(account, guild)
					.and_then(|()| store.clear_expression_guild(account, guild)),
			}
			.map(|_| Outcome::Saved),
			Operation::ClearChannel { channel } => store
				.clear_channel_history(account, channel)
				.map(|_| Outcome::Saved),
			Operation::SaveDraft { channel, content } => store
				.save_draft(account, channel, &content)
				.map(|_| Outcome::Saved),
			Operation::SaveChanges {
				channel,
				messages,
				retained,
				start,
			} => store
				.save_changes(account, channel, &messages, &retained, start)
				.map(|_| Outcome::Saved),
			Operation::SaveChannel {
				channel,
				messages,
				start,
			} => store
				.save_channel(account, channel, &messages, start)
				.map(|_| Outcome::Saved),
			Operation::DeleteMessages { channel, ids } => store
				.delete_messages(account, channel, &ids)
				.map(|_| Outcome::Saved),
			Operation::ClearHistory => store
				.clear_history(account)
				.map(|_| Outcome::HistoryCleared),
			Operation::Forget => store.forget_account(account).map(|_| Outcome::Saved),
		},
		Err(error) => Err(*error),
	};
	result.unwrap_or_else(|error| {
		if history_unsafe {
			history.fail();
		}
		if scoped_cleanup {
			return Outcome::ScopedCleanupFailed { account };
		}
		Outcome::Failed {
			error,
			message: failure_message,
			draft_restore,
			history_cleanup,
		}
	})
}

/// Offline restart check; uses only synthetic device preferences and opens no audio devices.
#[cfg(all(debug_assertions, feature = "demo"))]
pub fn debug_voice_preferences_check() {
	let root = std::env::temp_dir().join(format!(
		"serein-voice-preferences-{}-{}",
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	std::fs::create_dir(&root).unwrap();
	let path = root.join("client.sqlite3");
	let (send, commands) = mpsc::sync_channel(16);
	let (_, receive) = mpsc::sync_channel(16);
	let cache = Cache {
		send,
		receive,
		budget: Arc::new(Budget::default()),
		history: Arc::new(HistorySafety::default()),
	};
	let mut settings =
		crate::app_settings::Settings::from_preferences(Ok(local_store::AppPreferences::default()));
	let mut view = ui::MessagingUi::default();
	settings.apply(&mut view);
	view.set_voice_user_volume_overrides(&[(7, 35), (9, 150)]);
	view.set_voice_user_mutes(&[9]);
	settings.observe(&view);
	{
		let mut store = Ok(local_store::LocalStore::open(&path).unwrap());
		assert!(settings.save(Some(&cache), 0));
		let (_, account, epoch, operation, _reservation) = commands.try_recv().unwrap();
		assert!(matches!(
			execute(&mut store, &cache.history, account, epoch, operation),
			Outcome::AppPreferencesSaved(Ok(()))
		));
	}
	let mut restored = crate::app_settings::Settings::from_preferences(
		local_store::LocalStore::open(&path)
			.unwrap()
			.app_preferences(),
	);
	let mut restarted = ui::MessagingUi::default();
	restored.apply(&mut restarted);
	assert!(restored.loaded);
	assert!(restarted.voice_user_volumes().contains(&(7, 35)));
	assert!(restarted.voice_user_volumes().contains(&(9, 0)));
	assert_eq!(
		restarted.voice_user_volume_overrides(),
		vec![(7, 35), (9, 150)]
	);
	restored.observe(&restarted);
	assert!(!restored.state.touched && !restored.state.dirty);
	restarted.notifications_enabled = !restarted.notifications_enabled;
	restored.observe(&restarted);
	assert_eq!(restored.current.user_volumes, vec![(7, 35), (9, 150)]);
	assert_eq!(restored.current.muted_users, vec![9]);
	let mut failed =
		crate::app_settings::Settings::from_preferences(Err(local_store::StoreError::Unavailable));
	failed.apply(&mut restarted);
	restarted.notifications_enabled = !restarted.notifications_enabled;
	failed.observe(&restarted);
	assert!(!failed.save(Some(&cache), 0));
	assert!(matches!(
		commands.try_recv(),
		Err(mpsc::TryRecvError::Empty)
	));
	assert!(failed.state.failed && !failed.state.saving);
	assert_eq!(
		local_store::LocalStore::open(&path)
			.unwrap()
			.app_preferences()
			.unwrap()
			.user_volumes,
		vec![(7, 35), (9, 150)]
	);
	std::fs::remove_file(path).unwrap();
	std::fs::remove_dir(root).unwrap();
	println!(
		"Voice preferences debug check passed: SQLite reopen, startup restore, independent local mute and subsequent preference edits. No Discord or audio devices accessed."
	);
}

#[cfg(test)]
mod tests {
	use super::*;
	fn expression_hint(guild: Id, user: u64) -> model::ExpressionCatalog {
		model::ExpressionCatalog {
			guild,
			emojis: Some(vec![model::CustomEmoji {
				id: Id(user),
				name: "cached_emoji".into(),
				animated: true,
				available: false,
				managed: false,
				roles: None,
			}]),
			stickers: Some(vec![model::Sticker {
				id: Id(user + 1),
				name: "Cached sticker".into(),
				description: String::new(),
				tags: String::new(),
				format_type: 1,
				guild_id: Some(guild),
				pack_id: None,
				available: false,
			}]),
		}
	}
	#[test]
	fn expression_worker_restores_only_display_hints_and_fences_scoped_cleanup() {
		let mut state = test_support::demo_state();
		state.demo = false;
		let account = state.user.as_ref().unwrap().id;
		let guild = Id(10);
		let current = state.guilds.iter_mut().find(|g| g.id == guild).unwrap();
		current.emojis = None;
		current.stickers = None;
		let hint = expression_hint(guild, 90_000);
		assert!(hint.valid());
		let other = expression_hint(Id(11), 91_000);
		let safety = HistorySafety::default();
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		for (owner, value) in [
			(account, hint.clone()),
			(account, other.clone()),
			(Id(999_999), hint.clone()),
		] {
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					owner,
					safety.epoch(),
					Operation::SaveExpressions(Box::new(value))
				),
				Outcome::Saved
			));
		}
		let revision = state.expression_guild_cache_revision(guild);
		let old_epoch = safety.epoch();
		let Outcome::Expressions {
			account: owner,
			result: Ok(Some(restored)),
			..
		} = execute(
			&mut store,
			&safety,
			account,
			old_epoch,
			Operation::LoadExpressions { guild, revision },
		)
		else {
			panic!("expected cached expression hints")
		};
		assert_eq!(owner, account);
		assert!(state.hydrate_expression_catalog(guild, revision, restored));
		assert_eq!(
			state.display_guild_emojis(guild).unwrap()[0].name,
			"cached_emoji"
		);
		assert!(state.guild(guild).unwrap().emojis.is_none());
		assert!(!state.can_send_sticker(&hint.stickers.as_ref().unwrap()[0]));
		safety.invalidate();
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				old_epoch,
				Operation::SaveExpressions(Box::new(hint.clone()))
			),
			Outcome::Saved
		));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				safety.epoch(),
				Operation::ClearExpressions {
					guild,
					emoji: Some(true)
				}
			),
			Outcome::Saved
		));
		let remaining = store
			.as_ref()
			.unwrap()
			.load_expression_catalog(account, guild)
			.unwrap()
			.unwrap();
		assert!(remaining.emojis.is_none());
		assert_eq!(remaining.stickers.as_ref(), hint.stickers.as_ref());
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				safety.epoch(),
				Operation::ClearExpressions { guild, emoji: None }
			),
			Outcome::Saved
		));
		let db = store.as_ref().unwrap();
		assert!(
			db.load_expression_catalog(account, guild)
				.unwrap()
				.is_none()
		);
		assert_eq!(
			db.load_expression_catalog(account, Id(11)).unwrap(),
			Some(other)
		);
		assert_eq!(
			db.load_expression_catalog(Id(999_999), guild).unwrap(),
			Some(hint)
		);
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				old_epoch,
				Operation::LoadExpressions { guild, revision }
			),
			Outcome::Expressions {
				result: Ok(None),
				..
			}
		));
	}
	#[test]
	fn expression_schedule_coalesces_bounds_and_retries_after_progress() {
		let mut work = ExpressionCacheSchedule::default();
		work.scope((1, Id(1), 0), (1..=180).map(Id));
		assert_eq!(work.pending.len(), ExpressionCacheSchedule::LIMIT);
		work.selected(Some(Id(180)));
		assert_eq!(work.pending.len(), ExpressionCacheSchedule::LIMIT);
		assert_eq!(work.next(), Some((Id(180), ExpressionCacheSchedule::LOAD)));
		work.enqueue(Id(180), ExpressionCacheSchedule::SAVE);
		assert_eq!(work.pending.len(), ExpressionCacheSchedule::LIMIT);
		let (send, commands) = mpsc::sync_channel(1);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		assert!(cache.queue(1, Id(1), Operation::LoadDrafts));
		assert!(!cache.queue(
			1,
			Id(1),
			Operation::LoadExpressions {
				guild: Id(180),
				revision: 0
			}
		));
		work.blocked();
		assert!(work.next().is_none());
		drop(commands.try_recv().unwrap());
		work.capacity_changed();
		assert!(cache.queue(
			1,
			Id(1),
			Operation::LoadExpressions {
				guild: Id(180),
				revision: 0
			}
		));
		work.completed(Id(180), ExpressionCacheSchedule::LOAD);
		assert_eq!(work.next(), Some((Id(180), ExpressionCacheSchedule::SAVE)));
		work.remove(Id(180));
		assert_ne!(work.next().map(|(id, _)| id), Some(Id(180)));
		work.scope((2, Id(2), 0), std::iter::once(Id(300)));
		assert_eq!(work.next(), Some((Id(300), ExpressionCacheSchedule::LOAD)));
		work.completed(Id(300), ExpressionCacheSchedule::LOAD);
		work.completed(Id(300), ExpressionCacheSchedule::SAVE);
		work.refresh(std::iter::once(Id(300)));
		assert_eq!(work.next(), Some((Id(300), ExpressionCacheSchedule::LOAD)));
		work.completed(Id(300), ExpressionCacheSchedule::LOAD);
		assert!(
			work.next().is_none(),
			"permission refresh must not resave unchanged catalogs"
		);
		work.scope((3, Id(1), 0), (1..=128).map(Id));
		work.selected(Some(Id(1)));
		work.observe_epoch(7, std::iter::empty());
		work.observe_epoch(8, (129..=256).map(Id));
		assert_eq!(work.pending.len(), ExpressionCacheSchedule::LIMIT);
		assert_eq!(work.next(), Some((Id(1), ExpressionCacheSchedule::LOAD)));
		assert!(work.pending.iter().any(|(guild, _)| *guild == Id(256)));
		let snapshot = work.pending.clone();
		work.observe_epoch(8, std::iter::once(Id(999)));
		assert_eq!(
			work.pending, snapshot,
			"unchanged epoch must not rescan guilds"
		);
		assert!(coalesced_retry(&Operation::SaveGifFavorites(Vec::new())));
		assert!(coalesced_retry(&Operation::SaveExpressions(Box::new(
			expression_hint(Id(10), 93000)
		))));
		assert!(coalesced_retry(&Operation::SaveStickerPackHints(Box::new(
			model::StickerPackHints { packs: vec![] }
		))));
		assert!(!coalesced_retry(&Operation::SaveDraft {
			channel: Id(20),
			content: String::new()
		}));
	}
	#[test]
	fn expression_scheduler_restores_joined_guilds_after_sqlite_restart() {
		let root = std::env::temp_dir().join(format!(
			"serein-expressions-restart-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		std::fs::create_dir(&root).unwrap();
		let path = root.join("cache.sqlite");
		let mut state = test_support::demo_state();
		state.demo = false;
		let account = state.user.as_ref().unwrap().id;
		let mut second = state.guilds[0].clone();
		second.id = Id(11);
		second.emojis = Some(vec![model::CustomEmoji {
			id: Id(55_001),
			name: "other_server".into(),
			animated: false,
			available: true,
			managed: false,
			roles: None,
		}]);
		state.guilds.push(second);
		let safety = HistorySafety::default();
		{
			let mut store = Ok(LocalStore::open(&path).unwrap());
			for guild in [Id(10), Id(11)] {
				let catalog = state.cached_expression_catalog(guild).unwrap();
				assert!(matches!(
					execute(
						&mut store,
						&safety,
						account,
						safety.epoch(),
						Operation::SaveExpressions(Box::new(catalog))
					),
					Outcome::Saved
				));
				let row = state.guilds.iter_mut().find(|g| g.id == guild).unwrap();
				row.emojis = None;
				row.stickers = None;
			}
		}
		let mut reopened = Ok(LocalStore::open(&path).unwrap());
		let mut schedule = ExpressionCacheSchedule::default();
		schedule.scope(
			(state.generation, account, safety.epoch()),
			state.guilds.iter().map(|guild| guild.id),
		);
		while let Some((guild, operation)) = schedule.next() {
			if operation == ExpressionCacheSchedule::LOAD {
				let revision = state.expression_guild_cache_revision(guild);
				let Outcome::Expressions {
					result: Ok(Some(hints)),
					..
				} = execute(
					&mut reopened,
					&safety,
					account,
					safety.epoch(),
					Operation::LoadExpressions { guild, revision },
				)
				else {
					panic!("missing cached guild")
				};
				assert!(state.hydrate_expression_catalog(guild, revision, hints));
			}
			schedule.completed(guild, operation);
		}
		assert_eq!(
			state.display_guild_emojis(Id(11)).unwrap()[0].name,
			"other_server"
		);
		assert!(!state.display_guild_emojis(Id(11)).unwrap()[0].available);
		assert!(state.guild(Id(10)).unwrap().emojis.is_none());
		assert!(state.guild(Id(11)).unwrap().emojis.is_none());
		assert!(
			reopened
				.as_ref()
				.unwrap()
				.load_expression_catalog(Id(2), Id(11))
				.unwrap()
				.is_none()
		);
		drop(reopened);
		std::fs::remove_dir_all(root).unwrap();
	}
	#[test]
	fn expression_queue_charges_and_releases_bounded_read_capacity() {
		let (send, commands) = mpsc::sync_channel(1);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		assert!(cache.queue(
			1,
			Id(1),
			Operation::LoadExpressions {
				guild: Id(10),
				revision: 0
			}
		));
		assert!(*cache.budget.used.lock().unwrap() >= 8 * 1024 * 1024);
		assert!(!cache.queue(
			1,
			Id(1),
			Operation::LoadExpressions {
				guild: Id(11),
				revision: 0
			}
		));
		drop(commands.try_recv().unwrap());
		assert_eq!(*cache.budget.used.lock().unwrap(), 0);
		assert!(cache.queue(1, Id(1), Operation::LoadStickerPackHints { revision: 0 }));
		assert!(*cache.budget.used.lock().unwrap() >= 8 * 1024 * 1024);
		drop(commands.try_recv().unwrap());
		assert_eq!(*cache.budget.used.lock().unwrap(), 0);
	}
	#[test]
	fn expression_results_reserve_decoded_memory_until_consumed() {
		let catalog = expression_hint(Id(10), 93_000);
		let pack_id = Id(93_100);
		let packs = model::StickerPackHints {
			packs: vec![model::StickerPack {
				id: pack_id,
				name: "Cached pack".into(),
				stickers: vec![model::Sticker {
					id: Id(93_101),
					name: "Cached sticker".into(),
					description: String::new(),
					tags: String::new(),
					format_type: 1,
					guild_id: None,
					pack_id: Some(pack_id),
					available: false,
				}],
			}],
		};
		let expression_bytes = catalog.bytes();
		let pack_bytes = packs.bytes();
		let outcomes = [
			(
				Outcome::Expressions {
					account: Id(1),
					guild: Id(10),
					revision: 0,
					epoch: 0,
					result: Ok(Some(catalog)),
				},
				expression_bytes,
			),
			(
				Outcome::StickerPackHints {
					account: Id(1),
					revision: 0,
					epoch: 0,
					result: Ok(Some(packs)),
				},
				pack_bytes,
			),
		];
		let budget = Arc::new(Budget::default());
		for (outcome, expected) in outcomes {
			assert!(expected > 0);
			assert_eq!(outcome_bytes(&outcome), expected);
			let held = budget
				.reserve(outcome_bytes(&outcome) + 64 * 1024, false)
				.unwrap();
			assert!(
				budget
					.reserve(QUEUE_BYTES - held.bytes + 1, false)
					.is_none()
			);
			drop(held);
			assert!(budget.reserve(QUEUE_BYTES, false).is_some());
		}
		assert_eq!(
			outcome_bytes(&Outcome::StickerPackHints {
				account: Id(1),
				revision: 0,
				epoch: 0,
				result: Ok(None),
			}),
			0
		);
	}
	#[test]
	fn sticker_pack_worker_restores_disabled_hints_and_clears_only_the_account() {
		let mut state = test_support::demo_state();
		state.demo = false;
		state.stickers.loaded = false;
		let account = state.user.as_ref().unwrap().id;
		let pack_id = Id(92_000);
		let hints = model::StickerPackHints {
			packs: vec![model::StickerPack {
				id: pack_id,
				name: "Cached pack".into(),
				stickers: vec![model::Sticker {
					id: Id(92_001),
					name: "Cached sticker".into(),
					description: String::new(),
					tags: String::new(),
					format_type: 1,
					guild_id: None,
					pack_id: Some(pack_id),
					available: false,
				}],
			}],
		};
		assert!(hints.valid());
		let safety = HistorySafety::default();
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		for owner in [account, Id(999_999)] {
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					owner,
					safety.epoch(),
					Operation::SaveStickerPackHints(Box::new(hints.clone()))
				),
				Outcome::Saved
			));
		}
		let revision = state.sticker_pack_cache_revision();
		let epoch = safety.epoch();
		let Outcome::StickerPackHints {
			account: owner,
			result: Ok(Some(restored)),
			..
		} = execute(
			&mut store,
			&safety,
			account,
			epoch,
			Operation::LoadStickerPackHints { revision },
		)
		else {
			panic!("expected cached sticker pack hints")
		};
		assert_eq!(owner, account);
		assert!(state.hydrate_sticker_pack_hints(revision, restored));
		assert!(
			!state.stickers.loaded,
			"a disk hint cannot suppress live refresh"
		);
		assert!(!state.can_send_sticker(&hints.packs[0].stickers[0]));
		safety.invalidate();
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				epoch,
				Operation::SaveStickerPackHints(Box::new(hints.clone()))
			),
			Outcome::Saved
		));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				safety.epoch(),
				Operation::ClearHistory
			),
			Outcome::HistoryCleared
		));
		assert!(
			store
				.as_ref()
				.unwrap()
				.load_sticker_pack_hints(account)
				.unwrap()
				.is_none()
		);
		assert_eq!(
			store
				.as_ref()
				.unwrap()
				.load_sticker_pack_hints(Id(999_999))
				.unwrap(),
			Some(hints)
		);
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				epoch,
				Operation::LoadStickerPackHints { revision }
			),
			Outcome::StickerPackHints {
				result: Ok(None),
				..
			}
		));
	}
	#[test]
	fn gif_cache_retries_only_unsent_work_after_worker_progress_and_scopes_accounts() {
		let (send, commands) = mpsc::sync_channel(1);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let mut state = GifCache::default();
		let scope = (7, Id(1));
		let key = (scope.0, scope.1, 0, 0);
		assert!(cache.queue(7, Id(1), Operation::LoadDrafts));
		assert!(state.load.attempt(key));
		assert!(!cache.queue(7, Id(1), Operation::LoadGifFavorites));
		assert!(
			!state.load.attempt(key),
			"repaint cannot busy-loop on a full queue"
		);
		drop(commands.try_recv().unwrap());
		state.capacity_changed();
		assert!(state.load.attempt(key));
		assert!(cache.queue(7, Id(1), Operation::LoadGifFavorites));
		state.load.accepted(key);
		assert!(state.load.was_accepted(key));
		assert!(!state.may_save(scope), "accepted load is not yet applied");
		assert!(!state.load.attempt(key));
		let gif = model::Gif {
			id: "cached".into(),
			title: "cached".into(),
			url: "https://tenor.com/view/cached".into(),
			preview: "https://media.tenor.com/cached/tenor.gif".into(),
			width: 100,
			height: 100,
		};
		assert!(!cache.queue(7, Id(1), Operation::SaveGifFavorites(vec![gif.clone()])));
		state.reject_save(scope);
		assert!(!state.may_save(scope));
		assert!(
			!state.may_save((8, Id(2))),
			"another account must load its own cache"
		);
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let (_, account, epoch, operation, reservation) = commands.try_recv().unwrap();
		assert!(matches!(
			execute(&mut store, &cache.history, account, epoch, operation),
			Outcome::GifFavorites(_)
		));
		drop(reservation);
		state.loaded(scope);
		state.capacity_changed();
		assert!(!state.load.attempt(key), "an accepted read is not repeated");
		assert!(state.may_save(scope));
		assert!(cache.queue(7, Id(1), Operation::SaveGifFavorites(vec![gif.clone()])));
		let (_, account, epoch, operation, _reservation) = commands.try_recv().unwrap();
		assert!(matches!(
			execute(&mut store, &cache.history, account, epoch, operation),
			Outcome::Saved
		));
		assert_eq!(
			store.as_ref().unwrap().gif_favorites(Id(1)).unwrap(),
			vec![gif]
		);
		assert!(
			store
				.as_ref()
				.unwrap()
				.gif_favorites(Id(2))
				.unwrap()
				.is_empty()
		);
	}
	#[test]
	fn server_display_worker_obeys_account_and_history_fences() {
		let mut store = LocalStore::open(std::path::Path::new(":memory:"));
		let safety = HistorySafety::default();
		let row = local_store::ServerDisplay {
			guild: Id(10),
			name: Some("cached server".into()),
			icon: Some(None),
		};
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(1),
				0,
				Operation::SaveServers {
					rows: vec![row.clone()],
					revision: 7
				}
			),
			Outcome::ServersSaved {
				result: Some(Ok(())),
				..
			}
		));
		let Outcome::Servers { result, .. } = execute(
			&mut store,
			&safety,
			Id(1),
			0,
			Operation::LoadServers {
				revision: 7,
				guilds: vec![Id(10)],
			},
		) else {
			panic!("server load")
		};
		assert_eq!(result.unwrap(), vec![row.clone()]);
		let Outcome::Servers { result, .. } = execute(
			&mut store,
			&safety,
			Id(2),
			0,
			Operation::LoadServers {
				revision: 7,
				guilds: vec![Id(10)],
			},
		) else {
			panic!("server load")
		};
		assert!(result.unwrap().is_empty());
		safety.invalidate();
		let updated = local_store::ServerDisplay {
			name: Some("renamed server".into()),
			..row.clone()
		};
		assert!(
			matches!(
				execute(
					&mut store,
					&safety,
					Id(1),
					0,
					Operation::SaveServers {
						rows: vec![updated.clone()],
						revision: 7
					}
				),
				Outcome::ServersSaved { result: None, .. }
			),
			"an invalidated write must not acknowledge dirty fields"
		);
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(1),
				safety.epoch(),
				Operation::SaveServers {
					rows: vec![updated.clone()],
					revision: 8
				}
			),
			Outcome::ServersSaved {
				result: Some(Ok(())),
				..
			}
		));
		let Outcome::Servers { result, .. } = execute(
			&mut store,
			&safety,
			Id(1),
			safety.epoch(),
			Operation::LoadServers {
				revision: 8,
				guilds: vec![Id(10)],
			},
		) else {
			panic!("server load")
		};
		assert_eq!(result.unwrap(), vec![updated]);
		let Outcome::Servers { result, .. } = execute(
			&mut store,
			&safety,
			Id(1),
			0,
			Operation::LoadServers {
				revision: 7,
				guilds: vec![Id(10)],
			},
		) else {
			panic!("server load")
		};
		assert!(result.unwrap().is_empty());
	}
	#[test]
	fn rejected_history_read_waits_for_capacity_then_retries_once() {
		let (send, commands) = mpsc::sync_channel(1);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let mut load = HistoryLoad::default();
		let key = (1, Id(2), 7, cache.history.epoch());
		assert!(cache.queue(1, Id(1), Operation::LoadDrafts));
		assert!(load.attempt(key));
		assert!(!cache.queue(
			1,
			Id(1),
			Operation::LoadChannel {
				channel: Id(2),
				request: 7
			}
		));
		for _ in 0..100 {
			assert!(!load.attempt(key), "repainting cannot spin on a full queue");
		}
		drop(commands.try_recv().unwrap());
		load.capacity_changed();
		assert!(load.attempt(key));
		assert!(cache.queue(
			1,
			Id(1),
			Operation::LoadChannel {
				channel: Id(2),
				request: 7
			}
		));
		load.accepted(key);
		load.capacity_changed();
		assert!(!load.attempt(key), "accepted read is never duplicated");
		assert!(
			load.attempt((2, Id(2), 7, 1)),
			"account and safety changes retire the attempt"
		);
	}
	fn member_hint_store(account: Id) -> Result<LocalStore, StoreError> {
		let mut store = LocalStore::open(std::path::Path::new(":memory:")).unwrap();
		store
			.save_member_displays(
				account,
				&[model::MemberDisplay {
					guild: Id(10),
					user: Id(900000),
					name: "Cached account".into(),
					username: Some("cached_account".into()),
					nick: Some("Cached nickname".into()),
				}],
			)
			.unwrap();
		Ok(store)
	}
	fn member_hint_rows(
		store: &mut Result<LocalStore, StoreError>,
		safety: &HistorySafety,
		account: Id,
		revision: u64,
	) -> Vec<model::MemberDisplay> {
		let Outcome::Members { result, .. } = execute(
			store,
			safety,
			account,
			safety.epoch(),
			Operation::LoadMembers {
				guild: Id(10),
				revision,
			},
		) else {
			panic!("member load")
		};
		result.unwrap()
	}
	#[test]
	fn resumed_session_reloads_member_hints_without_changing_history_epoch() {
		let mut state = test_support::demo_state();
		state.demo = false;
		let account = state.user.as_ref().unwrap().id;
		let safety = HistorySafety::default();
		let epoch = safety.epoch();
		let mut store = member_hint_store(account);
		let mut load = HistoryLoad::default();
		let generation = state.generation;
		let key = (generation, Id(10), 0, epoch);
		assert!(load.attempt(key));
		load.accepted(key);
		let revision = state.member_display_revision();
		let rows = member_hint_rows(&mut store, &safety, account, revision);
		state.hydrate_member_displays(Id(10), revision, rows);
		assert_eq!(
			state.member_nickname(Id(20), Id(900000)),
			Some(Some("Cached nickname"))
		);
		let stale = client_core::Envelope {
			generation: generation.wrapping_add(1),
			event: client_core::Event::Disconnected,
		};
		assert!(!load.observe_session(generation, &stale));
		state.apply(stale);
		assert!(
			!load.attempt(key),
			"obsolete session events cannot duplicate accepted reads"
		);
		let disconnected = client_core::Envelope {
			generation,
			event: client_core::Event::Disconnected,
		};
		assert!(load.observe_session(generation, &disconnected));
		state.apply(disconnected);
		assert!(state.member_identity(Id(20), Id(900000)).is_none());
		let resumed = client_core::Envelope {
			generation,
			event: client_core::Event::Resumed,
		};
		assert!(!load.observe_session(generation, &resumed));
		state.apply(resumed);
		assert_eq!(state.generation, generation);
		assert_eq!(safety.epoch(), epoch);
		assert!(load.attempt(key));
		load.accepted(key);
		let revision = state.member_display_revision();
		let rows = member_hint_rows(&mut store, &safety, account, revision);
		state.hydrate_member_displays(Id(10), revision, rows);
		assert_eq!(
			state.member_nickname(Id(20), Id(900000)),
			Some(Some("Cached nickname"))
		);
		load.capacity_changed();
		assert!(!load.attempt(key));
	}
	#[test]
	fn discarded_member_hints_retry_after_worker_progress_and_keep_live_updates() {
		let mut state = test_support::demo_state();
		state.demo = false;
		let account = state.user.as_ref().unwrap().id;
		let safety = HistorySafety::default();
		let mut store = member_hint_store(account);
		let mut load = HistoryLoad::default();
		let key = (state.generation, Id(10), 0, safety.epoch());
		assert!(load.attempt(key));
		load.accepted(key);
		let revision = state.member_display_revision();
		let rows = member_hint_rows(&mut store, &safety, account, revision);
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::MemberIdentity(Box::new(model::MemberIdentity {
				guild: Id(10),
				user: Id(900001),
				profile: None,
				nick: model::Patch::Value("Live nickname".into()),
				roles: model::Patch::Absent,
				departed: false,
			})),
		});
		assert_ne!(revision, state.member_display_revision());
		state.hydrate_member_displays(Id(10), revision, rows);
		assert!(
			state.member_identity(Id(20), Id(900000)).is_none(),
			"stale completion remains rejected"
		);
		assert!(
			!load.attempt(key),
			"repaint does not retry an accepted read"
		);
		// The worker completion is the explicit retry signal; it is not a frame timer.
		load.member_result(key, revision, &state);
		assert!(load.attempt(key));
		load.accepted(key);
		let revision = state.member_display_revision();
		let rows = member_hint_rows(&mut store, &safety, account, revision);
		state.hydrate_member_displays(Id(10), revision, rows);
		assert_eq!(
			state.member_nickname(Id(20), Id(900000)),
			Some(Some("Cached nickname"))
		);
		assert_eq!(
			state.member_nickname(Id(20), Id(900001)),
			Some(Some("Live nickname"))
		);
		load.capacity_changed();
		assert!(!load.attempt(key));
		load.retry((key.0, Id(11), key.2, key.3));
		assert!(
			!load.attempt(key),
			"another guild's completion cannot restart this read"
		);
	}
	#[test]
	fn member_load_completed_away_from_server_retries_after_return() {
		for home in [false, true] {
			let mut state = test_support::demo_state();
			state.demo = false;
			let account = state.user.as_ref().unwrap().id;
			let safety = HistorySafety::default();
			let mut store = member_hint_store(account);
			let mut load = HistoryLoad::default();
			let key = (state.generation, Id(10), 0, safety.epoch());
			assert!(load.attempt(key));
			load.accepted(key);
			let revision = state.member_display_revision();
			let rows = member_hint_rows(&mut store, &safety, account, revision);
			if home {
				state.arrived_home();
			} else {
				state.select(Id(22));
			}
			assert_eq!(
				state.member_display_revision(),
				revision,
				"ordinary navigation is not a new identity observation"
			);
			load.member_result(key, revision, &state);
			state.hydrate_member_displays(Id(10), revision, rows);
			assert!(
				state.member_identity(Id(20), Id(900000)).is_none(),
				"a completion outside its server remains rejected"
			);
			state.select(Id(20));
			assert!(
				load.attempt(key),
				"worker progress released the old server admission marker"
			);
			load.accepted(key);
			let revision = state.member_display_revision();
			let rows = member_hint_rows(&mut store, &safety, account, revision);
			load.member_result(key, revision, &state);
			state.hydrate_member_displays(Id(10), revision, rows);
			assert_eq!(
				state.member_nickname(Id(20), Id(900000)),
				Some(Some("Cached nickname"))
			);
			load.capacity_changed();
			assert!(
				!load.attempt(key),
				"successful hydration is not repeated on repaint"
			);
		}
	}
	#[test]
	fn scoped_cleanup_fences_old_writes_and_preserves_other_windows() {
		let (send, commands) = mpsc::sync_channel(4);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		for (account, channel) in [(Id(1), Id(2)), (Id(1), Id(3)), (Id(9), Id(2))] {
			store
				.as_mut()
				.unwrap()
				.save_channel(
					account,
					channel,
					&[test_support::message(10, channel)],
					false,
				)
				.unwrap();
		}
		let old = cache.history.epoch();
		assert!(cache.queue(
			1,
			Id(1),
			Operation::SaveChannel {
				channel: Id(2),
				messages: vec![test_support::message(11, Id(2))],
				start: false
			}
		));
		assert!(cache.invalidate_scope(1, Id(1), Operation::ClearChannel { channel: Id(2) }));
		assert!(!cache.history.allows(old));
		while let Ok((_, account, epoch, op, _reservation)) = commands.try_recv() {
			assert!(matches!(
				execute(&mut store, &cache.history, account, epoch, op),
				Outcome::Saved
			));
		}
		let db = store.as_ref().unwrap();
		assert!(db.load_channel(Id(1), Id(2)).unwrap().is_empty());
		assert_eq!(db.load_channel(Id(1), Id(3)).unwrap().len(), 1);
		assert_eq!(db.load_channel(Id(9), Id(2)).unwrap().len(), 1);
		assert!(cache.history.allows(cache.history.epoch()));
		let mut unavailable = Err(StoreError::Unavailable);
		assert!(matches!(
			execute(
				&mut unavailable,
				&cache.history,
				Id(1),
				cache.history.epoch(),
				Operation::ClearChannel { channel: Id(3) }
			),
			Outcome::ScopedCleanupFailed { account: Id(1) }
		));
		assert!(!cache.history.allows(cache.history.epoch()));
	}
	#[test]
	fn member_storage_worker_roundtrip_and_deletion_are_epoch_fenced() {
		let safety = HistorySafety::default();
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let row = model::MemberDisplay {
			guild: Id(10),
			user: Id(2),
			name: "Synthetic".into(),
			username: Some("account".into()),
			nick: Some("Nickname".into()),
		};
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(1),
				0,
				Operation::SaveMembers(vec![row.clone()])
			),
			Outcome::Saved
		));
		let Outcome::Members {
			result: Ok(rows),
			epoch,
			..
		} = execute(
			&mut store,
			&safety,
			Id(1),
			0,
			Operation::LoadMembers {
				guild: Id(10),
				revision: 0,
			},
		)
		else {
			panic!("worker read");
		};
		assert_eq!(rows, vec![row.clone()]);
		safety.invalidate();
		assert!(!safety.allows(epoch));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(1),
				safety.epoch(),
				Operation::RemoveMember {
					guild: Id(10),
					user: Some(Id(2))
				}
			),
			Outcome::Saved
		));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(1),
				0,
				Operation::SaveMembers(vec![row])
			),
			Outcome::Saved
		));
		assert!(
			store
				.as_ref()
				.unwrap()
				.load_member_displays(Id(1), Id(10))
				.unwrap()
				.is_empty()
		);
	}

	#[test]
	fn scoped_cleanup_retries_member_save_without_undoing_an_explicit_clear() {
		let (send, commands) = mpsc::sync_channel(4);
		let (_, receive) = mpsc::sync_channel(4);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let mut state = test_support::demo_state();
		state.demo = false;
		let account = state.user.as_ref().unwrap().id;
		let row = model::MemberDisplay {
			guild: Id(10),
			user: Id(900000),
			name: "Synthetic".into(),
			username: Some("account".into()),
			nick: Some("Current nickname".into()),
		};
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let mut cleared = MemberCacheClear::default();
		let first = member_save_key(&state, Id(10), cache.history.epoch(), &cleared).unwrap();
		assert!(cache.queue(
			state.generation,
			account,
			Operation::SaveMembers(vec![row.clone()])
		));
		assert!(cache.invalidate_scope(
			state.generation,
			account,
			Operation::ClearChannel { channel: Id(20) },
		));
		while let Ok((_, owner, epoch, operation, _reservation)) = commands.try_recv() {
			assert!(matches!(
				execute(&mut store, &cache.history, owner, epoch, operation),
				Outcome::Saved
			));
		}
		assert!(
			store
				.as_ref()
				.unwrap()
				.load_member_displays(account, Id(10))
				.unwrap()
				.is_empty()
		);
		let retry = member_save_key(&state, Id(10), cache.history.epoch(), &cleared).unwrap();
		assert_ne!(
			first, retry,
			"scoped invalidation must retry the retired save"
		);
		assert!(cache.queue(
			state.generation,
			account,
			Operation::SaveMembers(vec![row.clone()])
		));
		let (_, owner, epoch, operation, _reservation) = commands.try_recv().unwrap();
		assert!(matches!(
			execute(&mut store, &cache.history, owner, epoch, operation),
			Outcome::Saved
		));
		assert_eq!(
			store
				.as_ref()
				.unwrap()
				.load_member_displays(account, Id(10))
				.unwrap(),
			vec![row.clone()]
		);

		// Full clearing suppresses retained hints for each guild independently. A
		// fresh observation in one guild must not restore another guild's old rows.
		state.hydrate_member_displays(Id(10), state.member_display_revision(), vec![row]);
		let identity = model::MemberIdentity {
			guild: Id(10),
			user: Id(900001),
			profile: None,
			nick: model::Patch::Value("Fresh".into()),
			roles: model::Patch::Absent,
			departed: false,
		};
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::MemberIdentity(Box::new(identity.clone())),
		});
		cleared.reset(&state);
		assert!(cleared.blocked.contains(&Id(10)));
		cleared.blocked.insert(Id(11));
		assert!(member_save_key(&state, Id(10), cache.history.epoch(), &cleared).is_none());
		let before = state.member_display_revision();
		let saved_revision = state.member_display_cache_revision();
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::MemberIdentity(Box::new(identity)),
		});
		assert_eq!(state.member_display_cache_revision(), saved_revision);
		assert_ne!(state.member_display_revision(), before);
		cleared.observe(&state, Some(Id(10)), before);
		assert!(member_save_key(&state, Id(10), cache.history.epoch(), &cleared).is_some());
		assert!(member_save_key(&state, Id(11), cache.history.epoch(), &cleared).is_none());
		state.generation = state.generation.wrapping_add(1);
		assert!(member_save_key(&state, Id(11), cache.history.epoch(), &cleared).is_some());
	}

	#[test]
	fn partial_live_nickname_evicts_old_disk_hint_across_reopen() {
		use std::time::{SystemTime, UNIX_EPOCH};
		let root = std::env::temp_dir().join(format!(
			"serein-partial-nickname-{}-{}",
			std::process::id(),
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_nanos(),
		));
		std::fs::create_dir(&root).unwrap();
		for (case, nick, typing) in [
			(0, model::Patch::Null, false),
			(1, model::Patch::Value("New nickname".into()), false),
			(2, model::Patch::Value("Typing nickname".into()), true),
		] {
			let path = root.join(format!("{case}.sqlite3"));
			let mut state = test_support::demo_state();
			state.demo = false;
			let account = state.user.as_ref().unwrap().id;
			let mut store = Ok(LocalStore::open(&path).unwrap());
			let stale = model::MemberDisplay {
				guild: Id(10),
				user: Id(900000),
				name: "Cached".into(),
				username: Some("cached".into()),
				nick: Some("Old nickname".into()),
			};
			store
				.as_mut()
				.unwrap()
				.save_member_displays(account, &[stale])
				.unwrap();
			let safety = HistorySafety::default();
			let rows = member_hint_rows(
				&mut store,
				&safety,
				account,
				state.member_display_revision(),
			);
			state.hydrate_member_displays(Id(10), state.member_display_revision(), rows);
			let identity = model::MemberIdentity {
				guild: Id(10),
				user: Id(900000),
				profile: None,
				nick,
				roles: model::Patch::Absent,
				departed: false,
			};
			let make_event = || {
				if typing {
					client_core::Event::Typing(client_core::typing::Signal {
						channel: Id(20),
						user: Id(900000),
						timestamp: SystemTime::now()
							.duration_since(UNIX_EPOCH)
							.unwrap()
							.as_secs(),
						member: Some(Box::new(identity.clone())),
					})
				} else {
					client_core::Event::MemberIdentity(Box::new(identity.clone()))
				}
			};
			let departed = model::MemberIdentity {
				departed: true,
				..identity.clone()
			};
			let stale_departure = client_core::Envelope {
				generation: state.generation.wrapping_sub(1),
				event: client_core::Event::MemberIdentity(Box::new(departed.clone())),
			};
			assert_eq!(departed_member_target(&state, &stale_departure), None);
			let current_departure = client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::MemberIdentity(Box::new(departed)),
			};
			assert_eq!(
				departed_member_target(&state, &current_departure),
				Some((Id(10), Id(900000)))
			);
			let target = nickname_target(&make_event());
			let before = state.member_display_cache_revision();
			state.apply(client_core::Envelope {
				generation: state.generation.wrapping_sub(1),
				event: make_event(),
			});
			assert_eq!(
				incomplete_nickname_cleanup(&state, target, before),
				None,
				"stale generation is ignored"
			);
			let expired = client_core::Event::Typing(client_core::typing::Signal {
				channel: Id(20),
				user: Id(900000),
				timestamp: 0,
				member: Some(Box::new(identity.clone())),
			});
			let target = nickname_target(&expired);
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: expired,
			});
			assert_eq!(
				incomplete_nickname_cleanup(&state, target, before),
				None,
				"expired typing is ignored"
			);
			let target = nickname_target(&make_event());
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: make_event(),
			});
			assert_eq!(
				incomplete_nickname_cleanup(&state, target, before),
				Some((Id(10), Id(900000)))
			);
			safety.invalidate();
			let epoch = safety.epoch();
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					account,
					epoch,
					Operation::RemoveMember {
						guild: Id(10),
						user: Some(Id(900000))
					}
				),
				Outcome::Saved
			));
			drop(store);
			assert!(
				LocalStore::open(&path)
					.unwrap()
					.load_member_displays(account, Id(10))
					.unwrap()
					.is_empty()
			);
		}
		std::fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn credential_cleanup_retries_full_storage_without_repeating_keyring_deletion() {
		let (send, commands) = mpsc::sync_channel(1);
		let (_, receive) = mpsc::sync_channel(1);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let active = Id(1);
		let removed = Id(2);
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let saved = model::SavedAccount {
			id: removed,
			name: "Synthetic".into(),
			display: None,
			avatar: None,
			discriminator: 0,
			has_token: true,
		};
		store.as_mut().unwrap().save_account(&saved).unwrap();
		for account in [active, removed] {
			store
				.as_mut()
				.unwrap()
				.save_draft(account, Id(7), "offline draft")
				.unwrap();
		}
		assert!(cache.queue(1, active, Operation::LoadDrafts));
		let mut pending = AccountForgets::default();
		// The OS deletion has already succeeded. A full storage queue retains only
		// this acknowledgment; retrying must not repeat a credential-store operation.
		assert!(pending.request(1, removed));
		assert_eq!(pending.next(2, Some(active)), Some(removed));
		assert!(!cache.queue(2, removed, Operation::Forget));
		assert!(pending.contains(removed));
		assert_eq!(store.as_ref().unwrap().accounts().unwrap()[0].id, removed);
		let (_, _, _, _, reservation) = commands.try_recv().unwrap();
		drop(reservation);
		assert!(cache.queue(2, pending.next(2, Some(active)).unwrap(), Operation::Forget));
		pending.queued(removed);
		let (generation, account, epoch, operation, reservation) = commands.try_recv().unwrap();
		assert_eq!((generation, account), (2, removed));
		assert!(matches!(
			execute(&mut store, &cache.history, account, epoch, operation),
			Outcome::Saved
		));
		drop(reservation);
		assert!(!pending.pending());
		assert!(store.as_ref().unwrap().accounts().unwrap().is_empty());
		assert!(
			store
				.as_ref()
				.unwrap()
				.load_drafts(removed)
				.unwrap()
				.is_empty()
		);
		assert_eq!(
			store.as_ref().unwrap().load_drafts(active).unwrap()[&Id(7)],
			"offline draft"
		);
		assert!(commands.try_recv().is_err());
		assert_eq!(*cache.budget.used.lock().unwrap(), 0);
	}

	#[test]
	fn pending_account_cleanup_is_bounded_and_new_signin_supersedes_only_its_account() {
		let mut pending = AccountForgets::default();
		for id in 1..=model::MAX_SAVED_ACCOUNTS as u64 {
			assert!(pending.request(1, Id(id)));
			assert!(
				pending.request(1, Id(id)),
				"repeated acknowledgments coalesce"
			);
		}
		assert!(!pending.request(1, Id(9)));
		assert_eq!(pending.next(2, Some(Id(1))), Some(Id(2)));
		assert!(
			!pending.contains(Id(1)),
			"new sign-in preserves its account data"
		);
		assert!(pending.contains(Id(2)), "switching preserves other cleanup");
		for id in 2..=model::MAX_SAVED_ACCOUNTS as u64 {
			assert_eq!(pending.next(3, None), Some(Id(id)));
			pending.queued(Id(id));
		}
		assert!(!pending.pending());
		assert!(
			pending.request(3, Id(1)),
			"a later explicit deletion is independent"
		);
		assert_eq!(pending.next(3, Some(Id(1))), Some(Id(1)));
	}

	#[test]
	fn premium_unicode_drafts_enter_the_worker_and_reject_only_oversized_content() {
		use model::message_options::{MAX_DRAFT_CONTENT_BYTES, MAX_PREMIUM_CONTENT};
		let (send, commands) = mpsc::sync_channel(16);
		let (_, receive) = mpsc::sync_channel(16);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let draft = format!("@silent\u{2003}{}", "🦀".repeat(MAX_PREMIUM_CONTENT));
		assert!(cache.queue(
			7,
			Id(1),
			Operation::SaveDraft {
				channel: Id(2),
				content: draft.clone()
			}
		));
		let (_, _, epoch, operation, reservation) = commands.try_recv().unwrap();
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		assert!(matches!(
			execute(&mut store, &cache.history, Id(1), epoch, operation),
			Outcome::Saved
		));
		assert_eq!(
			store.as_ref().unwrap().load_drafts(Id(1)).unwrap()[&Id(2)],
			draft
		);
		drop(reservation);
		assert_eq!(*cache.budget.used.lock().unwrap(), 0);
		assert!(!cache.queue(
			7,
			Id(1),
			Operation::SaveDraft {
				channel: Id(2),
				content: "x".repeat(MAX_DRAFT_CONTENT_BYTES + 1)
			}
		));
		assert!(commands.try_recv().is_err());
	}

	#[test]
	fn shortcut_restore_waits_for_queue_space_without_losing_or_duplicating_the_request() {
		let (send, commands) = mpsc::sync_channel(16);
		let (_, receive) = mpsc::sync_channel(16);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let mut view = ui::MessagingUi::default();
		view.channel_preferences_reload = true;
		for _ in 0..16 {
			assert!(cache.queue(7, Id(0), Operation::LoadAppearance));
		}
		assert!(!crate::queue_channel_preferences(
			Some(&cache),
			&mut view,
			7,
			Id(42)
		));
		assert!(view.channel_preferences_reload);
		assert!(!view.channel_preferences_load_pending);
		assert!(view.channel_preferences_status.is_empty());
		commands.try_recv().unwrap();
		assert!(crate::queue_channel_preferences(
			Some(&cache),
			&mut view,
			7,
			Id(42)
		));
		assert!(!view.channel_preferences_reload);
		assert!(view.channel_preferences_load_pending);
		// A second READY or retry click while loading cannot enqueue another restore.
		view.channel_preferences_reload = true;
		assert!(!crate::queue_channel_preferences(
			Some(&cache),
			&mut view,
			7,
			Id(42)
		));
		for _ in 0..15 {
			assert!(matches!(
				commands.try_recv().unwrap().3,
				Operation::LoadAppearance
			));
		}
		let (generation, account, epoch, operation, reservation) = commands.try_recv().unwrap();
		assert_eq!((generation, account), (7, Id(42)));
		assert!(matches!(operation, Operation::LoadChannelPreferences));
		assert!(commands.try_recv().is_err());
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let preferences = model::ChannelPreferences {
			favorites: vec![Id(19)],
			pinned: vec![Id(20)],
			collapsed_categories: vec![Id(21)],
			last_channels: vec![(Id(22), Id(23))],
		};
		store
			.as_ref()
			.unwrap()
			.save_channel_preferences(account, &preferences)
			.unwrap();
		let Outcome::ChannelPreferences(Ok(restored)) =
			execute(&mut store, &cache.history, account, epoch, operation)
		else {
			panic!("Expected restored shortcuts");
		};
		assert_eq!(restored, preferences);
		drop(reservation);
		assert_eq!(*cache.budget.used.lock().unwrap(), 0);
		view.clear();
		assert!(!view.channel_preferences_load_pending && !view.channel_preferences_reload);
		view.channel_preferences_reload = true;
		assert!(!crate::queue_channel_preferences(
			None,
			&mut view,
			8,
			Id(43)
		));
		assert!(!view.channel_preferences_reload);
		assert!(!view.channel_preferences_status.is_empty());
	}

	#[test]
	fn notification_choice_survives_a_full_queue_and_database_reopen() {
		let root = std::env::temp_dir().join(format!(
			"serein-notification-restart-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		std::fs::create_dir(&root).unwrap();
		let path = root.join("client.sqlite3");
		let (send, commands) = mpsc::sync_channel(16);
		let (_, receive) = mpsc::sync_channel(16);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let initial = LocalStore::open(&path)
			.unwrap()
			.app_preferences()
			.unwrap()
			.notifications_enabled;
		for enabled in [!initial, initial] {
			let mut store = Ok(LocalStore::open(&path).unwrap());
			let mut settings = crate::app_settings::Settings {
				current: store.as_ref().unwrap().app_preferences().unwrap(),
				loaded: true,
				..Default::default()
			};
			let mut view = ui::MessagingUi::default();
			settings.apply(&mut view);
			view.notifications_enabled = enabled;
			settings.observe(&view);
			for _ in 0..16 {
				assert!(cache.queue(1, Id(0), Operation::LoadAppearance));
			}
			assert!(!settings.save(Some(&cache), 1));
			assert!(settings.state.needs_attention());
			for _ in 0..16 {
				commands.try_recv().unwrap();
			}
			// The next frame retries without another click, after cache work has drained.
			settings.observe(&view);
			assert!(settings.save(Some(&cache), 2));
			assert!(!settings.save(Some(&cache), 2));
			let (_, account, epoch, operation, _reservation) = commands.try_recv().unwrap();
			assert!(matches!(
				execute(&mut store, &cache.history, account, epoch, operation),
				Outcome::AppPreferencesSaved(Ok(()))
			));
			drop(store);
			let reopened = LocalStore::open(&path).unwrap();
			let restored = crate::app_settings::Settings {
				current: reopened.app_preferences().unwrap(),
				..Default::default()
			};
			let mut restarted_view = ui::MessagingUi::default();
			restored.apply(&mut restarted_view);
			assert_eq!(restarted_view.notifications_enabled, enabled);
		}
		std::fs::remove_file(path).unwrap();
		std::fs::remove_dir(root).unwrap();
	}

	#[test]
	fn minimize_to_tray_results_are_independent_of_account_history() {
		let safety = HistorySafety::default();
		safety.fail();
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		assert!(matches!(
			execute(&mut store, &safety, Id(0), 0, Operation::LoadMinimizeToTray),
			Outcome::MinimizeToTray(Ok(true))
		));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				Id(0),
				0,
				Operation::SaveMinimizeToTray(false)
			),
			Outcome::MinimizeToTraySaved(Ok(()))
		));
		assert!(matches!(
			execute(&mut store, &safety, Id(9), 0, Operation::LoadMinimizeToTray),
			Outcome::MinimizeToTray(Ok(false))
		));
		let mut unavailable = Err(StoreError::Unavailable);
		assert!(matches!(
			execute(
				&mut unavailable,
				&safety,
				Id(0),
				0,
				Operation::LoadMinimizeToTray
			),
			Outcome::MinimizeToTray(Err(StoreError::Unavailable))
		));
		assert!(matches!(
			execute(
				&mut unavailable,
				&safety,
				Id(0),
				0,
				Operation::SaveMinimizeToTray(false)
			),
			Outcome::MinimizeToTraySaved(Err(StoreError::Unavailable))
		));

		{
			let safety = HistorySafety::default();
			safety.block();
			let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
			assert!(matches!(
				execute(&mut store, &safety, Id(0), 0, Operation::LoadGameActivity),
				Outcome::GameActivity(Ok(false))
			));
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					Id(0),
					0,
					Operation::SaveGameActivity(true)
				),
				Outcome::GameActivitySaved(Ok(()))
			));
			assert!(matches!(
				execute(&mut store, &safety, Id(9), 0, Operation::LoadGameActivity),
				Outcome::GameActivity(Ok(true))
			));
			let mut unavailable = Err(StoreError::Unavailable);
			assert!(matches!(
				execute(
					&mut unavailable,
					&safety,
					Id(0),
					0,
					Operation::LoadGameActivity
				),
				Outcome::GameActivity(Err(StoreError::Unavailable))
			));
			assert!(matches!(
				execute(
					&mut unavailable,
					&safety,
					Id(0),
					0,
					Operation::SaveGameActivity(false)
				),
				Outcome::GameActivitySaved(Err(StoreError::Unavailable))
			));
		}

		{
			let safety = HistorySafety::default();
			let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
			let value = model::ReadingPreferences {
				zoom_percent: 125,
				sidebar_width: 300,
				show_members: false,
				show_members_dms: false,
				compact_messages: false,
				double_click_reaction_enabled: false,
				double_click_reaction: 0,
				animate_gifs: false,
				smooth_scrolling: true,
				scroll_speed_percent: 100,
				hide_media_links: true,
				confirm_external_links: true,
			};
			store
				.as_mut()
				.unwrap()
				.save_draft(Id(1), Id(2), "Synthetic draft")
				.unwrap();
			safety.block(); // History cleanup does not prohibit application settings.
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					Id(0),
					0,
					Operation::SaveReadingPreferences(value)
				),
				Outcome::ReadingPreferencesSaved(Ok(()))
			));
			assert!(matches!(execute(&mut store, &safety, Id(9), 0,
            Operation::LoadReadingPreferences), Outcome::ReadingPreferences(Ok(stored)) if stored == value));
			assert_eq!(
				store.as_ref().unwrap().load_drafts(Id(1)).unwrap()[&Id(2)],
				"Synthetic draft"
			);
			let mut unavailable = Err(StoreError::Unavailable);
			assert!(matches!(
				execute(
					&mut unavailable,
					&safety,
					Id(0),
					0,
					Operation::LoadReadingPreferences
				),
				Outcome::ReadingPreferences(Err(StoreError::Unavailable))
			));
			assert!(matches!(
				execute(
					&mut unavailable,
					&safety,
					Id(0),
					0,
					Operation::SaveReadingPreferences(value)
				),
				Outcome::ReadingPreferencesSaved(Err(StoreError::Unavailable))
			));
		}
	}

	#[test]
	fn cleanup_acknowledgements_cross_generations_without_reopening_early() {
		let safety = HistorySafety::default();
		let mut clears = HistoryClears::default();
		safety.block();
		assert!(clears.request(Id(1)));
		clears.queued(Id(1)); // Cleanup queued before logout.
		assert!(clears.request(Id(9))); // Another account needs cleanup after login.
		assert!(!clears.acknowledge(&safety)); // Old-generation completion still counts.
		assert!(!safety.allows(safety.epoch()));
		clears.queued(Id(9));
		assert!(clears.request(Id(9))); // A second deletion races that account's first clear.
		assert!(!clears.acknowledge(&safety));
		clears.queued(Id(9));
		assert!(clears.acknowledge(&safety));
		assert!(safety.allows(safety.epoch()));
		assert!(!clears.pending());
		safety.block();
		for account in 1..=16 {
			assert!(clears.request(Id(account)));
		}
		assert!(!clears.request(Id(17)));
		for account in 1..=16 {
			assert_eq!(clears.next(), Some(Id(account)));
			clears.queued(Id(account));
		}
		safety.fail();
		for _ in 0..16 {
			clears.acknowledge(&safety);
		}
		// Even a racing clear of the separate blocked atomic cannot override failure.
		safety.blocked.store(false, Ordering::SeqCst);
		assert!(!safety.allows(safety.epoch()));
	}

	#[test]
	fn deletion_epoch_rejects_queued_saves_and_returning_loads() {
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		let safety = HistorySafety::default();
		let account = Id(1);
		let channel = Id(2);
		let snapshot = vec![
			test_support::message(10, channel),
			test_support::message(11, channel),
		];
		for owner in [account, Id(9)] {
			assert!(matches!(
				execute(
					&mut store,
					&safety,
					owner,
					0,
					Operation::SaveChannel {
						channel,
						messages: snapshot.clone(),
						start: false,
					}
				),
				Outcome::Saved
			));
		}
		let loaded = execute(
			&mut store,
			&safety,
			account,
			0,
			Operation::LoadChannel {
				channel,
				request: 5,
			},
		);
		let Outcome::Channel {
			epoch, messages, ..
		} = loaded
		else {
			panic!()
		};
		assert_eq!(messages.len(), 2);
		let mut state = client_core::State {
			user: Some(test_support::message(1, channel).author),
			auth: client_core::auth::AuthState::Authenticated,
			gateway_connected: true,
			selected: Some(channel),
			freshness: model::Freshness::Fresh,
			channels: vec![model::Channel {
				id: channel,
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				icon: None,
				position: 0,
				recipients: vec![],
				last_message: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			..Default::default()
		};
		let mut reply = test_support::message(12, channel);
		reply.kind = 19;
		reply.reply_to = Some(Id(10));
		reply.reply_deleted = true;
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Message(reply),
		});
		let deletions = state.take_reply_deletions();
		assert_eq!(deletions, vec![(channel, Id(10))]);
		safety.invalidate();
		assert!(!safety.allows(epoch));
		assert!(matches!(
			execute(
				&mut store,
				&safety,
				account,
				0,
				Operation::LoadChannel {
					channel,
					request: 5
				}
			),
			Outcome::Saved
		));
		execute(
			&mut store,
			&safety,
			account,
			safety.epoch(),
			Operation::DeleteMessages {
				channel,
				ids: deletions.into_iter().map(|(_, id)| id).collect(),
			},
		);
		execute(
			&mut store,
			&safety,
			account,
			0,
			Operation::SaveChannel {
				channel,
				messages: snapshot,
				start: false,
			},
		);
		let current = store
			.as_ref()
			.unwrap()
			.load_channel(account, channel)
			.unwrap();
		assert_eq!(
			current.iter().map(|m| m.id).collect::<Vec<_>>(),
			vec![Id(11)]
		);
		assert_eq!(
			store
				.as_ref()
				.unwrap()
				.load_channel(Id(9), channel)
				.unwrap()
				.len(),
			2
		);
	}

	#[test]
	fn full_queue_blocks_history_until_scoped_cleanup_and_io_failure_stays_closed() {
		let (send, commands) = mpsc::sync_channel(16);
		let (_, receive) = mpsc::sync_channel(16);
		let cache = Cache {
			send,
			receive,
			budget: Arc::new(Budget::default()),
			history: Arc::new(HistorySafety::default()),
		};
		let account = Id(1);
		let channel = Id(2);
		for _ in 0..16 {
			assert!(cache.queue(0, account, Operation::LoadDrafts));
		}
		assert!(!cache.delete_messages(0, account, channel, vec![Id(10)]));
		assert!(!cache.history.allows(cache.history.epoch()));
		assert!(!cache.queue(
			0,
			account,
			Operation::LoadChannel {
				channel,
				request: 1
			}
		));
		assert!(!cache.queue(
			0,
			account,
			Operation::SaveChannel {
				channel,
				messages: vec![],
				start: false,
			}
		));
		for _ in 0..16 {
			commands.try_recv().unwrap();
		}
		assert!(cache.queue(1, account, Operation::ClearHistory));
		let (generation, owner, epoch, operation, _reservation) = commands.try_recv().unwrap();
		assert_eq!((generation, owner), (1, account));
		let mut store = Ok(LocalStore::open(std::path::Path::new(":memory:")).unwrap());
		for owner in [account, Id(9)] {
			store
				.as_mut()
				.unwrap()
				.save_channel(owner, channel, &[test_support::message(10, channel)], false)
				.unwrap();
			store
				.as_mut()
				.unwrap()
				.save_draft(owner, channel, "preserved draft")
				.unwrap();
		}
		assert!(matches!(
			execute(&mut store, &cache.history, owner, epoch, operation),
			Outcome::HistoryCleared
		));
		cache.history.cleared();
		assert!(cache.history.allows(cache.history.epoch()));
		assert!(
			store
				.as_ref()
				.unwrap()
				.load_channel(account, channel)
				.unwrap()
				.is_empty()
		);
		assert_eq!(
			store
				.as_ref()
				.unwrap()
				.load_channel(Id(9), channel)
				.unwrap()
				.len(),
			1
		);
		assert_eq!(
			store.as_ref().unwrap().load_drafts(account).unwrap()[&channel],
			"preserved draft"
		);
		let mut failed = Err(StoreError::Unavailable);
		assert!(matches!(
			execute(
				&mut failed,
				&cache.history,
				account,
				epoch,
				Operation::DeleteMessages {
					channel,
					ids: vec![Id(10)]
				}
			),
			Outcome::Failed { .. }
		));
		cache.history.cleared();
		assert!(!cache.history.allows(cache.history.epoch()));
		assert!(cache.queue(
			2,
			account,
			Operation::SaveDraft {
				channel,
				content: "still usable".into()
			}
		));
	}
}
