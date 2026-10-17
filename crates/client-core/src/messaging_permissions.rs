//! Account messaging preferences are shown only after Discord acknowledges them.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::messaging_permissions::{Change, Snapshot};

#[derive(Default)]
pub struct Settings {
	pub snapshot: Option<Snapshot>,
	pub pending: bool,
	pub error: Option<Failure>,
	/// A failed explicit change stays available for review after a fresh read.
	pub draft: Option<Change>,
	pub reload_required: bool,
	writing: bool,
	request: u64,
}

impl State {
	pub fn request_messaging_permissions(&mut self) -> Option<Command> {
		self.messaging_permissions_command(None)
	}

	pub fn update_messaging_permissions(&mut self, change: Change) -> Option<Command> {
		if !change.valid()
			|| self.messaging_permissions.snapshot.is_none()
			|| self.messaging_permissions.reload_required
		{
			return None;
		}
		self.messaging_permissions_command(Some(change))
	}

	fn messaging_permissions_command(&mut self, change: Option<Change>) -> Option<Command> {
		if self.messaging_permissions.pending {
			return None;
		}
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			self.messaging_permissions.error = Some(Failure::ProtocolAt(
				"Connect to Discord to update messaging permissions",
			));
			return None;
		}
		self.messaging_permissions.error = None;
		if self.demo {
			let mut snapshot = self
				.messaging_permissions
				.snapshot
				.clone()
				.unwrap_or_default();
			if let Some(change) = change {
				change.apply(&mut snapshot);
			}
			if snapshot.valid() {
				self.messaging_permissions.snapshot = Some(snapshot);
				self.messaging_permissions.reload_required = false;
				self.messaging_permissions.draft = None;
			} else {
				self.messaging_permissions.error = Some(Failure::Capacity);
			}
			return None;
		}
		self.messaging_permissions.request = self.messaging_permissions.request.wrapping_add(1);
		self.messaging_permissions.pending = true;
		self.messaging_permissions.writing = change.is_some();
		if let Some(change) = &change {
			self.messaging_permissions.draft = Some(change.clone());
		}
		Some(Command::MessagingPermissions {
			request: self.messaging_permissions.request,
			change,
		})
	}

	pub fn apply_messaging_permissions(&mut self, request: u64, result: Result<Snapshot, Failure>) {
		if !self.messaging_permissions.pending || request != self.messaging_permissions.request {
			return;
		}
		self.messaging_permissions.pending = false;
		let writing = std::mem::take(&mut self.messaging_permissions.writing);
		let result = result.and_then(|snapshot| {
			if snapshot.valid() {
				Ok(snapshot)
			} else {
				Err(Failure::Capacity)
			}
		});
		match result {
			Ok(snapshot) => {
				self.messaging_permissions.snapshot = Some(snapshot);
				self.messaging_permissions.error = None;
				self.messaging_permissions.reload_required = false;
				if writing {
					self.messaging_permissions.draft = None;
				}
			}
			Err(failure) => {
				self.messaging_permissions.error = Some(failure);
				self.messaging_permissions.reload_required = true;
				if failure.ends_session() && failure != Failure::Capacity {
					self.fail(failure);
				}
			}
		}
	}

	pub(crate) fn invalidate_messaging_permissions(&mut self, failure: Option<Failure>) {
		self.messaging_permissions.snapshot = None;
		self.messaging_permissions.pending = false;
		self.messaging_permissions.writing = false;
		self.messaging_permissions.reload_required = true;
		self.messaging_permissions.error = failure;
		self.messaging_permissions.request = self.messaging_permissions.request.wrapping_add(1);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn acknowledged_changes_reject_stale_responses_and_demo_stays_offline() {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			..State::default()
		};
		let Command::MessagingPermissions { request, .. } =
			state.request_messaging_permissions().unwrap()
		else {
			panic!()
		};
		state.apply_messaging_permissions(request + 1, Ok(Snapshot::default()));
		assert!(state.messaging_permissions.snapshot.is_none());
		state.apply_messaging_permissions(request, Ok(Snapshot::default()));
		let command = state
			.update_messaging_permissions(Change::DefaultAllowDms(false))
			.unwrap();
		assert!(
			state
				.messaging_permissions
				.snapshot
				.as_ref()
				.unwrap()
				.default_allow_dms
		);
		state.command_rejected(command);
		assert!(!state.messaging_permissions.pending);
		assert!(state.messaging_permissions.error.is_some());
		let Command::MessagingPermissions { request, .. } =
			state.request_messaging_permissions().unwrap()
		else {
			panic!()
		};
		state.invalidate_messaging_permissions(None);
		state.apply_messaging_permissions(request, Ok(Snapshot::default()));
		assert!(state.messaging_permissions.snapshot.is_none());
		state.logout();
		state.demo = true;
		assert!(state.request_messaging_permissions().is_none());
		assert!(
			state
				.update_messaging_permissions(Change::DefaultAllowDms(false))
				.is_none()
		);
		assert!(
			!state
				.messaging_permissions
				.snapshot
				.unwrap()
				.default_allow_dms
		);
	}
	#[test]
	fn uncertain_privacy_writes_preserve_draft_and_require_explicit_reload_and_apply() {
		use model::privacy_preferences::{Change as PrivacyChange, ProfileVisibility};
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			..State::default()
		};
		let Command::MessagingPermissions { request, .. } =
			state.request_messaging_permissions().unwrap()
		else {
			panic!();
		};
		state.apply_messaging_permissions(request, Ok(Snapshot::default()));
		let change = Change::Privacy(PrivacyChange::ProfileVisibility(ProfileVisibility::Friends));
		let Command::MessagingPermissions { request, .. } =
			state.update_messaging_permissions(change.clone()).unwrap()
		else {
			panic!();
		};
		state.apply_messaging_permissions(request, Err(Failure::Ambiguous));
		assert_eq!(state.messaging_permissions.draft, Some(change.clone()));
		assert!(state.update_messaging_permissions(change.clone()).is_none());
		let Command::MessagingPermissions {
			request,
			change: read,
		} = state.request_messaging_permissions().unwrap()
		else {
			panic!();
		};
		assert!(read.is_none());
		state.apply_messaging_permissions(request, Ok(Snapshot::default()));
		assert!(!state.messaging_permissions.reload_required);
		assert_eq!(
			state.messaging_permissions.draft,
			Some(change.clone()),
			"read must not automatically repeat the write"
		);
		let Command::MessagingPermissions { request, .. } =
			state.update_messaging_permissions(change.clone()).unwrap()
		else {
			panic!();
		};
		let mut saved = Snapshot::default();
		change.apply(&mut saved);
		state.apply_messaging_permissions(request, Ok(saved));
		assert!(state.messaging_permissions.draft.is_none());
		assert_eq!(
			state
				.messaging_permissions
				.snapshot
				.as_ref()
				.unwrap()
				.privacy
				.profile_visibility,
			ProfileVisibility::Friends
		);
	}
}
