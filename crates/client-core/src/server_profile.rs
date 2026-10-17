//! One own-server-profile editor, scoped to account generation, guild and request.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id, Patch, permissions,
	server_profile::{Edit, Snapshot},
};

pub struct Event {
	pub user: Id,
	pub guild: Id,
	pub request: u64,
	pub result: Result<Box<Snapshot>, Failure>,
}

#[derive(Default)]
pub struct Editor {
	pub guild: Option<Id>,
	pub data: Option<Snapshot>,
	pub pending: bool,
	pub saving: bool,
	pub reload_required: bool,
	pub error: Option<&'static str>,
	pub request: u64,
	pub saved: u64,
	generation: u64,
	user: Option<Id>,
}
impl Editor {
	pub(crate) fn reset(&mut self) {
		*self = Self {
			request: self.request.wrapping_add(1),
			..Self::default()
		};
	}
}

impl State {
	pub fn can_edit_server_profile(&self, guild: Id) -> bool {
		self.user.as_ref().is_some_and(|user| {
			self.guild(guild).is_some()
				&& self
					.permissions
					.guilds
					.get(&guild)
					.is_some_and(|permissions| {
						permissions.member.is_some() || permissions.owner == Some(user.id)
					})
		})
	}
	pub fn can_change_own_nickname(&self, guild: Id) -> bool {
		let Some(user) = self.user.as_ref() else {
			return false;
		};
		self.can_edit_server_profile(guild)
			&& self
				.permissions
				.guilds
				.get(&guild)
				.and_then(|permissions| {
					self.permissions.effective(
						guild,
						permissions,
						user.id,
						Some(&[]),
						Self::permission_time(),
					)
				})
				.is_some_and(|bits| bits & permissions::CHANGE_NICKNAME != 0)
	}
	pub fn load_server_profile(&mut self, guild: Id) -> Option<Command> {
		if self.own_server_profile.saving
			|| !self.can_edit_server_profile(guild)
			|| (!self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected))
		{
			return None;
		}
		if self.own_server_profile.guild == Some(guild) && self.own_server_profile.pending {
			return None;
		}
		let user = self.user.as_ref()?.id;
		if self.own_server_profile.guild != Some(guild)
			|| self.own_server_profile.generation != self.generation
		{
			self.own_server_profile.reset();
		}
		let editor = &mut self.own_server_profile;
		editor.guild = Some(guild);
		editor.user = Some(user);
		editor.generation = self.generation;
		editor.request = editor.request.wrapping_add(1);
		editor.pending = true;
		editor.reload_required = true;
		editor.error = None;
		Some(Command::ServerProfile {
			user,
			guild,
			request: editor.request,
			edit: None,
		})
	}
	pub fn save_server_profile(&mut self, edit: Edit) -> Option<Command> {
		let guild = self.own_server_profile.guild?;
		if !self.can_save_server_profile(&edit) {
			return None;
		}
		let user = self.user.as_ref()?.id;
		let editor = &mut self.own_server_profile;
		editor.request = editor.request.wrapping_add(1);
		editor.pending = true;
		editor.saving = true;
		editor.error = None;
		Some(Command::ServerProfile {
			user,
			guild,
			request: editor.request,
			edit: Some(Box::new(edit)),
		})
	}
	pub fn can_save_server_profile(&self, edit: &Edit) -> bool {
		self.own_server_profile.guild.is_some_and(|guild| {
			!self.own_server_profile.pending
				&& !self.own_server_profile.reload_required
				&& !self.owned_cosmetics.saving
				&& self.own_server_profile.data.is_some()
				&& !edit.is_empty()
				&& self.server_profile_edit_allowed(guild, edit)
		})
	}
	fn server_profile_edit_allowed(&self, guild: Id, edit: &Edit) -> bool {
		self.can_edit_server_profile(guild)
			&& (self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
			&& edit.valid()
			&& (matches!(edit.nick, Patch::Absent) || self.can_change_own_nickname(guild))
			&& (!edit.needs_nitro() || self.premium_type == 2)
	}
	/// Dispatch rechecks current membership, permissions, entitlement and request identity.
	pub fn server_profile_command_allowed(
		&self,
		user: Id,
		guild: Id,
		request: u64,
		edit: &Option<Box<Edit>>,
	) -> bool {
		let editor = &self.own_server_profile;
		editor.pending
			&& editor.guild == Some(guild)
			&& editor.user == Some(user)
			&& editor.generation == self.generation
			&& editor.request == request
			&& self.user.as_ref().is_some_and(|own| own.id == user)
			&& self.can_edit_server_profile(guild)
			&& (self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
			&& edit
				.as_ref()
				.is_none_or(|edit| self.server_profile_edit_allowed(guild, edit))
	}
	pub fn close_server_profile(&mut self) {
		if !self.own_server_profile.saving {
			self.own_server_profile.reset();
		}
	}
	pub(crate) fn interrupt_server_profile(&mut self) {
		let editor = &mut self.own_server_profile;
		if editor.guild.is_some() {
			editor.error = Some(if editor.saving {
				"Server profile save interrupted; some changes may have applied. Reload before saving again"
			} else {
				"Reconnect and reload your server profile"
			});
			editor.request = editor.request.wrapping_add(1);
			editor.pending = false;
			editor.saving = false;
			editor.reload_required = true;
		}
	}
	pub(crate) fn apply_server_profile(&mut self, event: Event) {
		let editor = &self.own_server_profile;
		if !editor.pending
			|| editor.guild != Some(event.guild)
			|| editor.user != Some(event.user)
			|| editor.generation != self.generation
			|| editor.request != event.request
			|| self.user.as_ref().is_none_or(|own| own.id != event.user)
		{
			return;
		}
		if !self.can_edit_server_profile(event.guild) {
			self.own_server_profile.reset();
			return;
		}
		let saving = self.own_server_profile.saving;
		self.own_server_profile.pending = false;
		self.own_server_profile.saving = false;
		match event.result {
			Ok(snapshot)
				if snapshot.profile.user.id == event.user
					&& snapshot.guild() == Some(event.guild)
					&& snapshot.valid() =>
			{
				if saving {
					self.profile_cache.forget(event.user);
					self.preload.profiles.touch(None);
					if self.profile.as_ref().is_some_and(|view| {
						view.user == event.user && view.guild == Some(event.guild)
					}) {
						self.clear_profile();
					}
					// The Gateway owns member-row reconciliation. Invalidate loaded rows so
					// a stale server avatar/nickname is not presented as the saved identity.
					if self
						.members
						.as_ref()
						.is_some_and(|members| members.guild == Some(event.guild))
					{
						self.close_members();
					}
					self.own_server_profile.saved = self.own_server_profile.saved.wrapping_add(1);
				}
				self.own_server_profile.reload_required = false;
				self.own_server_profile.error = None;
				self.own_server_profile.data = Some(*snapshot);
			}
			result => {
				self.own_server_profile.reload_required = true;
				self.own_server_profile.error = Some(if saving {
					"Server profile save was not confirmed; some changes may have applied. Reload before saving again"
				} else if let Err(failure) = &result {
					failure.label()
				} else {
					"Server profile response was incomplete or invalid; reload to continue"
				});
				if let Err(failure) = result
					&& failure.ends_session()
					&& failure != Failure::Capacity
				{
					self.fail(failure);
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Envelope, Event as CoreEvent};
	use model::{GuildProfile, User, UserProfile, server_profile::Values};
	fn state() -> State {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			user: Some(User {
				id: Id(1),
				name: "Synthetic".into(),
				avatar: None,
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
				kind: Default::default(),
				webhook: false,
			}),
			guilds: vec![model::Guild {
				id: Id(2),
				name: "Synthetic server".into(),
				icon: None,
				emojis: None,
				stickers: None,
				default_message_notifications: None,
			}],
			..State::default()
		};
		state.permissions.guilds.insert(
			Id(2),
			permissions::Guild {
				id: Id(2),
				owner: Some(Id(1)),
				roles: Some(vec![]),
				member: Some(permissions::Member {
					roles: vec![],
					timeout_until: None,
				}),
			},
		);
		state
	}
	fn snapshot(state: &State) -> Box<Snapshot> {
		Box::new(Snapshot {
			profile: UserProfile {
				user: state.user.clone().unwrap(),
				username: "synthetic".into(),
				global_name: Some("Synthetic".into()),
				bio: "Global bio".into(),
				pronouns: String::new(),
				banner: None,
				accent_color: None,
				theme_colors: None,
				clan: None,
				board: None,
				badges: vec![],
				connections: vec![],
				mutual_guilds: vec![],
				mutual_friends: vec![],
				guild: Some(GuildProfile {
					guild: Id(2),
					roles: vec![],
					nick: None,
					avatar: None,
					banner: None,
					bio: String::new(),
					pronouns: String::new(),
					joined_at: None,
				}),
				effect_sku: None,
				frame_sku: None,
				effect: None,
				frame: None,
				badge_catalog: None,
				limited: false,
				private: false,
			},
			values: Values::default(),
		})
	}
	fn finish(state: &mut State, command: Command, result: Result<Box<Snapshot>, Failure>) {
		let Command::ServerProfile {
			user,
			guild,
			request,
			..
		} = command
		else {
			panic!("wrong command");
		};
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::ServerProfile(Event {
				user,
				guild,
				request,
				result,
			}),
		});
	}
	fn load(state: &mut State) {
		let command = state.load_server_profile(Id(2)).unwrap();
		let snapshot = snapshot(state);
		finish(state, command, Ok(snapshot));
	}
	#[test]
	fn nickname_permission_and_premium_are_rechecked_before_dispatch() {
		let mut state = state();
		load(&mut state);
		assert!(state.own_server_profile.data.is_some());
		assert!(
			state
				.save_server_profile(Edit {
					bio: Patch::Value("Nitro bio".into()),
					..Edit::default()
				})
				.is_none()
		);
		let command = state
			.save_server_profile(Edit {
				nick: Patch::Value("Server name".into()),
				..Edit::default()
			})
			.unwrap();
		let Command::ServerProfile {
			user,
			guild,
			request,
			ref edit,
		} = command
		else {
			panic!();
		};
		assert!(state.server_profile_command_allowed(user, guild, request, edit));
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(9));
		state.permissions.clear_cache();
		assert!(!state.server_profile_command_allowed(user, guild, request, edit));
		state.command_rejected(command);
		assert!(state.own_server_profile.reload_required);
		assert!(state.own_server_profile.data.is_some());
		load(&mut state);
		state.premium_type = 2;
		let command = state
			.save_server_profile(Edit {
				bio: Patch::Value("Nitro bio".into()),
				..Edit::default()
			})
			.unwrap();
		let Command::ServerProfile {
			user,
			guild,
			request,
			ref edit,
		} = command
		else {
			panic!();
		};
		state.premium_type = 0;
		assert!(!state.server_profile_command_allowed(user, guild, request, edit));
		state.command_rejected(command);
		load(&mut state);
		assert!(
			state
				.save_server_profile(Edit {
					bio: Patch::Null,
					avatar: Patch::Null,
					..Edit::default()
				})
				.is_some()
		);
	}
	#[test]
	fn interrupted_or_uncertain_saves_keep_snapshot_require_reload_and_retire_late_replies() {
		let mut state = state();
		load(&mut state);
		let edit = || Edit {
			pronouns: Patch::Value("they/them".into()),
			..Edit::default()
		};
		let command = state.save_server_profile(edit()).unwrap();
		assert!(state.load_server_profile(Id(2)).is_none());
		state.close_server_profile();
		assert!(state.own_server_profile.saving);
		finish(&mut state, command, Err(Failure::Ambiguous));
		assert!(state.own_server_profile.data.is_some());
		assert!(state.own_server_profile.reload_required);
		assert!(state.save_server_profile(edit()).is_none());
		load(&mut state);
		let command = state.save_server_profile(edit()).unwrap();
		let saved = snapshot(&state);
		state.apply(Envelope {
			generation: state.generation,
			event: CoreEvent::Disconnected,
		});
		finish(&mut state, command, Ok(saved));
		assert!(state.own_server_profile.reload_required);
		assert_eq!(state.own_server_profile.saved, 0);
		assert!(state.own_server_profile.error.is_some());
		state.logout();
		assert!(state.own_server_profile.data.is_none());
	}
	#[test]
	fn wrong_guild_or_user_response_never_replaces_own_profile_or_server_snapshot() {
		let mut state = state();
		let command = state.load_server_profile(Id(2)).unwrap();
		let mut wrong = snapshot(&state);
		wrong.profile.guild.as_mut().unwrap().guild = Id(3);
		finish(&mut state, command, Ok(wrong));
		assert!(state.own_server_profile.data.is_none());
		assert!(state.own_profile.data.is_none());
		let command = state.load_server_profile(Id(2)).unwrap();
		let mut wrong = snapshot(&state);
		wrong.profile.user.id = Id(9);
		finish(&mut state, command, Ok(wrong));
		assert!(state.own_server_profile.data.is_none());
		load(&mut state);
		let command = state
			.save_server_profile(Edit {
				pronouns: Patch::Null,
				..Edit::default()
			})
			.unwrap();
		let saved = snapshot(&state);
		state.remove_server(Id(2));
		finish(&mut state, command, Ok(saved));
		assert!(state.own_server_profile.data.is_none());
	}
}
