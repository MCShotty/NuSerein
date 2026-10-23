//! Administration snapshots share a session budget in addition to their page limits.
//! Charge only on administrative arrivals; ordinary messages never walk these catalogs.
use crate::State;
use model::{User, server_admin as admin};

pub const MAX_ADMIN_BYTES: usize = 8 * 1024 * 1024;

impl State {
	/// Conservative allocation charge, including spare vector and string capacity.
	/// Shared conversation/image caches remain in their existing independent budgets.
	pub fn retained_admin_bytes(&self) -> usize {
		let view = &self.server_admin;
		let mut bytes = size_of_val(view)
			+ view.query.search.capacity()
			+ view.retained_action_bytes()
			+ self.mfa.bytes()
			+ view.access.bytes()
			+ self.server_incidents.len()
				* (size_of::<model::Id>() + size_of::<model::server_access::Incident>() + 64);
		for charge in [
			view.command_permissions
				.as_ref()
				.map_or(0, |page| page.bytes()),
			view.community.as_ref().map_or(0, |page| page.bytes()),
			view.automod.as_ref().map_or(0, |page| page.bytes()),
			view.bans.as_ref().map_or(0, |page| page.bytes()),
			view.member_review.as_ref().map_or(0, |page| page.bytes()),
			view.bulk_report.as_ref().map_or(0, |page| page.bytes()),
			view.moderation_target
				.as_ref()
				.map_or(0, |member| member.bytes()),
			view.audit_log.as_ref().map_or(0, |page| page.bytes()),
			view.integrations.as_ref().map_or(0, |page| page.bytes()),
			view.invites.as_ref().map_or(0, |page| page.bytes()),
			view.roles.as_ref().map_or(0, |page| page.bytes()),
			view.members.as_ref().map_or(0, |page| page.bytes()),
			self.server_settings
				.snapshot
				.as_ref()
				.map_or(0, |page| page.heap_bytes()),
		] {
			bytes = bytes.saturating_add(charge);
		}
		if let Some(page) = &view.emojis {
			bytes = bytes.saturating_add(page.items.capacity() * size_of::<admin::Emoji>());
			for row in &page.items {
				bytes = bytes
					.saturating_add(row.emoji.heap_bytes())
					.saturating_add(row.uploader.as_ref().map_or(0, User::heap_bytes));
			}
		}
		if let Some(page) = &view.stickers {
			bytes = bytes.saturating_add(page.items.capacity() * size_of::<admin::Sticker>());
			for row in &page.items {
				bytes = bytes
					.saturating_add(row.sticker.heap_bytes())
					.saturating_add(row.uploader.as_ref().map_or(0, User::heap_bytes));
			}
		}
		if let Some(page) = &view.sounds {
			bytes = bytes.saturating_add(page.items.capacity() * size_of::<admin::Sound>());
			for row in &page.items {
				bytes = bytes
					.saturating_add(row.sound.heap_bytes())
					.saturating_add(row.uploader.as_ref().map_or(0, User::heap_bytes));
			}
		}
		bytes
	}

	pub(crate) fn enforce_admin_budget(&mut self) {
		if self.retained_admin_bytes() <= MAX_ADMIN_BYTES {
			return;
		}
		// Completed requests have already been reconciled. Retire their local pages,
		// without treating local panel pressure as an expired account or retrying writes.
		self.server_admin.reset();
		self.server_admin.error =
			Some("Administration panels reached their memory limit; reload to continue");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::Id;

	#[test]
	fn spare_snapshot_capacity_is_charged_and_retired_without_losing_the_account() {
		let mut state = State {
			user: Some(crate::tests::message(1).author),
			auth: crate::auth::AuthState::Authenticated,
			gateway_connected: true,
			..Default::default()
		};
		let account = state.user.as_ref().unwrap().id;
		let generation = state.generation;
		let empty = state.retained_admin_bytes();
		state.server_admin.members = Some(admin::Members {
			items: Vec::with_capacity(MAX_ADMIN_BYTES / size_of::<admin::Member>() + 1),
			..Default::default()
		});
		assert!(state.retained_admin_bytes() > MAX_ADMIN_BYTES);
		state.enforce_admin_budget();
		assert!(state.server_admin.members.is_none());
		assert!(state.server_admin.error.is_some());
		assert_eq!(state.generation, generation);
		assert_eq!(state.user.as_ref().unwrap().id, account);
		assert_eq!(state.auth, crate::auth::AuthState::Authenticated);
		assert!(state.gateway_connected);
		assert_eq!(state.retained_admin_bytes(), empty);
	}

	#[test]
	fn replacing_and_releasing_a_page_releases_its_charge() {
		let mut state = State::default();
		let empty = state.retained_admin_bytes();
		state.server_admin.invites = Some(model::server_invites::Snapshot {
			guild: Id(1),
			items: Vec::with_capacity(100),
			features: vec![],
		});
		let charged = state.retained_admin_bytes();
		assert!(charged >= empty + 100 * size_of::<model::server_invites::Invite>());
		state.enforce_admin_budget();
		assert!(state.server_admin.invites.is_some());
		state.server_admin.reset();
		assert_eq!(state.retained_admin_bytes(), empty);
	}
}
