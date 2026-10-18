//! A single explicit owned-collectible equip, scoped to the current own profile.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id,
	profile_cosmetics::{Change, Snapshot},
};

pub struct Event {
	pub user: Id,
	pub guild: Option<Id>,
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
	pub draft: Option<Change>,
	pub request: u64,
	user: Option<Id>,
	generation: u64,
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
	pub(crate) fn retain_owned_cosmetic_session(&mut self, user: Id) {
		if self.owned_cosmetics.user != Some(user)
			|| self.owned_cosmetics.generation != self.generation
		{
			self.owned_cosmetics.reset();
		}
	}
	pub fn load_owned_cosmetics(&mut self, guild: Option<Id>) -> Option<Command> {
		if self.owned_cosmetics.saving {
			return None;
		}
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			self.owned_cosmetics.error = Some("Reconnect and reload owned cosmetics");
			return None;
		}
		if !self.owned_cosmetics_scope_allowed(guild) {
			return None;
		}
		let user = self.user.as_ref()?.id;
		if self.owned_cosmetics.user != Some(user)
			|| self.owned_cosmetics.generation != self.generation
			|| self.owned_cosmetics.guild != guild
		{
			self.owned_cosmetics.reset();
		}
		if self.owned_cosmetics.pending {
			return None;
		}
		self.owned_cosmetics.user = Some(user);
		self.owned_cosmetics.generation = self.generation;
		self.owned_cosmetics.guild = guild;
		self.owned_cosmetics.error = None;
		self.owned_cosmetics.pending = true;
		self.owned_cosmetics.reload_required = true;
		self.owned_cosmetics.request = self.owned_cosmetics.request.wrapping_add(1);
		Some(Command::OwnedCosmetics {
			user,
			guild,
			request: self.owned_cosmetics.request,
			change: None,
			nitro: self.premium_type == 2,
		})
	}
	fn owned_cosmetics_scope_allowed(&self, guild: Option<Id>) -> bool {
		self.user.is_some()
			&& guild.is_none_or(|guild| self.can_edit_server_profile(guild))
			&& (self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
	}
	pub fn can_equip_owned_cosmetic(&self, change: Change) -> bool {
		let editor = &self.owned_cosmetics;
		!editor.pending
			&& !editor.reload_required
			&& !self.own_profile.saving
			&& !self.own_server_profile.saving
			&& editor.generation == self.generation
			&& editor.user == self.user.as_ref().map(|user| user.id)
			&& self.owned_cosmetics_scope_allowed(editor.guild)
			&& editor.data.as_ref().is_some_and(|data| {
				change.allowed(
					data,
					self.premium_type == 2,
					Self::permission_time().max(0) as u64,
				)
			})
	}
	pub fn equip_owned_cosmetic(&mut self, change: Change) -> Option<Command> {
		if !self.can_equip_owned_cosmetic(change) {
			return None;
		}
		let editor = &mut self.owned_cosmetics;
		editor.pending = true;
		editor.saving = true;
		editor.error = None;
		editor.draft = Some(change);
		editor.request = editor.request.wrapping_add(1);
		Some(Command::OwnedCosmetics {
			user: editor.user?,
			guild: editor.guild,
			request: editor.request,
			change: Some(change),
			nitro: self.premium_type == 2,
		})
	}
	pub fn owned_cosmetics_command_allowed(
		&self,
		user: Id,
		guild: Option<Id>,
		request: u64,
		change: Option<Change>,
		nitro: bool,
	) -> bool {
		let editor = &self.owned_cosmetics;
		editor.user == Some(user)
			&& editor.generation == self.generation
			&& editor.guild == guild
			&& editor.pending
			&& editor.request == request
			&& self.user.as_ref().is_some_and(|own| own.id == user)
			&& self.owned_cosmetics_scope_allowed(guild)
			&& change.is_none_or(|change| {
				editor.saving
					&& editor.draft == Some(change)
					&& nitro == (self.premium_type == 2)
					&& editor.data.as_ref().is_some_and(|data| {
						change.allowed(data, nitro, Self::permission_time().max(0) as u64)
					})
			})
	}
	pub fn close_owned_cosmetics(&mut self) {
		if !self.owned_cosmetics.saving {
			self.owned_cosmetics.reset();
		}
	}
	pub fn discard_owned_cosmetic_draft(&mut self) {
		if !self.owned_cosmetics.pending {
			self.owned_cosmetics.draft = None;
		}
	}
	pub(crate) fn interrupt_owned_cosmetics(&mut self) {
		let editor = &mut self.owned_cosmetics;
		if editor.user.is_some() {
			editor.error = Some(if editor.saving {
				"Equip was not confirmed. Reload before applying again"
			} else {
				"Reconnect and reload owned cosmetics"
			});
			editor.pending = false;
			editor.saving = false;
			editor.reload_required = true;
			editor.request = editor.request.wrapping_add(1);
			// Retire account metadata on disconnect; the small explicit draft is kept in RAM.
			editor.data = None;
		}
	}
	pub(crate) fn apply_owned_cosmetics(&mut self, event: Event) {
		let editor = &self.owned_cosmetics;
		if !editor.pending
			|| editor.user != Some(event.user)
			|| editor.guild != event.guild
			|| editor.request != event.request
			|| editor.generation != self.generation
			|| self.user.as_ref().is_none_or(|own| own.id != event.user)
		{
			return;
		}
		if !self.owned_cosmetics_scope_allowed(event.guild) {
			self.owned_cosmetics.reset();
			return;
		}
		let saving = self.owned_cosmetics.saving;
		self.owned_cosmetics.pending = false;
		self.owned_cosmetics.saving = false;
		match event.result {
			Ok(snapshot)
				if snapshot.valid()
					&& snapshot.current.user == event.user
					&& snapshot.current.guild == event.guild =>
			{
				self.owned_cosmetics.data = Some(*snapshot);
				self.owned_cosmetics.error = None;
				self.owned_cosmetics.reload_required = false;
				if saving {
					self.owned_cosmetics.draft = None;
					self.profile_cache.forget(event.user);
					self.preload.profiles.touch(None);
					if self
						.profile
						.as_ref()
						.is_some_and(|view| view.user == event.user)
					{
						self.clear_profile();
					}
					if event.guild.is_none() {
						self.clear_own_profile();
					} else {
						self.own_server_profile.reset();
					}
				}
			}
			result => {
				self.owned_cosmetics.reload_required = true;
				self.owned_cosmetics.error = Some(if saving {
					"Equip was not confirmed; it may have applied. Reload before applying again"
				} else if let Err(failure) = &result {
					failure.label()
				} else {
					"Owned cosmetic response was invalid"
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
	use model::{
		User,
		profile_cosmetics::{Current, Inventory, Item, Kind},
	};
	fn state() -> State {
		State {
			user: Some(User {
				id: Id(1),
				name: "Synthetic".into(),
				kind: Default::default(),
				webhook: false,
				avatar: None,
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
			}),
			demo: true,
			premium_type: 2,
			..State::default()
		}
	}
	fn snapshot() -> Snapshot {
		Snapshot {
			inventory: Inventory {
				items: vec![Item {
					sku: Id(8),
					kind: Kind::Frame,
					name: "Synthetic".into(),
					expires_at: None,
					subscription: true,
				}],
				unsupported: false,
			},
			current: Current {
				user: Id(1),
				guild: None,
				premium_type: Some(2),
				decoration: None,
				nameplate: None,
				profile: Vec::new(),
			},
		}
	}
	fn load(state: &mut State) {
		let Command::OwnedCosmetics {
			user,
			guild,
			request,
			..
		} = state.load_owned_cosmetics(None).unwrap()
		else {
			panic!()
		};
		state.apply_owned_cosmetics(Event {
			user,
			guild,
			request,
			result: Ok(Box::new(snapshot())),
		});
	}
	#[test]
	fn failed_equips_keep_explicit_drafts_and_retire_stale_session_replies() {
		let mut state = state();
		load(&mut state);
		let change = Change {
			kind: Kind::Frame,
			sku: Some(Id(8)),
		};
		let Command::OwnedCosmetics {
			user,
			guild,
			request,
			..
		} = state.equip_owned_cosmetic(change).unwrap()
		else {
			panic!()
		};
		state.apply_owned_cosmetics(Event {
			user,
			guild,
			request,
			result: Err(Failure::Ambiguous),
		});
		assert_eq!(state.owned_cosmetics.draft, Some(change));
		assert!(state.equip_owned_cosmetic(change).is_none());
		load(&mut state);
		assert_eq!(state.owned_cosmetics.draft, Some(change));
		assert!(state.equip_owned_cosmetic(change).is_some());
		state.interrupt_owned_cosmetics();
		assert!(state.owned_cosmetics.data.is_none());
		state.retain_owned_cosmetic_session(Id(1));
		assert_eq!(state.owned_cosmetics.draft, Some(change));
		state.apply_owned_cosmetics(Event {
			user,
			guild,
			request,
			result: Ok(Box::new(snapshot())),
		});
		assert!(state.owned_cosmetics.data.is_none());
		state.retain_owned_cosmetic_session(Id(2));
		assert!(state.owned_cosmetics.draft.is_none());
	}
	#[test]
	fn entitlement_revocation_blocks_dispatch_and_unknown_or_expired_ownership() {
		let mut state = state();
		load(&mut state);
		let change = Change {
			kind: Kind::Frame,
			sku: Some(Id(8)),
		};
		let Command::OwnedCosmetics {
			user,
			guild,
			request,
			nitro,
			change,
		} = state.equip_owned_cosmetic(change).unwrap()
		else {
			panic!()
		};
		assert!(state.owned_cosmetics_command_allowed(user, guild, request, change, nitro));
		state.premium_type = 0;
		assert!(!state.owned_cosmetics_command_allowed(user, guild, request, change, nitro));
		state.interrupt_owned_cosmetics();
		load(&mut state);
		assert!(!state.can_equip_owned_cosmetic(change.unwrap()));
		assert!(state.can_equip_owned_cosmetic(Change {
			kind: Kind::Frame,
			sku: None
		}));
		state.demo = false;
		state.gateway_connected = false;
		assert!(state.load_owned_cosmetics(None).is_none());
		assert!(state.owned_cosmetics.error.is_some());
	}
}
