//! Private account overview and one serial connection editor. No disk or extension projection.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id,
	account_settings::{ConnectionChange, Connections, Overview},
};

#[derive(Default)]
pub struct Settings {
	pub overview: Option<Overview>,
	pub overview_pending: bool,
	pub overview_error: Option<&'static str>,
	pub connections: Option<Connections>,
	pub connections_pending: bool,
	pub connections_saving: bool,
	pub connections_error: Option<&'static str>,
	pub connections_reload_required: bool,
	pub connection_draft: Option<ConnectionChange>,
	user: Option<Id>,
	generation: u64,
	overview_request: u64,
	connections_request: u64,
}

impl Settings {
	fn reset(&mut self) {
		*self = Self {
			overview_request: self.overview_request.wrapping_add(1),
			connections_request: self.connections_request.wrapping_add(1),
			..Self::default()
		};
	}
}

impl State {
	pub(crate) fn retain_account_settings_session(&mut self, user: Id) {
		if self.account_settings.user != Some(user)
			|| self.account_settings.generation != self.generation
		{
			self.account_settings.reset();
		}
	}
	fn account_settings_scope(&mut self) -> Option<Id> {
		let user = self.user.as_ref()?.id;
		if self.account_settings.user != Some(user)
			|| self.account_settings.generation != self.generation
		{
			self.account_settings.reset();
			self.account_settings.user = Some(user);
			self.account_settings.generation = self.generation;
		}
		Some(user)
	}
	fn account_settings_connected(&self) -> bool {
		self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected)
	}
	pub fn request_account_overview(&mut self) -> Option<Command> {
		let user = self.account_settings_scope()?;
		if self.account_settings.overview_pending {
			return None;
		}
		if !self.account_settings_connected() {
			self.account_settings.overview_error = Some("Reconnect to load account details");
			return None;
		}
		self.account_settings.overview_error = None;
		if self.demo {
			self.account_settings.overview = Some(Overview {
				user: self.user.clone()?,
				username: "preview-user".into(),
				email: Some("preview@example.invalid".into()),
				phone: None,
				verified: Some(true),
				mfa_enabled: Some(false),
				premium_type: Some(self.premium_type),
			});
			return None;
		}
		self.account_settings.overview_request =
			self.account_settings.overview_request.wrapping_add(1);
		self.account_settings.overview_pending = true;
		Some(Command::AccountOverview {
			user,
			request: self.account_settings.overview_request,
		})
	}
	pub fn request_account_connections(&mut self) -> Option<Command> {
		self.account_connections_command(None)
	}
	pub fn update_account_connection(&mut self, change: ConnectionChange) -> Option<Command> {
		if self.account_settings.connections_reload_required
			|| !change.valid()
			|| self
				.account_settings
				.connections
				.as_ref()
				.and_then(|connections| connections.find(&change.key))
				.is_none_or(|connection| !change.supported(connection))
		{
			return None;
		}
		self.account_connections_command(Some(change))
	}
	fn account_connections_command(&mut self, change: Option<ConnectionChange>) -> Option<Command> {
		let user = self.account_settings_scope()?;
		if self.account_settings.connections_pending {
			return None;
		}
		if !self.account_settings_connected() {
			self.account_settings.connections_error = Some("Reconnect to load account connections");
			return None;
		}
		self.account_settings.connections_error = None;
		if self.demo {
			let connections = self.account_settings.connections.get_or_insert_with(|| {
				use model::account_settings::{Connection, ConnectionKey};
				Connections {
					items: ["spotify", "github"]
						.into_iter()
						.map(|kind| Connection {
							key: ConnectionKey {
								kind: kind.into(),
								id: format!("synthetic-{kind}"),
							},
							name: "Preview account".into(),
							verified: true,
							revoked: false,
							visibility: Some(1),
							metadata_visibility: Some(1),
							show_activity: Some(false),
							friend_sync: Some(false),
						})
						.collect(),
				}
			});
			if let Some(change) = change {
				use model::account_settings::ConnectionSetting as C;
				if change.setting == C::Remove {
					connections.items.retain(|item| item.key != change.key);
				} else if let Some(item) = connections
					.items
					.iter_mut()
					.find(|item| item.key == change.key)
				{
					match change.setting {
						C::DisplayOnProfile(value) => item.visibility = Some(u32::from(value)),
						C::DisplayMetadata(value) => {
							item.metadata_visibility = Some(u32::from(value))
						}
						C::DisplayActivity(value) => item.show_activity = Some(value),
						C::SyncFriends(value) => item.friend_sync = Some(value),
						C::Remove => {}
					}
				}
			}
			self.account_settings.connections_reload_required = false;
			self.account_settings.connection_draft = None;
			return None;
		}
		let settings = &mut self.account_settings;
		settings.connections_request = settings.connections_request.wrapping_add(1);
		settings.connections_pending = true;
		settings.connections_saving = change.is_some();
		if let Some(change) = &change {
			settings.connection_draft = Some(change.clone());
		}
		Some(Command::AccountConnections {
			user,
			request: settings.connections_request,
			change,
		})
	}
	pub fn account_connections_command_allowed(
		&self,
		user: Id,
		request: u64,
		change: &Option<ConnectionChange>,
	) -> bool {
		let settings = &self.account_settings;
		settings.user == Some(user)
			&& settings.generation == self.generation
			&& self.user.as_ref().is_some_and(|own| own.id == user)
			&& settings.connections_pending
			&& settings.connections_request == request
			&& self.account_settings_connected()
			&& change.as_ref().is_none_or(|change| {
				change.valid()
					&& settings
						.connections
						.as_ref()
						.and_then(|connections| connections.find(&change.key))
						.is_some_and(|connection| change.supported(connection))
			})
	}
	pub(crate) fn apply_account_overview(
		&mut self,
		user: Id,
		request: u64,
		result: Result<Box<Overview>, Failure>,
	) {
		let settings = &mut self.account_settings;
		if settings.user != Some(user)
			|| settings.generation != self.generation
			|| self.user.as_ref().is_none_or(|own| own.id != user)
			|| !settings.overview_pending
			|| settings.overview_request != request
		{
			return;
		}
		settings.overview_pending = false;
		match result {
			Ok(overview) if overview.user.id == user && overview.valid() => {
				settings.overview = Some(*overview);
				settings.overview_error = None;
			}
			result => {
				settings.overview_error = Some(if let Err(failure) = &result {
					failure.label()
				} else {
					"Account details response was invalid"
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
	pub(crate) fn apply_account_connections(
		&mut self,
		user: Id,
		request: u64,
		result: Result<Box<Connections>, Failure>,
	) {
		let settings = &mut self.account_settings;
		if settings.user != Some(user)
			|| settings.generation != self.generation
			|| self.user.as_ref().is_none_or(|own| own.id != user)
			|| !settings.connections_pending
			|| settings.connections_request != request
		{
			return;
		}
		settings.connections_pending = false;
		let saving = std::mem::take(&mut settings.connections_saving);
		match result {
			Ok(connections) if connections.valid() => {
				settings.connections = Some(*connections);
				settings.connections_error = None;
				settings.connections_reload_required = false;
				if saving {
					settings.connection_draft = None;
					self.profile_cache.forget(user);
					self.preload.profiles.touch(None);
					if self
						.profile
						.as_ref()
						.is_some_and(|profile| profile.user == user)
					{
						self.clear_profile();
					}
				}
			}
			result => {
				settings.connections_reload_required = true;
				settings.connections_error = Some(if saving {
					"Connection change was not confirmed; it may have applied. Reload before applying again"
				} else if let Err(failure) = &result {
					failure.label()
				} else {
					"Account connections response was invalid"
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
	pub(crate) fn interrupt_account_settings(&mut self) {
		let settings = &mut self.account_settings;
		if settings.user.is_some() {
			settings.overview = None;
			settings.connections = None;
			settings.overview_pending = false;
			settings.connections_pending = false;
			settings.connections_reload_required = true;
			settings.overview_request = settings.overview_request.wrapping_add(1);
			settings.connections_request = settings.connections_request.wrapping_add(1);
			settings.overview_error = Some("Reconnect and reload account details");
			settings.connections_error = Some(if settings.connections_saving {
				"Connection change interrupted; it may have applied. Reload to check"
			} else {
				"Reconnect and reload account connections"
			});
			settings.connections_saving = false;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::{
		User,
		account_settings::{Connection, ConnectionKey, ConnectionSetting},
	};
	fn state() -> State {
		State {
			user: Some(User {
				member_avatar: None,
				username: None,
				id: Id(1),
				name: "Synthetic".into(),
				avatar: None,
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
				kind: Default::default(),
				webhook: false,
			}),
			auth: AuthState::Authenticated,
			gateway_connected: true,
			..State::default()
		}
	}
	fn connections() -> Box<Connections> {
		Box::new(Connections {
			items: vec![Connection {
				key: ConnectionKey {
					kind: "github".into(),
					id: "synthetic".into(),
				},
				name: "Synthetic".into(),
				verified: true,
				revoked: false,
				visibility: Some(1),
				metadata_visibility: None,
				show_activity: Some(false),
				friend_sync: None,
			}],
		})
	}
	fn load(state: &mut State) {
		let Command::AccountConnections { user, request, .. } =
			state.request_account_connections().unwrap()
		else {
			panic!();
		};
		state.apply_account_connections(user, request, Ok(connections()));
	}
	#[test]
	fn connection_failures_keep_the_change_and_require_reload_before_explicit_retry() {
		let mut state = state();
		load(&mut state);
		let change = ConnectionChange {
			key: connections().items[0].key.clone(),
			setting: ConnectionSetting::DisplayOnProfile(false),
		};
		let command = state.update_account_connection(change.clone()).unwrap();
		let Command::AccountConnections {
			user,
			request,
			ref change,
		} = command
		else {
			panic!();
		};
		assert!(state.account_connections_command_allowed(user, request, change));
		assert!(
			state.request_account_connections().is_none(),
			"one outstanding mutation"
		);
		state.command_rejected(command);
		assert!(state.account_settings.connection_draft.is_some());
		assert!(state.account_settings.connections_reload_required);
		let draft = state.account_settings.connection_draft.clone().unwrap();
		assert!(state.update_account_connection(draft.clone()).is_none());
		load(&mut state);
		assert!(
			state.account_settings.connection_draft.is_some(),
			"reading does not repeat the change"
		);
		let Command::AccountConnections { user, request, .. } =
			state.update_account_connection(draft).unwrap()
		else {
			panic!();
		};
		let mut saved = connections();
		saved.items[0].visibility = Some(0);
		state.apply_account_connections(user, request, Ok(saved));
		assert!(state.account_settings.connection_draft.is_none());
		assert_eq!(
			state.account_settings.connections.as_ref().unwrap().items[0].visibility,
			Some(0)
		);
	}
	#[test]
	fn disconnect_and_account_switch_retire_private_results_and_unsupported_fields() {
		let mut state = state();
		load(&mut state);
		assert!(
			state
				.update_account_connection(ConnectionChange {
					key: connections().items[0].key.clone(),
					setting: ConnectionSetting::SyncFriends(true)
				})
				.is_none()
		);
		let Command::AccountOverview { user, request } = state.request_account_overview().unwrap()
		else {
			panic!();
		};
		let overview = Overview {
			user: state.user.clone().unwrap(),
			username: "synthetic".into(),
			email: Some("preview@example.invalid".into()),
			phone: None,
			verified: Some(true),
			mfa_enabled: Some(false),
			premium_type: None,
		};
		state.interrupt_account_settings();
		state.apply_account_overview(user, request, Ok(Box::new(overview)));
		assert!(state.account_settings.overview.is_none());
		assert!(state.account_settings.connections.is_none());
		let Command::AccountConnections { user, request, .. } =
			state.request_account_connections().unwrap()
		else {
			panic!();
		};
		state.logout();
		state.apply_account_connections(user, request, Ok(connections()));
		assert!(state.account_settings.connections.is_none());
		assert!(state.account_settings.connection_draft.is_none());
	}
	#[test]
	fn demo_account_workflows_do_not_emit_commands() {
		let mut state = state();
		state.demo = true;
		assert!(state.request_account_overview().is_none());
		assert!(state.account_settings.overview.is_some());
		assert!(state.request_account_connections().is_none());
		let key = state.account_settings.connections.as_ref().unwrap().items[0]
			.key
			.clone();
		assert!(
			state
				.update_account_connection(ConnectionChange {
					key: key.clone(),
					setting: ConnectionSetting::DisplayOnProfile(false)
				})
				.is_none()
		);
		assert_eq!(
			state
				.account_settings
				.connections
				.as_ref()
				.unwrap()
				.find(&key)
				.unwrap()
				.visibility,
			Some(0)
		);
		assert!(
			state
				.update_account_connection(ConnectionChange {
					key: key.clone(),
					setting: ConnectionSetting::Remove
				})
				.is_none()
		);
		assert!(
			state
				.account_settings
				.connections
				.as_ref()
				.unwrap()
				.find(&key)
				.is_none()
		);
	}
}
