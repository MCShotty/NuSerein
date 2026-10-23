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

	/// Spare vector capacity is owned memory too, even when a page has no rows.
	fn valid_panel_state() -> State {
		const PANEL: usize = 900 * 1024;
		let user = crate::tests::message(1).author;
		let owner = user.id;
		let mut state = State {
			user: Some(user),
			demo: true,
			gateway_connected: true,
			auth: crate::auth::AuthState::Authenticated,
			..Default::default()
		};
		state.guilds.push(model::Guild {
			id: Id(2),
			name: "Synthetic server".into(),
			icon: None,
			default_message_notifications: None,
			emojis: None,
			stickers: None,
		});
		state.permissions.guilds.insert(
			Id(2),
			model::permissions::Guild {
				id: Id(2),
				owner: Some(owner),
				member: None,
				roles: None,
			},
		);
		let view = &mut state.server_admin;
		view.guild = Some(Id(2));
		view.members = Some(admin::Members {
			items: Vec::with_capacity(PANEL / size_of::<admin::Member>()),
			..Default::default()
		});
		assert!(view.members.as_ref().unwrap().valid());
		view.emojis = Some(admin::Emojis {
			items: Vec::with_capacity(PANEL / size_of::<admin::Emoji>()),
			..Default::default()
		});
		let page = admin::Result::Emojis(view.emojis.take().unwrap());
		assert!(page.valid());
		let admin::Result::Emojis(page) = page else {
			unreachable!()
		};
		view.emojis = Some(page);
		view.stickers = Some(admin::Stickers {
			items: Vec::with_capacity(PANEL / size_of::<admin::Sticker>()),
			..Default::default()
		});
		let page = admin::Result::Stickers(view.stickers.take().unwrap());
		assert!(page.valid());
		let admin::Result::Stickers(page) = page else {
			unreachable!()
		};
		view.stickers = Some(page);
		view.sounds = Some(admin::Sounds {
			items: Vec::with_capacity(PANEL / size_of::<admin::Sound>()),
			..Default::default()
		});
		let page = admin::Result::Sounds(view.sounds.take().unwrap());
		assert!(page.valid());
		let admin::Result::Sounds(page) = page else {
			unreachable!()
		};
		view.sounds = Some(page);
		view.audit_log = Some(model::server_audit_log::Page {
			guild: Id(2),
			entries: Vec::with_capacity(1800 * 1024 / size_of::<model::server_audit_log::Entry>()),
			users: Vec::new(),
			has_more: false,
		});
		assert!(view.audit_log.as_ref().unwrap().valid());
		view.integrations = Some(model::server_integrations::Snapshot {
			guild: Id(2),
			channel: None,
			integrations: None,
			webhooks: Some(Vec::with_capacity(
				800 * 1024 / size_of::<model::server_integrations::Webhook>(),
			)),
		});
		assert!(view.integrations.as_ref().unwrap().valid());
		assert!(state.retained_admin_bytes() > 6 * 1024 * 1024);
		assert!(state.retained_admin_bytes() < MAX_ADMIN_BYTES);
		state
	}

	#[test]
	fn a_small_read_preserves_multiple_valid_cached_panels_under_the_budget() {
		let mut state = valid_panel_state();
		let before = state.retained_admin_bytes();
		let command = state.request_server_admin(
			Id(2),
			admin::Action::Invites(model::server_invites::Action::Load),
		);
		assert!(matches!(command, Some(crate::Command::ServerAdmin { .. })));
		assert!(state.retained_admin_bytes() < MAX_ADMIN_BYTES);
		assert!(
			state.retained_admin_bytes() - before < 1024,
			"a no-payload read reserves its boxed command, not four MiB"
		);
		assert!(state.server_admin.members.is_some() && state.server_admin.audit_log.is_some());
		assert!(state.server_admin.pending && state.server_admin.error.is_none());
	}

	#[test]
	fn a_small_challenged_action_keeps_its_pending_operation_and_large_valid_panels() {
		use crate::mfa::{self, Method, Target};
		let mut state = valid_panel_state();
		let command = state
			.request_server_admin(
				Id(2),
				admin::Action::CreateEmoji {
					name: "synthetic_emoji".into(),
					image: "data:image/png;base64,AAAA".into(),
				},
			)
			.unwrap();
		let target = Target::from_command(&command).unwrap();
		assert!(target.bytes() < 1024);
		let before = state.retained_admin_bytes();
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::Mfa(mfa::Event::Required(mfa::Notice {
				id: 7,
				target,
				methods: vec![Method::Totp],
			})),
		});
		assert!(state.retained_admin_bytes() < MAX_ADMIN_BYTES);
		assert!(state.retained_admin_bytes() - before < 1024);
		assert!(
			state
				.mfa
				.pending
				.as_ref()
				.is_some_and(|pending| pending.id == 7)
		);
		assert!(state.server_admin.pending && state.server_admin.saving);
		assert!(state.server_admin.members.is_some() && state.server_admin.audit_log.is_some());
		assert!(
			state.maintain_mfa().is_none(),
			"small actions must not cancel their own verification"
		);
		let Some(crate::Command::Mfa(verify)) = state.submit_mfa(Method::Totp, "123456".into())
		else {
			panic!("pending action remains eligible")
		};
		assert!(state.mfa_command_allowed(&verify));
	}

	#[test]
	fn stripped_upload_payload_is_still_charged_once_for_the_transport() {
		let mut state = valid_panel_state();
		let before = state.retained_admin_bytes();
		let command = state.request_server_admin(
			Id(2),
			admin::Action::CreateSticker {
				name: "Synthetic sticker".into(),
				description: String::new(),
				tags: "🦆".into(),
				filename: "sticker.png".into(),
				content_type: "image/png".into(),
				file: vec![1; admin::MAX_STICKER_FILE_BYTES],
			},
		);
		assert!(matches!(command, Some(crate::Command::ServerAdmin { .. })));
		let charge = state.retained_admin_bytes() - before;
		assert!(charge >= admin::MAX_STICKER_FILE_BYTES);
		assert!(
			charge < admin::MAX_STICKER_FILE_BYTES + 4096,
			"the reconciliation copy releases image bytes"
		);
		assert!(state.server_admin.members.is_some() && state.server_admin.saving);
	}

	#[test]
	fn large_pending_drafts_and_panels_are_rejected_before_dispatch() {
		use model::community_admin as m;
		let mut state = valid_panel_state();
		let data = m::Data::Onboarding(m::Onboarding {
			prompts: (1..=18)
				.map(|prompt| m::Prompt {
					id: Id(prompt),
					kind: 0,
					title: "Question".into(),
					single_select: true,
					required: false,
					in_onboarding: false,
					options: (1..=50)
						.map(|option| m::OptionRow {
							id: Id(prompt * 100 + option),
							title: "Choice".into(),
							description: Some("🦆".repeat(90)),
							emoji: m::Emoji::default(),
							channels: Vec::new(),
							roles: Vec::new(),
						})
						.collect(),
				})
				.collect(),
			..Default::default()
		});
		assert!(data.valid() && data.editable());
		let mut after = data.clone();
		let m::Data::Onboarding(onboarding) = &mut after else {
			unreachable!()
		};
		onboarding.prompts[0].title = "Updated question".into();
		state.server_admin.community = Some(m::Snapshot {
			guild: Id(2),
			data: data.clone(),
			roles: Vec::new(),
		});
		assert!(state.server_admin.community.as_ref().unwrap().valid());
		let action = admin::Action::Community(m::Action::Edit {
			before: Box::new(data),
			after: Box::new(after),
			pending: m::PendingApplications::Approve,
		});
		assert!(action.valid());
		assert!(state.server_admin_action_allowed(Id(2), &action));
		assert!(state.retained_admin_bytes() + 2 * action.heap_bytes() > MAX_ADMIN_BYTES);
		let generation = state.generation;
		assert!(state.request_server_admin(Id(2), action).is_none());
		assert!(!state.server_admin.pending);
		assert!(state.server_admin.error.is_some());
		assert!(state.server_admin.community.is_none());
		assert!(state.retained_admin_bytes() < MAX_ADMIN_BYTES);
		assert_eq!(state.generation, generation);
		assert_eq!(state.auth, crate::auth::AuthState::Authenticated);
	}

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
