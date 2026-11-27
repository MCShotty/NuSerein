//! Preference requests carry account or call ownership, never playback authority.
use super::Request;
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id,
	soundboard::{Favorite, Sound},
};

pub const MAX_SYNCED_FAVORITES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
	LoadFavorites,
	Favorite {
		key: Favorite,
		enabled: bool,
	},
	LoadEntrance,
	Entrance {
		before: Option<Favorite>,
		after: Option<Favorite>,
	},
	LoadGlobalEntrance,
	GlobalEntrance {
		before: Option<Favorite>,
		after: Option<Favorite>,
	},
	EntranceCatalog(Option<Id>),
}
impl SettingsAction {
	fn account(self) -> bool {
		matches!(
			self,
			Self::LoadGlobalEntrance | Self::GlobalEntrance { .. } | Self::EntranceCatalog(_)
		)
	}
	fn favorites(self) -> bool {
		matches!(self, Self::LoadFavorites | Self::Favorite { .. })
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsScope {
	Account(Id),
	Call { guild: Id, channel: Id, call: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsWork {
	pub request: u64,
	pub generation: u64,
	pub scope: SettingsScope,
	pub action: SettingsAction,
}
impl SettingsWork {
	pub fn call_owner(self) -> Option<(Id, u64)> {
		match self.scope {
			SettingsScope::Call { channel, call, .. } => Some((channel, call)),
			SettingsScope::Account(_) => None,
		}
	}
}

pub enum SettingsResult {
	Favorites(Vec<Id>),
	/// Selected setting and (for a server selection only) the global default.
	Entrance(Option<Favorite>, Option<Favorite>),
	Catalog(Vec<Sound>),
}
impl SettingsResult {
	pub(super) fn bytes(&self) -> usize {
		match self {
			Self::Favorites(ids) => ids.capacity() * size_of::<Id>(),
			Self::Entrance(..) => 0,
			Self::Catalog(sounds) => model::soundboard::sound_bytes(sounds),
		}
	}
}

#[derive(Default)]
pub struct Settings {
	pending: Option<SettingsWork>,
	favorites: Option<Vec<Id>>,
	entrance: Option<(Id, Option<Favorite>)>,
	global_entrance: Option<Option<Favorite>>,
	catalog: Option<(Option<Id>, Vec<Sound>)>,
	pub favorites_error: Option<&'static str>,
	pub entrance_error: Option<&'static str>,
	pub global_error: Option<&'static str>,
	pub catalog_error: Option<&'static str>,
}
impl Settings {
	pub(super) fn interrupt(&mut self) {
		if let Some(work) = self.pending.take() {
			self.fail(work.action, "voice-soundboard-settings-interrupted");
		}
		self.favorites = None;
		self.entrance = None;
		self.global_entrance = None;
		self.catalog = None;
	}
	pub(super) fn catalog_changed(&mut self, guild: Id) {
		if self
			.catalog
			.as_ref()
			.is_some_and(|(source, _)| *source == Some(guild))
		{
			self.catalog = None;
		}
		if let Some(work) = self
			.pending
			.filter(|work| work.action == SettingsAction::EntranceCatalog(Some(guild)))
		{
			self.pending = None;
			self.fail(work.action, "voice-soundboard-entrance-catalog-changed");
		}
	}
	fn fail(&mut self, action: SettingsAction, error: &'static str) {
		match action {
			SettingsAction::EntranceCatalog(_) => {
				self.catalog = None;
				self.catalog_error = Some(error);
			}
			SettingsAction::LoadGlobalEntrance | SettingsAction::GlobalEntrance { .. } => {
				self.global_entrance = None;
				self.global_error = Some(error);
			}
			_ if action.favorites() => {
				self.favorites = None;
				self.favorites_error = Some(error);
			}
			_ => {
				self.entrance = None;
				self.entrance_error = Some(error);
			}
		}
	}
	pub fn favorites(&self) -> Option<&[Id]> {
		self.favorites.as_deref()
	}
	pub fn entrance(&self, guild: Id) -> Option<Option<Favorite>> {
		self.entrance
			.filter(|(id, _)| *id == guild)
			.map(|(_, sound)| sound)
	}
	pub fn global_entrance(&self) -> Option<Option<Favorite>> {
		self.global_entrance
	}
	pub fn catalog(&self) -> Option<(Option<Id>, &[Sound])> {
		self.catalog
			.as_ref()
			.map(|(source, sounds)| (*source, sounds.as_slice()))
	}
	pub fn pending(&self) -> bool {
		self.pending.is_some()
	}
}

impl State {
	fn soundboard_settings_scope(&self) -> Option<SettingsScope> {
		let (channel, guild) = self.soundboard_channel()?;
		let active = self.voice.active.as_ref()?;
		(self.soundboard_settings_account().is_some()
			&& self.guild(guild).is_some()
			&& self.can_view(channel)
			&& active.reconnecting.is_none())
		.then_some(SettingsScope::Call {
			guild,
			channel,
			call: active.request,
		})
	}
	fn soundboard_settings_account(&self) -> Option<Id> {
		let account = self.user.as_ref()?.id;
		(!self.demo
			&& self.gateway_connected
			&& self.auth == AuthState::Authenticated
			&& account.0 != 0)
			.then_some(account)
	}
	pub fn global_entrance_sound_eligible(&self) -> bool {
		self.premium_type == 2 && self.soundboard_settings_account().is_some()
	}
	fn owns_soundboard_settings(&self, work: SettingsWork) -> bool {
		work.generation == self.generation
			&& match work.scope {
				SettingsScope::Call { .. } => {
					!work.action.account() && self.soundboard_settings_scope() == Some(work.scope)
				}
				SettingsScope::Account(account) => {
					work.action.account()
						&& self.soundboard_settings_account() == Some(account)
						&& self.global_entrance_sound_eligible()
						&& match work.action {
							SettingsAction::EntranceCatalog(Some(id)) => self.guild(id).is_some(),
							_ => true,
						}
				}
			}
	}
	pub fn entrance_sound_eligible(&self) -> bool {
		self.premium_type == 2
			&& matches!(self.soundboard_settings_scope(), Some(SettingsScope::Call { channel, .. }) if
				!self.is_stage(channel) && self.permission(channel, model::permissions::USE_SOUNDBOARD) == Some(true))
	}
	pub fn can_read_soundboard_settings(&self) -> bool {
		self.soundboard_settings_scope().is_some()
	}
	pub fn can_change_synced_soundboard_favorite(&self, key: Favorite, enabled: bool) -> bool {
		!self.soundboard.settings.pending()
			&& self.soundboard_settings_scope().is_some_and(|scope| {
				self.soundboard_settings_action_allowed(
					scope,
					SettingsAction::Favorite { key, enabled },
				)
			})
	}

	fn settings_catalog_sound(&self, guild: Id, key: Favorite) -> Option<&Sound> {
		let (source, id) = key;
		let board = &self.soundboard;
		let catalog = match source {
			None => &board.default,
			Some(source) if source == guild => {
				&board.guild.as_ref().filter(|(id, _)| *id == guild)?.1
			}
			Some(source) => return self.external_sound(source, id),
		};
		catalog.loaded.then_some(())?;
		catalog
			.sounds
			.iter()
			.find(|sound| sound.available && (sound.guild, sound.id) == key)
	}
	fn soundboard_settings_action_allowed(
		&self,
		scope: SettingsScope,
		action: SettingsAction,
	) -> bool {
		let settings = &self.soundboard.settings;
		let SettingsScope::Call { guild, .. } = scope else {
			return self.global_entrance_sound_eligible()
				&& match action {
					SettingsAction::LoadGlobalEntrance => true,
					SettingsAction::EntranceCatalog(source) => {
						source.is_none_or(|id| id.0 != 0 && self.guild(id).is_some())
					}
					SettingsAction::GlobalEntrance { before, after } => {
						settings.global_entrance == Some(before)
							&& before != after && after.is_none_or(|key| {
							key.0.is_none_or(|id| self.guild(id).is_some())
								&& settings.catalog().is_some_and(|(source, sounds)| {
									source == key.0
										&& sounds.iter().any(|sound| {
											sound.available && (sound.guild, sound.id) == key
										})
								})
						})
					}
					_ => false,
				};
		};
		match action {
			SettingsAction::LoadFavorites => true,
			SettingsAction::Favorite { key, enabled } => {
				settings.favorites().is_some_and(|ids| {
					ids.contains(&key.1) != enabled
						&& (!enabled || ids.len() < MAX_SYNCED_FAVORITES)
				}) && self.settings_catalog_sound(guild, key).is_some()
			}
			SettingsAction::LoadEntrance => self.entrance_sound_eligible(),
			SettingsAction::Entrance { before, after } => {
				self.entrance_sound_eligible()
					&& settings.entrance(guild) == Some(before)
					&& before != after
					&& after.is_none_or(|key| self.settings_catalog_sound(guild, key).is_some())
			}
			_ => false,
		}
	}
	pub fn request_soundboard_settings(&mut self, action: SettingsAction) -> Option<Command> {
		self.retire_soundboard_settings();
		let scope = if action.account() {
			SettingsScope::Account(self.soundboard_settings_account()?)
		} else {
			self.soundboard_settings_scope()?
		};
		if self.soundboard.settings.pending()
			|| !self.soundboard_settings_action_allowed(scope, action)
		{
			return None;
		}
		let work = SettingsWork {
			request: self.soundboard.next_request(),
			generation: self.generation,
			scope,
			action,
		};
		let settings = &mut self.soundboard.settings;
		match action {
			SettingsAction::EntranceCatalog(_) => {
				settings.catalog = None;
				settings.catalog_error = None;
			}
			SettingsAction::LoadGlobalEntrance | SettingsAction::GlobalEntrance { .. } => {
				settings.global_error = None
			}
			_ if action.favorites() => settings.favorites_error = None,
			_ => settings.entrance_error = None,
		}
		settings.pending = Some(work);
		Some(Command::Soundboard(Request::Settings(work)))
	}
	pub(super) fn soundboard_settings_allowed(&self, work: SettingsWork) -> bool {
		self.soundboard.settings.pending == Some(work)
			&& self.owns_soundboard_settings(work)
			&& self.soundboard_settings_action_allowed(work.scope, work.action)
	}
	pub fn retire_soundboard_settings(&mut self) {
		if let Some(work) = self
			.soundboard
			.settings
			.pending
			.filter(|work| !self.owns_soundboard_settings(*work))
		{
			self.soundboard_settings_rejected(work);
		}
	}
	pub(super) fn soundboard_settings_rejected(&mut self, work: SettingsWork) {
		if self.soundboard.settings.pending == Some(work) {
			self.soundboard.settings.pending = None;
			self.soundboard
				.settings
				.fail(work.action, "voice-soundboard-settings-interrupted");
		}
	}
	pub(super) fn apply_soundboard_settings(
		&mut self,
		work: SettingsWork,
		result: Result<SettingsResult, Failure>,
	) {
		if self.soundboard.settings.pending != Some(work) {
			return;
		}
		if !self.owns_soundboard_settings(work)
			|| (!work.action.account()
				&& !work.action.favorites()
				&& !self.entrance_sound_eligible())
		{
			self.soundboard_settings_rejected(work);
			return;
		}
		let settings = &mut self.soundboard.settings;
		settings.pending = None;
		let mismatch = "voice-soundboard-settings-unconfirmed";
		match (work.action, result) {
			(action, Ok(SettingsResult::Favorites(ids)))
				if action.favorites()
					&& ids.len() <= MAX_SYNCED_FAVORITES
					&& ids
						.iter()
						.enumerate()
						.all(|(i, id)| id.0 != 0 && !ids[..i].contains(id)) =>
			{
				settings.favorites = Some(ids);
			}
			(
				SettingsAction::LoadGlobalEntrance | SettingsAction::GlobalEntrance { .. },
				Ok(SettingsResult::Entrance(sound, None)),
			) if valid_sound(sound) => {
				settings.global_entrance = Some(sound);
				settings.global_error = None;
			}
			(
				SettingsAction::LoadEntrance | SettingsAction::Entrance { .. },
				Ok(SettingsResult::Entrance(sound, fallback)),
			) if valid_sound(sound) && valid_sound(fallback) => {
				if let SettingsScope::Call { guild, .. } = work.scope {
					settings.entrance = Some((guild, sound));
					settings.global_entrance = Some(fallback);
					settings.global_error = None;
				}
			}
			(SettingsAction::EntranceCatalog(source), Ok(SettingsResult::Catalog(sounds)))
				if model::soundboard::valid_sounds(&sounds)
					&& sounds.iter().all(|sound| sound.guild == source) =>
			{
				settings.catalog = Some((source, sounds));
			}
			(action, Err(error)) => settings.fail(action, error.label()),
			(action, Ok(_)) => settings.fail(action, mismatch),
		}
	}
}
fn valid_sound(sound: Option<Favorite>) -> bool {
	sound.is_none_or(|(source, id)| id.0 != 0 && source.is_none_or(|id| id.0 != 0))
}
