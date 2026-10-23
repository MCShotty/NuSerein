//! One permission-checked, session-only server settings draft.
use crate::{MessagingUi, avatars::Avatars, design, dialog};
use client_core::{Command, State};
use egui::Color32;
use model::{
	Id, Patch,
	server_settings::{Edit, Settings, Trait},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
	#[default]
	Profile,
	Engagement,
	Safety,
	Emoji,
	Stickers,
	Soundboard,
	Members,
	Roles,
	Invites,
	Integrations,
	AuditLog,
	AutoMod,
	Moderation,
	Onboarding,
	ChannelsRoles,
	Screening,
	Guide,
	Applications,
	Access,
	Bans,
	Events,
}
impl Page {
	fn allowed(self, state: &State, guild: Id) -> bool {
		match self {
			Self::Applications => crate::server_access::AccessUi::allowed(
				state,
				guild,
				crate::server_access::Page::Applications,
			),
			Self::Access => crate::server_access::AccessUi::allowed(
				state,
				guild,
				crate::server_access::Page::Access,
			),
			Self::Bans => state.can_view_guild_bans(guild),
			Self::Events => state.can_browse_events(guild),
			Self::Onboarding | Self::ChannelsRoles | Self::Screening | Self::Guide => {
				state.can_open_community_settings(guild)
			}
			Self::AutoMod => state.can_manage_guild(guild),
			Self::Moderation => state.can_open_moderation(guild),
			Self::Profile | Self::Engagement | Self::Safety => state.can_manage_guild(guild),
			Self::Emoji => state.can_open_emoji_settings(guild),
			Self::Stickers => state.can_open_sticker_settings(guild),
			Self::Soundboard => state.can_open_sound_settings(guild),
			Self::Members => state.can_open_member_settings(guild),
			Self::Roles => state.can_open_role_settings(guild),
			Self::Invites => state.can_open_invite_settings(guild),
			Self::Integrations => state.can_open_integration_settings(guild),
			Self::AuditLog => state.can_open_audit_log_settings(guild),
		}
	}
	fn label(self) -> &'static str {
		match self {
			Self::Applications => "server-settings-page-applications",
			Self::Access => "server-settings-page-access",
			Self::Bans => "server-settings-page-bans",
			Self::Events => "server-settings-page-events",
			Self::Onboarding => "community-onboarding",
			Self::ChannelsRoles => "community-channels-roles",
			Self::Screening => "community-screening",
			Self::Guide => "community-guide",
			Self::AutoMod => "automod-title",
			Self::Moderation => "moderation-title",
			Self::Profile => "server-settings-page-profile",
			Self::Engagement => "server-settings-page-engagement",
			Self::Safety => "server-settings-page-safety",
			Self::Emoji => "server-settings-page-emoji",
			Self::Stickers => "server-settings-page-stickers",
			Self::Soundboard => "server-settings-page-soundboard",
			Self::Members => "server-settings-page-members",
			Self::Roles => "server-settings-page-roles",
			Self::Invites => "server-settings-page-invites",
			Self::Integrations => "server-settings-page-integrations",
			Self::AuditLog => "server-settings-page-audit-log",
		}
	}
	fn activity_label(self, state: &State, guild: Id, label: &str) -> Option<String> {
		let count = state.guild_application_count(guild);
		if self == Self::Applications
			&& let Some(count) = count.filter(|count| *count > 0)
		{
			return Some(format!(
				"{} ({})",
				crate::i18n::translate_if_key(label),
				if count > 99 {
					"99+".into()
				} else {
					count.to_string()
				}
			));
		}
		(matches!(self, Self::Members | Self::Applications)
			&& count.is_none()
			&& state.server_admin.guild == Some(guild)
			&& state.server_admin.access.applications_changed
			&& state.can_review_guild_applications(guild))
		.then(|| format!("{} •", crate::i18n::translate_if_key(label)))
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
	Server,
	People,
	Moderation,
	Community,
	Apps,
	Expressions,
}
impl Section {
	fn label(self) -> &'static str {
		match self {
			Self::Server => "server-settings-section-server",
			Self::People => "server-settings-show-people",
			Self::Moderation => "server-settings-show-moderation",
			Self::Community => "server-settings-section-community",
			Self::Apps => "server-settings-show-apps",
			Self::Expressions => "server-settings-show-expression",
		}
	}
}
const PAGES: [Page; 21] = [
	Page::Profile,
	Page::Engagement,
	Page::Access,
	Page::Members,
	Page::Applications,
	Page::Roles,
	Page::Invites,
	Page::Safety,
	Page::AutoMod,
	Page::Moderation,
	Page::Bans,
	Page::AuditLog,
	Page::Onboarding,
	Page::ChannelsRoles,
	Page::Screening,
	Page::Guide,
	Page::Events,
	Page::Integrations,
	Page::Emoji,
	Page::Stickers,
	Page::Soundboard,
];
impl Page {
	fn section(self) -> Section {
		match self {
			Self::Profile | Self::Engagement | Self::Access => Section::Server,
			Self::Members | Self::Applications | Self::Roles | Self::Invites => Section::People,
			Self::Safety | Self::AutoMod | Self::Moderation | Self::Bans | Self::AuditLog => {
				Section::Moderation
			}
			Self::Onboarding
			| Self::ChannelsRoles
			| Self::Screening
			| Self::Guide
			| Self::Events => Section::Community,
			Self::Integrations => Section::Apps,
			Self::Emoji | Self::Stickers | Self::Soundboard => Section::Expressions,
		}
	}
	fn keywords(self) -> &'static str {
		match self {
			Self::Applications => "members join requests pending approve reject interview answers",
			Self::Access => "community owner ownership transfer rules updates channels access",
			Self::Bans => "ban unban search reason history",
			Self::Events => "schedule recurring events stage interested RSVP",
			Self::Profile => "name icon banner traits description appearance",
			Self::Engagement => {
				"defaults notification system welcome boost inactivity afk timeout activity"
			}
			Self::Safety => "verification filter explicit content phone email security",
			Self::Members => "people nickname username joined account age roles prune",
			Self::Roles => "permissions hierarchy access reorder icon member",
			Self::Invites => "links inviter uses expiry revoke pause access",
			Self::AutoMod => "rules keywords spam mention exemptions alert",
			Self::Moderation => "bans unban timeout kick member reason",
			Self::AuditLog => "history changes actor actions reasons",
			Self::Onboarding => "questions defaults join welcome",
			Self::ChannelsRoles => "customize questions channels roles",
			Self::Screening => "rules screening membership verification terms",
			Self::Guide => "server guide resources tasks welcome",
			Self::Integrations => "apps bots commands permissions webhooks followed channels",
			Self::Emoji => "expressions custom emotes upload",
			Self::Stickers => "expressions stickers upload",
			Self::Soundboard => "expressions sounds upload volume",
		}
	}
}
#[derive(Clone, Copy)]
struct Destination {
	page: Page,
	label: &'static str,
	anchor: Option<&'static str>,
}
const SETTINGS: [Destination; 11] = [
	Destination {
		page: Page::Profile,
		label: "server-settings-profile-form-name",
		anchor: Some("name"),
	},
	Destination {
		page: Page::Profile,
		label: "server-settings-profile-form-icon",
		anchor: Some("icon"),
	},
	Destination {
		page: Page::Profile,
		label: "server-settings-profile-form-banner",
		anchor: Some("banner"),
	},
	Destination {
		page: Page::Profile,
		label: "server-settings-profile-form-traits",
		anchor: Some("traits"),
	},
	Destination {
		page: Page::Profile,
		label: "server-settings-profile-form-description",
		anchor: Some("description"),
	},
	Destination {
		page: Page::Engagement,
		label: "server-settings-engagement-system-messages",
		anchor: Some("system"),
	},
	Destination {
		page: Page::Engagement,
		label: "server-settings-engagement-activity-feed-settings",
		anchor: Some("activity"),
	},
	Destination {
		page: Page::Engagement,
		label: "server-settings-engagement-default-notification-settings",
		anchor: Some("notifications"),
	},
	Destination {
		page: Page::Engagement,
		label: "server-settings-engagement-inactive-channel",
		anchor: Some("inactive"),
	},
	Destination {
		page: Page::Safety,
		label: "server-settings-safety-verification",
		anchor: Some("verification"),
	},
	Destination {
		page: Page::Safety,
		label: "server-settings-safety-filter",
		anchor: Some("filter"),
	},
];
fn destinations(state: &State, guild: Id, query: &str) -> Vec<Destination> {
	let query = query.trim().to_lowercase();
	if query.is_empty() {
		return PAGES
			.into_iter()
			.filter(|page| page.allowed(state, guild))
			.map(|page| Destination {
				page,
				label: page.label(),
				anchor: None,
			})
			.collect();
	}
	let words: Vec<_> = query.split_whitespace().collect();
	let mut results = Vec::with_capacity(PAGES.len() + SETTINGS.len());
	for page in PAGES.into_iter().filter(|page| page.allowed(state, guild)) {
		let label = crate::i18n::translate_if_key(page.label());
		let section = crate::i18n::translate_if_key(page.section().label());
		let searchable = format!("{label} {section} {}", page.keywords()).to_lowercase();
		if words.iter().all(|word| searchable.contains(word)) {
			results.push(Destination {
				page,
				label: page.label(),
				anchor: None,
			});
		}
	}
	if !query.is_empty() {
		for target in SETTINGS
			.into_iter()
			.filter(|target| target.page.allowed(state, guild))
		{
			let label = crate::i18n::translate_if_key(target.label).to_lowercase();
			if words.iter().all(|word| label.contains(word)) {
				results.push(target);
			}
		}
	}
	results.sort_by_key(|target| {
		(
			PAGES
				.iter()
				.position(|page| *page == target.page)
				.unwrap_or(usize::MAX),
			target.anchor.is_some(),
		)
	});
	results
}
fn search_anchor(ui: &mut egui::Ui, guild: Id, name: &'static str) {
	let key = egui::Id::unique(("server-settings-search-anchor", guild));
	if ui.ctx().data(|data| data.get_temp::<&'static str>(key)) == Some(name) {
		ui.ctx()
			.data_mut(|data| data.remove_temp::<&'static str>(key));
		ui.scroll_to_rect(
			egui::Rect::from_min_size(ui.cursor().min, egui::vec2(1.0, 72.0)),
			Some(egui::Align::Min),
		);
	}
}

#[derive(Default)]
pub(super) struct Editor {
	community: crate::community_admin::CommunityUi,
	access: crate::server_access::AccessUi,
	pub events_requested: Option<Id>,
	automod: crate::server_automod::AutoModUi,
	moderation: crate::server_moderation::ModerationUi,
	scope: Option<(u64, Id)>,
	page: Page,
	navigation_search: String,
	navigation_cursor: usize,
	navigation_ime: bool,
	navigation_error: Option<&'static str>,
	refresh_conflict: bool,
	show_all_pages: bool,
	draft: Option<Settings>,
	baseline: Option<Settings>,
	revision: u64,
	submitted: bool,
	discard: bool,
	icon: Patch<String>,
	icon_preview: Option<egui::TextureHandle>,
	icon_request: u64,
	icon_requested: bool,
	icon_pending: bool,
	icon_error: Option<&'static str>,
	form_error: Option<&'static str>,
	delete: bool,
	delete_name: String,
	emoji_picker: crate::emoji_picker::Picker,
	pub(super) admin: crate::server_admin::Admin,
	stickers: crate::server_stickers::StickersUi,
	sounds: crate::server_sounds::SoundsUi,
	roles: crate::server_roles::RolesUi,
	invites: crate::server_invites::InvitesUi,
	integrations: crate::server_integrations::IntegrationsUi,
	audit_log: crate::server_audit_log::AuditLogUi,
}

impl MessagingUi {
	pub fn take_community_artwork_request(&mut self) -> Option<(u64, Id, Option<Id>, bool, u64)> {
		let (generation, guild) = self.server_settings.scope?;
		let (channel, resource, request) = self.server_settings.community.take_artwork_request()?;
		Some((generation, guild, channel, resource, request))
	}
	pub fn community_artwork_active(&self, scope: (u64, Id, Option<Id>, bool, u64)) -> bool {
		self.server_settings.scope == Some((scope.0, scope.1))
			&& self
				.server_settings
				.community
				.artwork_active((scope.2, scope.3, scope.4))
	}
	pub fn accept_community_artwork(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, Option<Id>, bool, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) {
		if self.server_settings.scope == Some((scope.0, scope.1)) {
			self.server_settings
				.community
				.accept_artwork(ctx, (scope.2, scope.3, scope.4), result);
		}
	}
	pub fn preview_server_moderation(
		&mut self,
		state: &mut State,
		guild: Id,
		user: Option<Id>,
	) -> Option<Command> {
		if !state.can_open_moderation(guild) {
			return None;
		}
		self.settings.open = false;
		self.server_settings = Editor {
			scope: Some((state.generation, guild)),
			page: Page::Moderation,
			..Default::default()
		};
		if let Some(user) = user {
			self.server_settings
				.moderation
				.select(state.generation, guild, user);
			state.request_server_admin(
				guild,
				model::server_admin::Action::Moderation(model::moderation::Action::Member(user)),
			)
		} else {
			None
		}
	}
	pub fn preview_server_automod(&mut self, state: &mut State, guild: Id) -> Option<Command> {
		if !state.can_manage_guild(guild) {
			return None;
		}
		self.settings.open = false;
		self.server_settings = Editor {
			scope: Some((state.generation, guild)),
			page: Page::AutoMod,
			..Default::default()
		};
		state.request_server_admin(
			guild,
			model::server_admin::Action::AutoMod(model::automod::Action::Load),
		)
	}
	/// Select the engagement page in the offline preview harness.
	pub fn preview_server_engagement(&mut self) {
		self.server_settings.page = Page::Engagement;
	}
	/// Select the safety page in the offline preview harness.
	pub fn preview_server_safety(&mut self) {
		self.server_settings.page = Page::Safety;
	}
	pub fn preview_server_roles(
		&mut self,
		state: &mut State,
		guild: Id,
		role: Option<Id>,
	) -> Option<Command> {
		if !state.can_open_role_settings(guild) {
			return None;
		}
		self.settings.open = false;
		self.server_settings = Editor {
			scope: Some((state.generation, guild)),
			page: Page::Roles,
			..Editor::default()
		};
		self.server_settings.roles.select(role, guild);
		self.server_settings.roles.load(state, guild)
	}
	pub fn open_server_applications(&mut self, state: &mut State, guild: Id) -> Option<Command> {
		self.preview_server_admin(state, guild, "applications")
	}

	pub fn take_webhook_avatar_request(&mut self) -> Option<(u64, Id, Id, u64)> {
		if let Some(scope) = self.channel_menu.take_webhook_avatar_request() {
			return Some(scope);
		}
		let (generation, guild) = self.server_settings.scope?;
		let (webhook, request) = self.server_settings.integrations.take_avatar_request()?;
		Some((generation, guild, webhook, request))
	}
	pub fn webhook_avatar_active(&self, scope: (u64, Id, Id, u64)) -> bool {
		self.channel_menu.webhook_avatar_active(scope)
			|| (self.server_settings.scope == Some((scope.0, scope.1))
				&& self
					.server_settings
					.integrations
					.avatar_active(scope.2, scope.3))
	}
	pub fn accept_webhook_avatar(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, Id, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) {
		if self.channel_menu.webhook_avatar_active(scope) {
			self.channel_menu.accept_webhook_avatar(ctx, scope, result);
		} else if self.server_settings.scope == Some((scope.0, scope.1)) {
			self.server_settings
				.integrations
				.accept_avatar(ctx, scope.2, scope.3, result);
		}
	}

	pub fn take_server_role_icon_request(&mut self) -> Option<(u64, Id, Id, u64)> {
		let (generation, guild) = self.server_settings.scope?;
		let role = self.server_settings.roles.take_icon_request()?;
		self.server_role_icon_sequence = self.server_role_icon_sequence.wrapping_add(1);
		self.server_settings.roles.icon_request = self.server_role_icon_sequence;
		Some((generation, guild, role, self.server_role_icon_sequence))
	}
	pub fn accept_server_role_icon(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, Id, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) {
		if self.server_settings.scope == Some((scope.0, scope.1)) {
			self.server_settings
				.roles
				.accept_icon(ctx, scope.2, scope.3, result);
		}
	}
	pub fn preview_server_admin(
		&mut self,
		state: &mut State,
		guild: Id,
		page: &str,
	) -> Option<Command> {
		if matches!(page, "roles" | "role-editor" | "role-permissions") {
			let command = self.preview_server_roles(state, guild, None);
			if page != "roles" {
				self.server_settings
					.roles
					.preview_editor(page == "role-permissions");
			}
			return command;
		}
		if page == "automod" {
			return self.preview_server_automod(state, guild);
		}
		if matches!(page, "applications" | "access" | "bans" | "events") {
			let destination = match page {
				"applications" => Page::Applications,
				"access" => Page::Access,
				"bans" => Page::Bans,
				_ => Page::Events,
			};
			if !destination.allowed(state, guild) || self.has_server_settings_changes() {
				return None;
			}
			self.settings.open = false;
			self.server_settings = Editor {
				scope: Some((state.generation, guild)),
				page: destination,
				..Default::default()
			};
			return match destination {
				Page::Applications => self.server_settings.access.load(
					state,
					guild,
					crate::server_access::Page::Applications,
				),
				Page::Access => self.server_settings.access.load(
					state,
					guild,
					crate::server_access::Page::Access,
				),
				_ => None,
			};
		}
		if page == "moderation" {
			return self.preview_server_moderation(state, guild, None);
		}
		let webhooks = page == "webhooks";
		let expand_audit = page == "audit-log-expanded";
		let page = if page == "onboarding" {
			Page::Onboarding
		} else if page == "channels-roles" {
			Page::ChannelsRoles
		} else if page == "screening" {
			Page::Screening
		} else if page == "guide" {
			Page::Guide
		} else if matches!(page, "audit-log" | "audit-log-expanded") {
			Page::AuditLog
		} else if matches!(page, "integrations" | "webhooks") {
			Page::Integrations
		} else if page == "invites" {
			Page::Invites
		} else if page == "members" {
			Page::Members
		} else if page == "stickers" {
			Page::Stickers
		} else if page == "soundboard" {
			Page::Soundboard
		} else {
			Page::Emoji
		};
		if !page.allowed(state, guild) {
			return None;
		}
		self.settings.open = false;
		self.server_settings = Editor {
			scope: Some((state.generation, guild)),
			page,
			..Editor::default()
		};
		if matches!(
			page,
			Page::Onboarding | Page::ChannelsRoles | Page::Screening | Page::Guide
		) {
			state.request_server_admin(
				guild,
				model::server_admin::Action::Community(model::community_admin::Action::Load(
					match page {
						Page::Screening => model::community_admin::Section::Screening,
						Page::Guide => model::community_admin::Section::Guide,
						_ => model::community_admin::Section::Onboarding,
					},
				)),
			)
		} else if page == Page::AuditLog {
			self.server_settings.audit_log.preview(expand_audit);
			self.server_settings.audit_log.load(state, guild)
		} else if page == Page::Integrations {
			self.server_settings.integrations.preview(webhooks);
			self.server_settings.integrations.load(state, guild)
		} else if page == Page::Invites {
			self.server_settings.invites.load(state, guild)
		} else if page == Page::Stickers {
			self.server_settings.stickers.load(state, guild)
		} else if page == Page::Soundboard {
			self.server_settings.sounds.load(state, guild)
		} else {
			self.server_settings
				.admin
				.load(state, guild, page == Page::Members)
		}
	}
	pub fn accepts_server_emoji_drops(&self) -> bool {
		self.server_settings.is_open() && self.server_settings.page == Page::Emoji
	}
	pub fn take_server_sticker_request(&mut self) -> Option<(u64, Id, u64)> {
		let (generation, guild) = self.server_settings.scope?;
		if !self.server_settings.stickers.take_request() {
			return None;
		}
		self.server_sticker_sequence = self.server_sticker_sequence.wrapping_add(1);
		self.server_settings.stickers.request = self.server_sticker_sequence;
		Some((generation, guild, self.server_sticker_sequence))
	}
	pub fn accept_server_sticker(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, u64),
		result: Result<Option<crate::server_stickers::PreparedSticker>, &'static str>,
	) {
		if self.server_settings.scope == Some((scope.0, scope.1))
			&& self.server_settings.stickers.request == scope.2
		{
			self.server_settings.stickers.accept(ctx, result);
		}
	}
	pub fn take_server_sound_request(&mut self) -> Option<(u64, Id, u64)> {
		let (generation, guild) = self.server_settings.scope?;
		if !self.server_settings.sounds.take_request() {
			return None;
		}
		self.server_sticker_sequence = self.server_sticker_sequence.wrapping_add(1);
		self.server_settings.sounds.request = self.server_sticker_sequence;
		Some((generation, guild, self.server_sticker_sequence))
	}
	pub fn accept_server_sound(
		&mut self,
		scope: (u64, Id, u64),
		result: Result<Option<crate::server_sounds::PreparedSound>, &'static str>,
	) {
		if self.server_settings.scope == Some((scope.0, scope.1))
			&& self.server_settings.sounds.request == scope.2
		{
			self.server_settings.sounds.accept(result);
		}
	}
	/// The reviewed sound's scope while its dialog is open.
	fn server_sound_scope(&self) -> Option<(u64, Id, u64)> {
		let (generation, guild) = self.server_settings.scope?;
		Some((generation, guild, self.server_settings.sounds.request))
	}
	/// A selection of the reviewed sound to encode for upload.
	pub fn take_server_sound_trim(&mut self) -> Option<((u64, Id, u64), u32, u32)> {
		let scope = self.server_sound_scope()?;
		let (start, end) = self.server_settings.sounds.take_trim()?;
		Some((scope, start, end))
	}
	pub fn accept_server_sound_trim(
		&mut self,
		scope: (u64, Id, u64),
		result: Result<Vec<u8>, &'static str>,
	) {
		if self.server_sound_scope() == Some(scope) {
			self.server_settings.sounds.accept_trim(result);
		}
	}
	/// A request to play (`Some`) or stop (`None`) the reviewed selection locally.
	#[allow(clippy::type_complexity)]
	pub fn take_server_sound_preview(
		&mut self,
	) -> Option<((u64, Id, u64), Option<(u32, u32, u8)>)> {
		let scope = self.server_sound_scope()?;
		Some((scope, self.server_settings.sounds.take_preview()?))
	}
	/// The scope of the sound being chosen or reviewed. Decoded audio for any other scope,
	/// including a review that was reset without closing, can be released.
	pub fn server_sound_review(&self) -> Option<(u64, Id, u64)> {
		self.server_settings
			.sounds
			.reviewing()
			.then(|| self.server_sound_scope())
			.flatten()
	}
	/// True once after the sound review closed; its decoded audio can be released.
	pub fn take_server_sound_closed(&mut self) -> bool {
		self.server_settings.sounds.take_closed()
	}
	/// Offline preview: open the upload review with a synthetic sound.
	#[cfg(feature = "demo")]
	pub fn preview_server_sound_upload(&mut self, prepared: crate::server_sounds::PreparedSound) {
		self.server_settings.sounds.preview_upload(prepared);
	}
	pub fn queue_server_emoji_drop(&mut self, paths: Vec<std::path::PathBuf>) {
		if self.accepts_server_emoji_drops() {
			self.server_settings.admin.queue_files(paths);
		}
	}
	pub fn take_server_emoji_request(&mut self) -> Option<(u64, Id, u64, Vec<std::path::PathBuf>)> {
		let (generation, guild) = self.server_settings.scope?;
		let paths = self.server_settings.admin.take_files()?;
		self.server_emoji_sequence = self.server_emoji_sequence.wrapping_add(1);
		self.server_settings.admin.request = self.server_emoji_sequence;
		Some((generation, guild, self.server_emoji_sequence, paths))
	}
	pub fn accept_server_emojis(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, u64),
		result: Result<Vec<(String, String, bool, egui::ColorImage)>, &'static str>,
	) {
		if self.server_settings.scope == Some((scope.0, scope.1))
			&& self.server_settings.admin.request == scope.2
		{
			self.server_settings.admin.accept_files(ctx, result);
		}
	}
	pub fn has_server_settings_changes(&self) -> bool {
		self.server_settings.is_open()
			&& (self.server_settings.dirty()
				|| self.server_settings.submitted
				|| self.server_settings.icon_pending)
			|| self.server_settings.access.has_changes()
			|| self.server_settings.community.has_changes()
			|| self.server_settings.automod.has_changes()
			|| self.server_settings.moderation.has_changes()
			|| self.server_settings.admin.has_changes()
			|| self.server_settings.stickers.has_changes()
			|| self.server_settings.sounds.has_changes()
			|| self.server_settings.roles.has_changes()
			|| self.server_settings.invites.busy()
			|| self.server_settings.integrations.has_changes()
	}
	/// Opens the same permission-checked editor used by the server menu.
	pub fn preview_server_settings(&mut self, state: &mut State, guild: Id) -> Option<Command> {
		if !state.can_manage_guild(guild) {
			if state.can_open_moderation(guild) {
				return self.preview_server_moderation(state, guild, None);
			}
			if state.can_open_role_settings(guild) {
				return self.preview_server_roles(state, guild, None);
			}
			if state.can_open_emoji_settings(guild) {
				return self.preview_server_admin(state, guild, "emoji");
			}
			if state.can_open_sticker_settings(guild) {
				return self.preview_server_admin(state, guild, "stickers");
			}
			if state.can_open_integration_settings(guild) {
				return self.preview_server_admin(state, guild, "integrations");
			}
			if state.can_open_audit_log_settings(guild) {
				return self.preview_server_admin(state, guild, "audit-log");
			}
			return None;
		}
		self.settings.open = false;
		self.server_settings = Editor {
			scope: Some((state.generation, guild)),
			..Editor::default()
		};
		if state.server_settings.guild == Some(guild) && state.server_settings.snapshot.is_some() {
			None
		} else {
			state.load_server_settings(guild)
		}
	}

	pub fn take_server_icon_request(&mut self) -> Option<(u64, Id, u64)> {
		let editor = &mut self.server_settings;
		if !std::mem::take(&mut editor.icon_requested) {
			return None;
		}
		self.server_icon_sequence = self.server_icon_sequence.wrapping_add(1);
		editor.icon_request = self.server_icon_sequence;
		editor
			.scope
			.map(|(generation, guild)| (generation, guild, editor.icon_request))
	}

	pub fn accept_server_icon(
		&mut self,
		ctx: &egui::Context,
		scope: (u64, Id, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) {
		let editor = &mut self.server_settings;
		if editor.scope != Some((scope.0, scope.1))
			|| editor.icon_request != scope.2
			|| !editor.icon_pending
		{
			return;
		}
		editor.icon_pending = false;
		match result {
			Ok(Some((data, image)))
				if data.len() <= model::server_settings::MAX_ICON_DATA_URI
					&& image.size[0] <= 512
					&& image.size[1] <= 512 =>
			{
				editor.icon = Patch::Value(data);
				editor.icon_preview = Some(ctx.load_texture(
					"server-icon-draft",
					image,
					egui::TextureOptions::LINEAR,
				));
				editor.icon_error = None;
			}
			Ok(Some(_)) => editor.icon_error = Some("The prepared icon is too large."),
			Ok(None) => {}
			Err(error) => editor.icon_error = Some(error),
		}
	}
}

impl Editor {
	pub fn guild(&self) -> Option<Id> {
		self.scope.map(|(_, guild)| guild)
	}
	pub fn navigate_away(&mut self, state: &mut State) -> bool {
		if !self.is_open() {
			return true;
		}
		if self.access.has_changes()
			|| self.community.has_changes()
			|| self.automod.has_changes()
			|| self.moderation.has_changes()
			|| self.integrations.has_changes()
			|| self.roles.has_changes()
			|| self.stickers.has_changes()
			|| self.sounds.has_changes()
			|| self.invites.busy()
			|| self.dirty()
			|| self.refresh_conflict
			|| self.icon_pending
			|| self.admin.has_changes()
			|| state.server_admin.saving
			|| state.server_settings.saving
		{
			self.admin.navigation_error();
			return false;
		}
		*self = Self::default();
		state.close_server_settings();
		state.close_server_admin();
		true
	}
	pub fn is_open(&self) -> bool {
		self.scope.is_some()
	}
	fn dirty(&self) -> bool {
		self.draft != self.baseline || !matches!(self.icon, Patch::Absent)
	}
	fn reset(&mut self) {
		self.draft.clone_from(&self.baseline);
		self.icon = Patch::Absent;
		self.icon_preview = None;
		self.icon_error = None;
		self.form_error = None;
		self.icon_requested = false;
		self.icon_pending = false;
		self.refresh_conflict = false;
	}
	fn accept_latest(&mut self, state: &State, reapply: bool) {
		let Some(snapshot) = state
			.server_settings
			.snapshot
			.as_ref()
			.filter(|snapshot| self.guild() == Some(snapshot.guild))
		else {
			return;
		};
		let changes = reapply
			.then(|| {
				self.baseline
					.as_ref()
					.zip(self.draft.as_ref())
					.map(|(before, draft)| Edit::between(before, draft))
			})
			.flatten();
		self.baseline = Some(snapshot.clone());
		self.revision = state.server_settings.revision;
		let icon = if reapply {
			self.icon.clone()
		} else {
			Patch::Absent
		};
		let preview = reapply.then(|| self.icon_preview.take()).flatten();
		self.reset();
		self.icon_preview = preview;
		if let Some(changes) = changes {
			changes.apply(self.draft.as_mut().unwrap());
		}
		self.icon = icon;
	}
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		avatars: &mut Avatars,
		profile: &mut crate::profiles::ProfileSession,
		commands: &mut Vec<Command>,
	) {
		let Some((generation, guild)) = self.scope else {
			return;
		};
		if generation != state.generation
			|| !(state.can_manage_guild(guild)
				|| state.can_open_emoji_settings(guild)
				|| state.can_open_sticker_settings(guild)
				|| state.can_open_member_settings(guild)
				|| state.can_open_role_settings(guild)
				|| state.can_open_integration_settings(guild)
				|| state.can_open_audit_log_settings(guild)
				|| state.can_open_moderation(guild))
			|| !state.guilds.iter().any(|known| known.id == guild)
		{
			*self = Self::default();
			state.close_server_settings();
			state.close_server_admin();
			return;
		}
		if !self.page.allowed(state, guild) {
			if !state.can_manage_guild(guild) {
				self.draft = None;
				self.baseline = None;
				self.icon = Patch::Absent;
				self.icon_preview = None;
				self.icon_pending = false;
				self.icon_requested = false;
			}
			self.page = if state.can_manage_guild(guild) {
				Page::Profile
			} else if state.can_open_role_settings(guild) {
				Page::Roles
			} else if state.can_open_emoji_settings(guild) {
				Page::Emoji
			} else if state.can_open_sticker_settings(guild) {
				Page::Stickers
			} else if state.can_open_moderation(guild) {
				Page::Moderation
			} else if state.can_open_member_settings(guild) {
				Page::Members
			} else if state.can_open_integration_settings(guild) {
				Page::Integrations
			} else {
				Page::AuditLog
			};
			self.access = crate::server_access::AccessUi::default();
			self.community = crate::community_admin::CommunityUi::default();
			self.automod = crate::server_automod::AutoModUi::default();
			self.moderation = crate::server_moderation::ModerationUi::default();
			self.admin = crate::server_admin::Admin::default();
			self.stickers = crate::server_stickers::StickersUi::default();
			self.sounds = crate::server_sounds::SoundsUi::default();
			self.roles = crate::server_roles::RolesUi::default();
			self.invites = crate::server_invites::InvitesUi::default();
			self.integrations = crate::server_integrations::IntegrationsUi::default();
			self.audit_log = crate::server_audit_log::AuditLogUi::default();
			state.close_server_admin();
		}
		if let Some(page) = match self.page {
			Page::Applications => Some(crate::server_access::Page::Applications),
			Page::Access => Some(crate::server_access::Page::Access),
			Page::Safety => Some(crate::server_access::Page::Safety),
			_ => None,
		} && let Some(command) = self.access.load(state, guild, page)
		{
			commands.push(command);
		}
		self.invites.sync(state, guild);
		if self.page == Page::AuditLog
			&& let Some(command) = self.audit_log.load(state, guild)
		{
			commands.push(command);
		}
		self.integrations.sync(state, guild);
		if self.page == Page::Integrations
			&& let Some(command) = self.integrations.load(state, guild)
		{
			commands.push(command);
		}
		if self.page == Page::Invites
			&& let Some(command) = self.invites.load(state, guild)
		{
			commands.push(command);
		}
		if self.page == Page::Roles
			&& let Some(command) = self.roles.load(state, guild)
		{
			commands.push(command);
		}
		if matches!(self.page, Page::Emoji | Page::Members)
			&& let Some(command) = self.admin.load(state, guild, self.page == Page::Members)
		{
			commands.push(command);
		}
		if self.page == Page::Stickers
			&& let Some(command) = self.stickers.load(state, guild)
		{
			commands.push(command);
		}
		if self.page == Page::Soundboard
			&& let Some(command) = self.sounds.load(state, guild)
		{
			commands.push(command);
		}
		if state.server_settings.guild == Some(guild) {
			if self.submitted && !state.server_settings.pending {
				self.submitted = false;
				if state.server_settings.error.is_none() {
					self.baseline.clone_from(&state.server_settings.snapshot);
					self.reset();
				}
			}
			if (self.draft.is_none() || self.revision != state.server_settings.revision)
				&& let Some(snapshot) = &state.server_settings.snapshot
			{
				let changes = self
					.baseline
					.as_ref()
					.zip(self.draft.as_ref())
					.map(|(before, draft)| Edit::between(before, draft));
				let conflict = self.baseline.as_ref().zip(self.draft.as_ref()).is_some_and(
					|(before, draft)| settings_conflict(before, draft, snapshot, &self.icon),
				);
				if conflict || self.refresh_conflict {
					self.refresh_conflict = true;
				} else {
					let mut draft = snapshot.clone();
					if let Some(changes) = changes {
						changes.apply(&mut draft);
					}
					self.draft = Some(draft);
					self.baseline = Some(snapshot.clone());
				}
				self.revision = state.server_settings.revision;
			}
		}
		let invite_overlay = self.invites.overlay_open() || self.integrations.overlay_open();
		let settings_bar = self.dirty() || self.refresh_conflict || state.server_settings.saving;
		let roles_bar = self.page == Page::Roles && self.roles.has_changes();
		let close = dialog::SettingsShell::new("server-settings")
			.save_bar(settings_bar || roles_bar)
			.show(ctx, |ui, region| match region {
				dialog::ShellRegion::Navigation { compact } => {
					self.navigation(ui, state, guild, compact, avatars, commands);
				}
				dialog::ShellRegion::SaveBar => {
					if settings_bar {
						dialog::save_bar_frame(ctx)
							.show(ui, |ui| self.save_bar(ui, state, commands));
					}
					if roles_bar {
						dialog::save_bar_frame(ctx)
							.show(ui, |ui| self.roles.save_bar(ui, state, guild, commands));
					}
				}
				// Pages that virtualize their own list own the only vertical scrollbar;
				// wrapping them again would nest two scroll areas over one list.
				dialog::ShellRegion::Body if self.scrolling_page() => {
					dialog::page_fade(
						ui,
						egui::Id::unique(("server-settings-content", self.page as u8)),
					);
					dialog::fixed_width(ui, |ui| {
						self.page_body(ui, state, guild, avatars, profile, commands);
					});
				}
				dialog::ShellRegion::Body => {
					dialog::settings_page(ui, ("server-settings-content", self.page as u8), |ui| {
						self.page_body(ui, state, guild, avatars, profile, commands);
					});
				}
			});
		if self.page == Page::Invites {
			self.invites.overlays(ctx, state, guild, avatars, commands);
		}
		if self.page == Page::Integrations {
			self.integrations.overlays(ctx, state, guild, commands);
		}
		if !invite_overlay && close {
			if self.dirty()
				|| state.server_settings.saving
				|| self.access.has_changes()
				|| self.community.has_changes()
				|| self.automod.has_changes()
				|| self.moderation.has_changes()
				|| self.admin.has_changes()
				|| self.stickers.has_changes()
				|| self.sounds.has_changes()
				|| self.roles.has_changes()
				|| self.integrations.has_changes()
				|| self.invites.busy()
				|| state.server_admin.saving
			{
				self.discard = true;
			} else {
				self.scope = None;
				self.access = crate::server_access::AccessUi::default();
				self.community = crate::community_admin::CommunityUi::default();
				self.automod = crate::server_automod::AutoModUi::default();
				self.moderation = crate::server_moderation::ModerationUi::default();
				self.roles = crate::server_roles::RolesUi::default();
				self.stickers = crate::server_stickers::StickersUi::default();
				self.sounds = crate::server_sounds::SoundsUi::default();
				self.invites = crate::server_invites::InvitesUi::default();
				self.integrations = crate::server_integrations::IntegrationsUi::default();
				self.audit_log = crate::server_audit_log::AuditLogUi::default();
				state.close_server_settings();
				state.close_server_admin();
			}
		}
		if self.discard {
			let busy =
				state.server_settings.saving || state.server_admin.saving || self.invites.busy();
			let mut confirmation = dialog::Confirm::new(
				"discard-server-settings",
				"Discard unsaved changes?",
				"Your changes to this server will be lost.",
			)
			.danger()
			.confirm_label("Discard Changes")
			.cancel_label("Keep Editing")
			.enabled(!busy);
			if busy {
				confirmation = confirmation.note(
					dialog::Level::Info,
					"Wait for the current save to finish before closing.",
				);
			}
			match confirmation.show(ctx) {
				Some(dialog::Choice::Confirmed) => {
					*self = Self::default();
					state.close_server_settings();
					state.close_server_admin();
				}
				Some(dialog::Choice::Cancelled) => self.discard = false,
				None => {}
			}
		}
		if self.delete {
			self.delete_dialog(ctx, state, guild, commands);
		}
	}

	/// Grouped destinations remain keyboard reachable in the compact picker and sidebar.
	fn navigation(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		compact: bool,
		avatars: &mut Avatars,
		commands: &mut Vec<Command>,
	) {
		if !compact {
			self.navigation_header(ui, state, guild, avatars);
			ui.add_space(12.0);
		}
		let search_id = egui::Id::unique(("server-settings-search", guild));
		let has_ime_event = ui.input(|input| {
			input
				.events
				.iter()
				.any(|event| matches!(event, egui::Event::Ime(_)))
		});
		ui.input(|input| {
			for event in &input.events {
				match event {
					egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
						self.navigation_ime = !text.is_empty()
					}
					egui::Event::Ime(egui::ImeEvent::Commit(_)) => self.navigation_ime = false,
					_ => {}
				}
			}
		});
		let composing = self.navigation_ime || has_ime_event;
		let (down, up, enter, escape) =
			if ui.memory(|memory| memory.has_focus(search_id)) && !composing {
				ui.input_mut(|input| {
					(
						input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
						input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
						input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
						!self.navigation_search.is_empty()
							&& input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
					)
				})
			} else {
				(false, false, false, false)
			};
		if escape {
			self.navigation_search.clear();
			self.navigation_cursor = 0;
		}
		let search = ui.add(
			egui::TextEdit::singleline(&mut self.navigation_search)
				.id(search_id)
				.hint_text(crate::i18n::translate("server-settings-search"))
				.char_limit(100)
				.desired_width(ui.available_width()),
		);
		if search.changed() {
			self.navigation_cursor = 0;
		}
		let results = destinations(state, guild, &self.navigation_search);
		self.navigation_cursor = self.navigation_cursor.min(results.len().saturating_sub(1));
		let mut chosen = None;
		if !results.is_empty() {
			if down {
				self.navigation_cursor = (self.navigation_cursor + 1) % results.len();
			}
			if up {
				self.navigation_cursor =
					(self.navigation_cursor + results.len() - 1) % results.len();
			}
			if enter {
				chosen = results.get(self.navigation_cursor).copied();
			}
		}

		if compact {
			egui::ComboBox::from_id_salt(("server-settings-page-picker", guild))
				.width(ui.available_width())
				.selected_text(crate::i18n::translate_if_key(self.page.label()))
				.show_ui(ui, |ui| {
					self.destination_rows(
						ui,
						state,
						guild,
						&results,
						search.has_focus(),
						&mut chosen,
					)
				});
		} else if self.page == Page::Roles
			&& self.roles.editing()
			&& !self.show_all_pages
			&& self.navigation_search.trim().is_empty()
		{
			if ui
				.button(crate::i18n::translate("server-settings-all-pages"))
				.clicked()
			{
				self.show_all_pages = true;
			}
			self.roles.navigation(ui, state, guild, commands);
		} else {
			self.destination_rows(ui, state, guild, &results, search.has_focus(), &mut chosen);
		}
		let previous = self.page;
		if let Some(target) = chosen
			&& self.select_page(target.page, state)
		{
			if previous != self.page {
				state.retire_server_admin_read();
			}
			if let Some(anchor) = target.anchor {
				ui.ctx().data_mut(|data| {
					data.insert_temp(
						egui::Id::unique(("server-settings-search-anchor", guild)),
						anchor,
					)
				});
			}
			search.surrender_focus();
		}
		if let Some(error) = self.navigation_error {
			dialog::notice(ui, dialog::Level::Warning, error);
		}
		if self.navigation_search.is_empty() && state.can_delete_server(guild) {
			ui.add_space(10.0);
			ui.separator();
			if dialog::danger_nav_item(ui, "server-settings-delete-server-button-delete-server")
				.clicked()
			{
				state.clear_server_action_result(guild);
				self.delete = true;
				self.delete_name.clear();
			}
		}
	}
	fn destination_rows(
		&self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		results: &[Destination],
		search_focused: bool,
		chosen: &mut Option<Destination>,
	) {
		if results.is_empty() {
			ui.weak(crate::i18n::translate("server-settings-no-results"));
			return;
		}
		let colors = design::palette(ui);
		let mut section = None;
		for (index, target) in results.iter().enumerate() {
			if section != Some(target.page.section()) {
				section = Some(target.page.section());
				ui.add_space(10.0);
				ui.label(design::eyebrow(
					ui,
					crate::i18n::translate_if_key(target.page.section().label()),
					colors.muted,
				));
			}
			let selected = if search_focused && !self.navigation_search.trim().is_empty() {
				index == self.navigation_cursor
			} else {
				target.page == self.page && target.anchor.is_none()
			};
			let label = target.page.activity_label(state, guild, target.label);
			if crate::settings::nav_item(ui, label.as_deref().unwrap_or(target.label), selected)
				.clicked()
			{
				*chosen = Some(*target);
			}
		}
	}
	fn page_has_draft(&self) -> bool {
		match self.page {
			Page::Applications | Page::Access => self.access.has_changes(),
			Page::Safety => self.access.has_changes(),
			Page::Bans => self.moderation.has_changes(),
			Page::Onboarding | Page::ChannelsRoles | Page::Screening | Page::Guide => {
				self.community.has_changes()
			}
			Page::AutoMod => self.automod.has_changes(),
			Page::Moderation => self.moderation.has_changes(),
			Page::Members | Page::Emoji => self.admin.has_changes(),
			Page::Stickers => self.stickers.has_changes(),
			Page::Soundboard => self.sounds.has_changes(),
			Page::Roles => self.roles.has_changes(),
			Page::Integrations => self.integrations.has_changes(),
			Page::Invites => self.invites.busy() || self.invites.overlay_open(),
			_ => false,
		}
	}
	fn select_page(&mut self, page: Page, state: &State) -> bool {
		if self
			.scope
			.is_none_or(|(_, guild)| !page.allowed(state, guild))
		{
			return false;
		}
		if page == self.page {
			return true;
		}
		if self.page_has_draft() || state.server_admin.saving || state.server_settings.saving {
			self.navigation_error = Some("server-settings-save-before-navigation");
			return false;
		}
		self.page = page;
		self.show_all_pages = false;
		self.navigation_error = None;
		true
	}

	/// The server's icon, name and member count above the page list.
	fn navigation_header(
		&self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		avatars: &mut Avatars,
	) {
		let colors = design::palette(ui);
		let Some(known) = state.guild(guild) else {
			return;
		};
		let mut shown = known.clone();
		if let Some(draft) = &self.draft {
			shown.name.clone_from(&draft.name);
		}
		ui.horizontal(|ui| {
			ui.spacing_mut().item_spacing.x = 10.0;
			if let Some(texture) = &self.icon_preview {
				let (rect, _) =
					ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
				egui::Image::from_texture(texture)
					.corner_radius(12)
					.paint_at(ui, rect);
			} else {
				if matches!(self.icon, Patch::Null) {
					shown.icon = None;
				}
				let (rect, _) =
					ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
				avatars.paint_guild(ui, &shown, rect, state.demo, 12);
			}
			ui.vertical(|ui| {
				ui.spacing_mut().item_spacing.y = 0.0;
				ui.add(
					egui::Label::new(
						design::semibold(ui, &shown.name, 15.0).color(colors.text_strong),
					)
					.truncate(),
				);
				let members = self.draft.as_ref().and_then(|draft| draft.member_count);
				let subtitle = members.map_or_else(
					|| crate::i18n::translate("server-settings-nav-title"),
					|count| {
						format!(
							"{count} {}",
							crate::i18n::translate("server-settings-preview-members")
						)
					},
				);
				ui.add(
					egui::Label::new(egui::RichText::new(subtitle).size(12.0).color(colors.muted))
						.truncate(),
				);
			});
		});
	}

	fn delete_dialog(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		guild: Id,
		commands: &mut Vec<Command>,
	) {
		let Some(name) = state.guild(guild).map(|guild| guild.name.clone()) else {
			self.delete = false;
			return;
		};
		if !state.can_delete_server(guild) {
			self.delete = false;
			return;
		}
		let pending = state.server_action_pending();
		let reason = state.delete_server_reason(guild);
		let mut delete = false;
		let mut close = false;
		let response = dialog::Dialog::new(
			"delete-server",
			format!(
				"{} '{name}'",
				crate::i18n::translate("server-settings-delete-dialog-delete")
			),
		)
		.subtitle(format!(
			"{} {name}? {}",
			crate::i18n::translate("server-settings-delete-dialog-are-you-sure-you-want-to-delete"),
			crate::i18n::translate("server-settings-delete-dialog-this-action-cannot-be-undone")
		))
		.danger()
		.width(520.0)
		.show(ctx, |d| {
			d.content(|ui| {
				let label = dialog::label(ui, "server-settings-delete-dialog-enter-server-name");
				dialog::input(
					ui,
					egui::TextEdit::singleline(&mut self.delete_name)
						.align(egui::Align2::LEFT_CENTER)
						.char_limit(100)
						.desired_width(f32::INFINITY),
				)
				.labelled_by(label.id);
				if let Some(reason) = reason {
					dialog::notice(ui, dialog::Level::Warning, reason);
				} else if let Some(status) = state.server_action_status(guild) {
					dialog::notice(ui, dialog::Level::Error, status);
				}
				if state.demo {
					dialog::hint(
						ui,
						"server-settings-delete-dialog-offline-preview-no-server-changes",
					);
				}
			});
			d.footer(|ui| {
				ui.add_enabled_ui(
					!pending && reason.is_none() && self.delete_name == name,
					|ui| {
						delete = dialog::action(
							ui,
							if pending {
								"server-settings-delete-dialog-deleting"
							} else {
								"server-settings-delete-dialog-delete-server"
							},
							dialog::Action::Danger,
						)
						.clicked();
					},
				);
				close |= dialog::action(
					ui,
					"server-settings-delete-dialog-cancel",
					dialog::Action::Neutral,
				)
				.clicked();
			});
		});
		if delete && let Some(command) = state.delete_server(guild) {
			commands.push(command);
		}
		if (close || response.close) && !pending {
			self.delete = false;
			self.delete_name.clear();
		}
	}

	/// Pages whose own virtualized list scrolls; they must not sit inside a second scroll area.
	fn scrolling_page(&self) -> bool {
		match self.page {
			Page::AuditLog | Page::Invites => true,
			Page::Integrations => self.integrations.scrolls_itself(),
			_ => false,
		}
	}

	/// One page of settings content, with no scroll chrome of its own.
	fn page_body(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		avatars: &mut Avatars,
		profile: &mut crate::profiles::ProfileSession,
		commands: &mut Vec<Command>,
	) {
		match self.page {
			Page::Applications | Page::Access => {
				self.access.show(
					ui,
					state,
					guild,
					if self.page == Page::Applications {
						crate::server_access::Page::Applications
					} else {
						crate::server_access::Page::Access
					},
					commands,
				);
				return;
			}
			Page::Bans => {
				self.moderation.show_bans(ui, state, guild, commands);
				return;
			}
			Page::Events => {
				design::page_header(
					ui,
					"server-settings-page-events",
					Some("server-settings-events-help"),
					|_| {},
				);
				if ui
					.button(crate::i18n::translate("server-settings-open-events"))
					.clicked()
				{
					if self.dirty()
						|| self.page_has_draft()
						|| state.server_admin.saving
						|| state.server_settings.saving
					{
						self.navigation_error = Some("server-settings-save-before-navigation");
					} else {
						self.events_requested = Some(guild);
					}
				}
				return;
			}
			Page::Onboarding | Page::ChannelsRoles | Page::Screening | Page::Guide => {
				self.community.show(
					ui,
					state,
					guild,
					(
						match self.page {
							Page::Screening => model::community_admin::Section::Screening,
							Page::Guide => model::community_admin::Section::Guide,
							_ => model::community_admin::Section::Onboarding,
						},
						self.page == Page::ChannelsRoles,
					),
					avatars,
					commands,
				);
				return;
			}
			Page::AutoMod => {
				self.automod.show(ui, state, guild, commands);
				return;
			}
			Page::Moderation => {
				self.moderation.show(ui, state, guild, commands);
				return;
			}
			Page::AuditLog => {
				self.audit_log.show(ui, state, guild, avatars, commands);
				if let Some(target) = self.audit_log.target.take() {
					match target {
						crate::server_audit_log::Target::User(id) => {
							if let Some(user) = state
								.server_admin
								.audit_log
								.as_ref()
								.and_then(|page| page.users.iter().find(|user| user.id == id))
							{
								profile.command_open(user.clone());
							}
						}
						crate::server_audit_log::Target::Channel(id) if state.can_view(id) => {
							self.admin.message = Some(id)
						}
						crate::server_audit_log::Target::Role(id)
							if self.select_page(Page::Roles, state) =>
						{
							state.retire_server_admin_read();
							self.roles.select(Some(id), guild);
							if let Some(command) = self.roles.load(state, guild) {
								commands.push(command);
							}
						}
						_ => {}
					}
				}
				return;
			}
			Page::Integrations => {
				self.integrations.show(ui, state, guild, avatars, commands);
				return;
			}
			Page::Invites => {
				self.invites.show(ui, state, guild, avatars, commands);
				return;
			}
			Page::Roles => {
				self.roles.show(ui, state, guild, avatars, commands);
				return;
			}
			Page::Stickers => {
				self.stickers.show(ui, state, guild, avatars, commands);
				return;
			}
			Page::Soundboard => {
				self.sounds.show(ui, state, guild, avatars, commands);
				return;
			}
			Page::Emoji | Page::Members => {
				self.admin.show(
					ui,
					state,
					guild,
					self.page == Page::Members,
					avatars,
					profile,
					commands,
				);
				return;
			}
			Page::Profile | Page::Engagement | Page::Safety => {}
		}
		if let Some(error) = state.server_settings.error {
			dialog::notice(ui, dialog::Level::Error, error);
			if !state.server_settings.pending
				&& ui
					.button(crate::i18n::translate(
						"server-settings-page-body-reload-server-settings",
					))
					.clicked() && let Some(command) = state.load_server_settings(guild)
			{
				commands.push(command);
			}
		}
		if self.draft.is_none() {
			if state.server_settings.pending {
				ui.horizontal(|ui| {
					ui.spinner();
					ui.label(crate::i18n::translate(
						"server-settings-page-body-loading-server-settings",
					));
				});
			} else if !state.gateway_connected && !state.demo {
				ui.weak(crate::i18n::translate(
					"server-settings-page-body-reconnect-to-load-server-settings",
				));
			} else if ui
				.button(crate::i18n::translate(
					"server-settings-page-body-load-server-settings",
				))
				.clicked() && let Some(command) = state.load_server_settings(guild)
			{
				commands.push(command);
			}
			return;
		}
		if self.page == Page::Safety {
			self.access.show(
				ui,
				state,
				guild,
				crate::server_access::Page::Safety,
				commands,
			);
			ui.add_space(24.0);
		}
		ui.add_enabled_ui(!state.server_settings.pending, |ui| {
			if self.page == Page::Profile {
				self.profile(ui, state, avatars);
			} else if let Some(draft) = &mut self.draft {
				if self.page == Page::Safety {
					safety(ui, draft);
				} else {
					engagement(ui, state, draft);
				}
			}
		});
	}

	fn save_bar(&mut self, ui: &mut egui::Ui, state: &mut State, commands: &mut Vec<Command>) {
		let available = !state.server_settings.pending && !self.icon_pending;
		let (save, reset) = design::save_bar(
			ui,
			state
				.server_settings
				.saving
				.then_some("server-settings-save-bar-saving-changes"),
			available
				&& !self.refresh_conflict
				&& !state.server_settings.needs_refresh
				&& (state.demo || state.gateway_connected),
			available,
		);
		if reset {
			if self.refresh_conflict {
				self.accept_latest(state, false);
			} else {
				self.reset();
			}
		}
		if save && let (Some(baseline), Some(draft)) = (&self.baseline, &self.draft) {
			let mut edit = Edit::between(baseline, draft);
			if let Some(traits) = &mut edit.traits {
				traits.retain(|entry| !entry.label.is_empty());
			}
			edit.icon = self.icon.clone();
			if !edit.valid() {
				self.form_error = Some(
					"Use a server name of 2–100 characters, a description of up to 300 characters, and valid traits without control characters.",
				);
				return;
			}
			if let Some(command) = state.save_server_settings(edit) {
				commands.push(command);
				self.submitted = true;
				self.form_error = None;
			} else {
				self.form_error = Some(
					"Could not save these changes. Check the selected channels, reconnect, or reload the server settings and try again.",
				);
			}
		}
		if self.refresh_conflict {
			dialog::notice(ui, dialog::Level::Warning, "server-settings-conflict");
			ui.add_enabled_ui(available, |ui| {
				ui.horizontal_wrapped(|ui| {
					if ui
						.button(crate::i18n::translate("server-settings-conflict-reload"))
						.clicked()
					{
						self.accept_latest(state, false);
					}
					if ui
						.button(crate::i18n::translate("server-settings-conflict-reapply"))
						.clicked()
					{
						self.accept_latest(state, true);
					}
				});
			});
		}
		if state.server_settings.needs_refresh {
			ui.add_space(8.0);
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"server-settings-save-bar-reload-the-server-settings-before-saving-again-your-edits-will",
			);
		}
		if !state.demo && !state.gateway_connected {
			ui.add_space(8.0);
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"server-settings-save-bar-reconnect-to-save-changes",
			);
		}
		if let Some(error) = self.form_error {
			ui.add_space(8.0);
			dialog::notice(ui, dialog::Level::Error, error);
		}
	}

	fn profile(&mut self, ui: &mut egui::Ui, state: &State, avatars: &mut Avatars) {
		design::page_header(
			ui,
			"server-settings-profile-form-server-profile",
			Some(
				"server-settings-profile-form-customize-how-your-server-appears-in-invite-links-and-if",
			),
			|_| {},
		);
		let width = ui.available_width();
		if width >= 700.0 {
			let preview_width = if width >= 820.0 { 300.0 } else { 260.0 };
			let form_width = width - preview_width - 32.0;
			ui.horizontal_top(|ui| {
				ui.spacing_mut().item_spacing.x = 32.0;
				ui.allocate_ui_with_layout(
					egui::vec2(form_width, 0.0),
					egui::Layout::top_down(egui::Align::Min),
					|ui| {
						ui.set_width(form_width);
						self.profile_form(ui);
					},
				);
				ui.vertical(|ui| {
					ui.set_width(preview_width);
					ui.add_space(4.0);
					self.preview(ui, state, avatars);
				});
			});
		} else {
			self.profile_form(ui);
			ui.add_space(28.0);
			self.preview(ui, state, avatars);
		}
	}
	fn profile_form(&mut self, ui: &mut egui::Ui) {
		let Some(draft) = &mut self.draft else {
			return;
		};
		let colors = design::palette(ui);
		// Column spacing must not leak into the swatch, trait and button rows.
		ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
		search_anchor(ui, draft.guild, "name");
		design::group(ui, "server-settings-profile-form-name", |ui| {
			design::input(
				ui,
				egui::TextEdit::singleline(&mut draft.name)
					.align(egui::Align2::LEFT_CENTER)
					.char_limit(100)
					.hint_text(crate::i18n::translate("server-settings-profile-form-name")),
			)
			.on_hover_text(crate::i18n::translate("server-settings-profile-form-name"));
		});
		ui.add_space(16.0);
		search_anchor(ui, draft.guild, "icon");
		let icon_pending = self.icon_pending;
		let removable = draft.icon.is_some() || matches!(self.icon, Patch::Value(_));
		let (change, remove) = design::group(ui, "server-settings-profile-form-icon", |ui| {
			design::hint(
				ui,
				"server-settings-profile-form-we-recommend-an-image-of-at-least-512512",
			);
			ui.add_space(4.0);
			ui.horizontal_wrapped(|ui| icon_buttons(ui, icon_pending, removable))
				.inner
		});
		if change {
			self.icon_requested = true;
			self.icon_pending = true;
			self.icon_error = None;
		}
		if remove {
			self.icon = Patch::Null;
			self.icon_preview = None;
			self.icon_pending = false;
			self.icon_requested = false;
		}
		if let Some(error) = self.icon_error {
			ui.add_space(8.0);
			design::notice(ui, design::Level::Error, error);
		}
		ui.add_space(16.0);
		search_anchor(ui, draft.guild, "banner");
		design::group(ui, "server-settings-profile-form-banner", |ui| {
			ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
			let swatch_width = ((ui.available_width() - 32.0) / 5.0).max(20.0);
			for row in BANNER_SWATCHES.chunks(5) {
				ui.horizontal(|ui| {
					for &color in row {
						banner_swatch(ui, draft, color, swatch_width, &colors);
					}
				});
			}
			design::card_divider(ui);
			let current = draft.banner_color.unwrap_or(BANNER_SWATCHES[0]);
			let mut rgb = [(current >> 16) as u8, (current >> 8) as u8, current as u8];
			design::row(
				ui,
				"server-settings-profile-form-banner-custom",
				Some("server-settings-profile-form-banner-custom-help"),
				|ui| {
					if design::color_edit(ui, &mut rgb).changed() {
						draft.banner_color = Some(u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]));
					}
				},
			);
		});
		ui.add_space(16.0);
		search_anchor(ui, draft.guild, "traits");
		design::group(ui, "server-settings-profile-form-traits", |ui| {
			design::hint(
				ui,
				"server-settings-profile-form-add-up-to-5-traits-to-show-off-your-server",
			);
			ui.add_space(4.0);
			let columns = if ui.available_width() >= 480.0 {
				3
			} else if ui.available_width() >= 330.0 {
				2
			} else {
				1
			};
			let cell_width = (ui.available_width() - 8.0 * (columns - 1) as f32) / columns as f32;
			let mut traits = draft.traits.clone();
			traits.resize_with(5, || Trait {
				label: String::new(),
				emoji: None,
			});
			for (row, cells) in traits.chunks_mut(columns).enumerate() {
				ui.horizontal(|ui| {
					for (column, entry) in cells.iter_mut().enumerate() {
						ui.push_id(("server-trait", row, column), |ui| {
							trait_cell(ui, &mut self.emoji_picker, entry, cell_width, &colors);
						});
					}
				});
			}
			// Keep empty slots in the editor so an emoji can be chosen before typing its label.
			while traits.last().is_some_and(|entry| {
				entry.label.is_empty() && entry.emoji.as_deref().is_none_or(str::is_empty)
			}) {
				traits.pop();
			}
			for entry in &mut traits {
				if entry.emoji.as_deref() == Some("") {
					entry.emoji = None;
				}
			}
			draft.traits = traits;
		});
		ui.add_space(16.0);
		search_anchor(ui, draft.guild, "description");
		design::group(ui, "server-settings-profile-form-description", |ui| {
			design::hint(
				ui,
				"server-settings-profile-form-how-did-your-server-get-started-why-should-people-join",
			);
			ui.add_space(4.0);
			design::input(
				ui,
				egui::TextEdit::multiline(&mut draft.description)
					.hint_text(crate::i18n::translate(
						"server-settings-profile-form-tell-the-world-a-bit-about-this-server",
					))
					.char_limit(300)
					.desired_width(f32::INFINITY)
					.desired_rows(4),
			)
			.on_hover_text(crate::i18n::translate(
				"server-settings-profile-form-description",
			));
			let used = draft.description.chars().count();
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
				ui.label(egui::RichText::new(format!("{used}/300")).size(12.0).color(
					if used >= 280 {
						colors.warning
					} else {
						colors.muted
					},
				));
			});
		});
		ui.add_space(16.0);
		let guild = draft.guild;
		design::group(ui, "server-settings-profile-server-id", |ui| {
			design::row(
				ui,
				&guild.0.to_string(),
				Some("server-settings-profile-server-id-help"),
				|ui| {
					if design::button(
						ui,
						"server-settings-profile-copy-id",
						design::ButtonKind::Outline,
					)
					.clicked()
					{
						ui.ctx().copy_text(guild.0.to_string());
					}
				},
			);
		});
	}
	fn preview(&self, ui: &mut egui::Ui, state: &State, avatars: &mut Avatars) {
		let Some(draft) = &self.draft else {
			return;
		};
		let colors = design::palette(ui);
		ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
		ui.style_mut().override_font_id = Some(egui::FontId::proportional(13.0));
		let width = ui.available_width().min(300.0) - 2.0;
		egui::Frame::new()
			.fill(colors.raised)
			.stroke(egui::Stroke::new(1.0, colors.border))
			.corner_radius(16)
			.show(ui, |ui| {
				ui.set_width(width);
				let (banner, _) =
					ui.allocate_exact_size(egui::vec2(width, 125.0), egui::Sense::hover());
				gradient(ui, banner, draft.banner_color.unwrap_or(0x2153dc), 16);
				egui::Frame::new().inner_margin(16).show(ui, |ui| {
					ui.set_width(width - 32.0);
					let icon_rect = egui::Rect::from_min_size(
						egui::pos2(ui.cursor().left(), banner.bottom() - 36.0),
						egui::Vec2::splat(72.0),
					);
					ui.painter()
						.rect_filled(icon_rect.expand(4.0), 20, colors.raised);
					if let Some(texture) = &self.icon_preview {
						egui::Image::from_texture(texture)
							.corner_radius(16)
							.paint_at(ui, icon_rect);
					} else if let Some(guild) = state.guild(draft.guild) {
						let mut guild = guild.clone();
						guild.name.clone_from(&draft.name);
						if matches!(self.icon, Patch::Null) {
							guild.icon = None;
						}
						ui.scope_builder(egui::UiBuilder::new().max_rect(icon_rect), |ui| {
							avatars.show_guild_sized(ui, &guild, true, state.demo, 72.0);
						});
					}
					ui.advance_cursor_after_rect(icon_rect);
					ui.label(design::semibold(ui, &draft.name, 16.0));
					ui.horizontal_wrapped(|ui| {
						if let Some(count) = draft.online_count {
							ui.colored_label(
								colors.positive,
								format!(
									"● {count} {}",
									crate::i18n::translate("server-settings-preview-online")
								),
							);
						}
						if let Some(count) = draft.member_count {
							ui.weak(format!(
								"● {count} {}",
								crate::i18n::translate("server-settings-preview-members")
							));
						}
					});
					let seconds = ((draft.guild.0 >> 22) + 1_420_070_400_000) / 1000;
					if let Ok(date) = time::OffsetDateTime::from_unix_timestamp(seconds as i64) {
						let date = crate::local_time::local(date);
						ui.weak(format!(
							"{} {} {}",
							crate::i18n::translate("server-settings-preview-established"),
							date.month(),
							date.year()
						));
					}
					ui.horizontal_wrapped(|ui| {
						for entry in &draft.traits {
							if !entry.label.is_empty() {
								ui.add(
									egui::Button::new(format!(
										"{}{}{}",
										entry.emoji.as_deref().unwrap_or(""),
										if entry.emoji.is_some() { " " } else { "" },
										entry.label
									))
									.wrap()
									.sense(egui::Sense::hover())
									.fill(Color32::TRANSPARENT)
									.stroke(egui::Stroke::new(1.0, colors.border))
									.corner_radius(20),
								);
							}
						}
					});
					if !draft.description.is_empty() {
						ui.add_space(8.0);
						ui.label(&draft.description);
					}
				});
			});
	}
}

fn settings_conflict(
	before: &Settings,
	draft: &Settings,
	actual: &Settings,
	icon: &Patch<String>,
) -> bool {
	macro_rules! field {
		($field:ident) => {
			before.$field != draft.$field
				&& before.$field != actual.$field
				&& draft.$field != actual.$field
		};
	}
	field!(name)
		|| field!(banner_color)
		|| field!(traits)
		|| field!(description)
		|| field!(system_channel_id)
		|| field!(system_channel_flags)
		|| field!(activity_feed)
		|| field!(default_message_notifications)
		|| field!(afk_channel_id)
		|| field!(afk_timeout)
		|| field!(verification_level)
		|| field!(explicit_content_filter)
		|| (!matches!(icon, Patch::Absent)
			&& before.icon != actual.icon
			&& !(matches!(icon, Patch::Null) && actual.icon.is_none()))
}

const BANNER_SWATCHES: [u32; 10] = [
	0x2153dc, 0xf916a0, 0xed171a, 0xef7912, 0xf1cd29, 0x763a94, 0x04adf1, 0x46dcca, 0x496b00,
	0x282828,
];

/// "Change Server Icon" and "Remove Icon"; returns which one was clicked.
fn icon_buttons(ui: &mut egui::Ui, pending: bool, removable: bool) -> (bool, bool) {
	let change = ui
		.add_enabled_ui(!pending, |ui| {
			design::button(
				ui,
				if pending {
					"server-settings-profile-form-preparing-icon"
				} else {
					"server-settings-profile-form-change-server-icon"
				},
				design::ButtonKind::Primary,
			)
		})
		.inner
		.clicked();
	let remove = ui
		.add_enabled_ui(removable, |ui| {
			design::button(
				ui,
				"server-settings-profile-form-remove-icon",
				design::ButtonKind::Outline,
			)
		})
		.inner
		.clicked();
	(change, remove)
}

fn banner_swatch(
	ui: &mut egui::Ui,
	draft: &mut Settings,
	color: u32,
	width: f32,
	colors: &design::Palette,
) {
	let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::click());
	let selected = draft.banner_color == Some(color);
	let lift = ui.ctx().animate_bool_with_time(
		response.id.with("lift"),
		response.hovered() || response.has_focus(),
		ui.style().animation_time,
	);
	gradient(ui, rect.shrink(2.0 * (1.0 - lift)), color, 8);
	if selected || response.has_focus() {
		ui.painter().rect_stroke(
			rect.expand(3.0),
			10,
			egui::Stroke::new(
				2.0,
				if selected {
					colors.text_strong
				} else {
					colors.accent
				},
			),
			egui::StrokeKind::Outside,
		);
	}
	if selected {
		let badge = egui::Rect::from_center_size(
			rect.right_top() + egui::vec2(-14.0, 14.0),
			egui::Vec2::splat(18.0),
		);
		ui.painter()
			.circle_filled(badge.center(), 9.0, Color32::from_black_alpha(140));
		crate::icons::paint(
			ui.painter(),
			crate::icons::Icon::Check,
			badge.shrink(3.0),
			Color32::WHITE,
		);
	}
	let name = format!("Banner color #{color:06X}");
	response.widget_info(|| {
		egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), selected, &name)
	});
	if response.clicked() {
		draft.banner_color = Some(color);
	}
	response.on_hover_text(name);
}

fn trait_cell(
	ui: &mut egui::Ui,
	picker: &mut crate::emoji_picker::Picker,
	entry: &mut Trait,
	cell_width: f32,
	colors: &design::Palette,
) {
	let filled = !entry.label.is_empty();
	egui::Frame::new()
		.fill(colors.base)
		.stroke(egui::Stroke::new(
			1.0,
			if filled {
				colors.border
			} else {
				colors.border.gamma_multiply(0.6)
			},
		))
		.corner_radius(8)
		.inner_margin(8)
		.show(ui, |ui| {
			ui.set_width((cell_width - 18.0).max(60.0));
			ui.horizontal(|ui| {
				picker.unicode_button(ui, &mut entry.emoji);
				ui.add(
					egui::TextEdit::singleline(&mut entry.label)
						.align(egui::Align2::LEFT_CENTER)
						.char_limit(100)
						.hint_text(crate::i18n::translate(
							"server-settings-profile-form-trait-name",
						))
						.desired_width((cell_width - 90.0).max(24.0))
						.frame(egui::Frame::NONE),
				)
				.on_hover_text(crate::i18n::translate(
					"server-settings-profile-form-trait-name",
				));
				if filled
					&& crate::icons::button(
						ui,
						crate::icons::Icon::Close,
						18.0,
						&crate::i18n::translate("server-settings-profile-form-remove-trait"),
					)
					.clicked()
				{
					entry.label.clear();
					entry.emoji = None;
				}
			});
		});
}
fn gradient(ui: &mut egui::Ui, rect: egui::Rect, color: u32, radius: u8) {
	let top = Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8);
	let bottom = top.lerp_to_gamma(Color32::WHITE, 0.38);
	ui.painter().rect_filled(rect, radius, top);
	// Match the profile-card gradient: rounded solid caps and an interpolated middle band.
	ui.painter().rect_filled(
		egui::Rect::from_min_max(
			egui::pos2(rect.left(), rect.bottom() - f32::from(radius)),
			rect.right_bottom(),
		),
		egui::CornerRadius {
			nw: 0,
			ne: 0,
			sw: radius,
			se: radius,
		},
		bottom,
	);
	let band = egui::Rect::from_min_max(
		egui::pos2(rect.left(), rect.top() + f32::from(radius) - 1.0),
		egui::pos2(rect.right(), rect.bottom() - f32::from(radius) + 1.0),
	);
	let mut mesh = egui::Mesh::default();
	mesh.colored_vertex(band.left_top(), top);
	mesh.colored_vertex(band.right_top(), top);
	mesh.colored_vertex(band.left_bottom(), bottom);
	mesh.colored_vertex(band.right_bottom(), bottom);
	mesh.add_triangle(0, 1, 2);
	mesh.add_triangle(1, 3, 2);
	ui.painter().add(egui::Shape::mesh(mesh));
}
/// System message toggles: the flag bit is set when the message is suppressed.
const SYSTEM_MESSAGES: [(u64, &str); 4] = [
	(0, "server-settings-engagement-system-welcome"),
	(3, "server-settings-engagement-system-welcome-sticker"),
	(1, "server-settings-engagement-system-boost"),
	(2, "server-settings-engagement-system-tips"),
];
/// Controls sit at a fixed width on the right of a [`design::row`].
const PICKER_WIDTH: f32 = 240.0;

fn engagement(ui: &mut egui::Ui, state: &State, draft: &mut Settings) {
	ui.set_max_width(ui.available_width().min(850.0));
	design::page_header(
		ui,
		"server-settings-engagement-engagement",
		Some("server-settings-engagement-manage-settings-that-help-keep-your-server-active"),
		|_| {},
	);
	search_anchor(ui, draft.guild, "system");
	design::section(
		ui,
		"server-settings-engagement-system-messages",
		Some("server-settings-engagement-configure-system-event-messages-sent-to-your-server"),
	);
	design::card(ui, |ui| {
		for (index, (bit, text)) in SYSTEM_MESSAGES.into_iter().enumerate() {
			if index > 0 {
				design::card_divider(ui);
			}
			let mask = 1 << bit;
			let mut enabled = draft.system_channel_flags & mask == 0;
			if design::switch(ui, text, None, &mut enabled).changed() {
				if enabled {
					draft.system_channel_flags &= !mask;
				} else {
					draft.system_channel_flags |= mask;
				}
			}
		}
		design::card_divider(ui);
		design::row(
			ui,
			"server-settings-engagement-system-messages-channel",
			Some("server-settings-engagement-this-is-the-channel-we-send-system-event-messages-to"),
			|ui| channel_picker(ui, state, draft.guild, &mut draft.system_channel_id, false),
		);
	});
	ui.add_space(24.0);
	search_anchor(ui, draft.guild, "activity");
	design::section(
		ui,
		"server-settings-engagement-activity-feed-settings",
		Some("server-settings-engagement-shows-a-feed-of-activity-from-games-and-connected-apps"),
	);
	design::card(ui, |ui| {
		let mut enabled = draft.activity_feed.unwrap_or(false);
		if design::switch(
			ui,
			"server-settings-engagement-display-activity-feed-in-this-server",
			draft
				.activity_feed
				.is_none()
				.then_some("server-settings-engagement-server-default"),
			&mut enabled,
		)
		.changed()
		{
			draft.activity_feed = Some(enabled);
		}
	});
	ui.add_space(24.0);
	search_anchor(ui, draft.guild, "notifications");
	design::section(
		ui,
		"server-settings-engagement-default-notification-settings",
		Some(
			"server-settings-engagement-this-will-determine-whether-members-who-have-not-explicitly-set",
		),
	);
	design::card(ui, |ui| {
		for (value, label, detail) in [
			(0, "server-settings-engagement-all-messages", None),
			(
				1,
				"server-settings-engagement-only-mentions",
				Some(
					"server-settings-engagement-we-highly-recommend-setting-this-to-only-mentions-for-a",
				),
			),
		] {
			let selected = draft.default_message_notifications == value;
			if design::radio_row(ui, selected, label, detail).clicked() {
				draft.default_message_notifications = value;
			}
		}
	});
	ui.add_space(24.0);
	search_anchor(ui, draft.guild, "inactive");
	design::section(
		ui,
		"server-settings-engagement-inactive-channel",
		Some(
			"server-settings-engagement-automatically-move-members-to-this-channel-and-mute-them-when",
		),
	);
	design::card(ui, |ui| {
		design::row(
			ui,
			"server-settings-engagement-inactive-channel",
			None,
			|ui| channel_picker(ui, state, draft.guild, &mut draft.afk_channel_id, true),
		);
		design::card_divider(ui);
		design::row(
			ui,
			"server-settings-engagement-inactive-timeout",
			None,
			|ui| {
				ui.add_enabled_ui(draft.afk_channel_id.is_some(), |ui| {
					timeout_picker(ui, &mut draft.afk_timeout);
				});
			},
		);
	});
}

/// Verification levels with their documented member requirements.
const VERIFICATION_LEVELS: [(&str, &str); 5] = [
	(
		"server-settings-safety-verification-none",
		"server-settings-safety-verification-none-detail",
	),
	(
		"server-settings-safety-verification-low",
		"server-settings-safety-verification-low-detail",
	),
	(
		"server-settings-safety-verification-medium",
		"server-settings-safety-verification-medium-detail",
	),
	(
		"server-settings-safety-verification-high",
		"server-settings-safety-verification-high-detail",
	),
	(
		"server-settings-safety-verification-highest",
		"server-settings-safety-verification-highest-detail",
	),
];
const CONTENT_FILTERS: [&str; 3] = [
	"server-settings-safety-filter-disabled",
	"server-settings-safety-filter-no-roles",
	"server-settings-safety-filter-all",
];

fn safety(ui: &mut egui::Ui, draft: &mut Settings) {
	ui.set_max_width(ui.available_width().min(850.0));
	design::page_header(
		ui,
		"server-settings-page-safety",
		Some("server-settings-safety-subtitle"),
		|_| {},
	);
	let community = draft.community();
	if community {
		design::notice(
			ui,
			design::Level::Info,
			"server-settings-safety-community-note",
		);
		ui.add_space(16.0);
	}
	search_anchor(ui, draft.guild, "verification");
	design::section(
		ui,
		"server-settings-safety-verification",
		Some("server-settings-safety-verification-help"),
	);
	design::card(ui, |ui| {
		for (level, (label, detail)) in (0u8..).zip(VERIFICATION_LEVELS) {
			let selected = draft.verification_level == level;
			let allowed = !community || level >= 1;
			if ui
				.add_enabled_ui(allowed, |ui| {
					design::radio_row(ui, selected, label, Some(detail))
				})
				.inner
				.clicked()
			{
				draft.verification_level = level;
			}
		}
	});
	ui.add_space(24.0);
	search_anchor(ui, draft.guild, "filter");
	design::section(
		ui,
		"server-settings-safety-filter",
		Some("server-settings-safety-filter-help"),
	);
	design::card(ui, |ui| {
		for (level, label) in (0u8..).zip(CONTENT_FILTERS) {
			let selected = draft.explicit_content_filter == level;
			let allowed = !community || level == model::server_settings::MAX_CONTENT_FILTER;
			if ui
				.add_enabled_ui(allowed, |ui| design::radio_row(ui, selected, label, None))
				.inner
				.clicked()
			{
				draft.explicit_content_filter = level;
			}
		}
	});
}

fn channel_picker(
	ui: &mut egui::Ui,
	state: &State,
	guild: Id,
	selected: &mut Option<Id>,
	voice: bool,
) {
	let choices: Vec<_> = state
		.channels
		.iter()
		.filter(|channel| {
			channel.guild == Some(guild)
				&& state.can_view(channel.id)
				&& if voice {
					channel.kind == 2
				} else {
					matches!(channel.kind, 0 | 5)
				}
		})
		.collect();
	let muted = design::palette(ui).muted;
	let current = selected.and_then(|id| choices.iter().find(|channel| channel.id == id));
	let label = current.map_or_else(
		|| {
			crate::i18n::translate_if_key(if selected.is_some() {
				"server-settings-channel-picker-unavailable-channel"
			} else if voice {
				"server-settings-channel-picker-no-inactive-channel"
			} else {
				"server-settings-channel-picker-no-system-messages-channel"
			})
		},
		|channel| channel.name.clone(),
	);
	let mut label = egui::Atoms::new(label);
	if let Some(channel) = current {
		label.push_left(crate::icons::atom(
			crate::icons::channel(channel.kind),
			16.0,
			muted,
		));
	}
	let empty = choices.is_empty();
	let response = egui::ComboBox::from_id_salt(("server-channel", voice))
		.selected_text(label)
		.width(PICKER_WIDTH.min(ui.available_width()))
		.show_ui(ui, |ui| {
			ui.selectable_value(
				selected,
				None,
				crate::i18n::translate("server-settings-channel-picker-none"),
			);
			for channel in choices {
				ui.selectable_value(
					selected,
					Some(channel.id),
					(
						crate::icons::atom(crate::icons::channel(channel.kind), 16.0, muted),
						channel.name.as_str(),
					),
				);
			}
		})
		.response;
	if empty {
		response.on_hover_text(crate::i18n::translate(
			"server-settings-channel-picker-no-accessible-channels-available",
		));
	}
}
fn timeout_picker(ui: &mut egui::Ui, timeout: &mut u32) {
	egui::ComboBox::from_id_salt("server-afk-timeout")
		.selected_text(format!(
			"{} {}",
			*timeout / 60,
			crate::i18n::translate("server-settings-timeout-picker-minutes")
		))
		.width(PICKER_WIDTH.min(ui.available_width()))
		.show_ui(ui, |ui| {
			for seconds in [60, 300, 900, 1800, 3600] {
				ui.selectable_value(
					timeout,
					seconds,
					format!(
						"{} {}",
						seconds / 60,
						crate::i18n::translate("server-settings-timeout-picker-minutes")
					),
				);
			}
		});
}

#[cfg(test)]
mod navigation_tests {
	use super::*;
	#[test]
	fn application_updates_show_a_neutral_marker_only_when_the_count_is_unknown() {
		let (mut state, guild, _) = fixture();
		state.server_admin.guild = Some(guild);
		state.server_admin.access.applications_changed = true;
		state.server_admin.access.pending_count = None;
		for page in [Page::Members, Page::Applications] {
			assert!(
				page.activity_label(&state, guild, page.label())
					.unwrap()
					.ends_with('•')
			);
			assert!(
				page.activity_label(&state, Id(999_999), page.label())
					.is_none()
			);
		}
		assert!(
			Page::Roles
				.activity_label(&state, guild, Page::Roles.label())
				.is_none()
		);
		state.server_admin.access.pending_count = Some(0);
		assert!(
			Page::Applications
				.activity_label(&state, guild, Page::Applications.label())
				.is_none()
		);
		assert!(
			Page::Members
				.activity_label(&state, guild, Page::Members.label())
				.is_none()
		);
		state.server_admin.access.pending_count = Some(100);
		assert!(
			Page::Applications
				.activity_label(&state, guild, Page::Applications.label())
				.unwrap()
				.ends_with("(99+)")
		);
		state.server_admin.access.pending_count = None;
		state.server_admin.access.applications_changed = false;
		assert!(
			Page::Applications
				.activity_label(&state, guild, Page::Applications.label())
				.is_none()
		);
		state.server_admin.access.applications_changed = true;
		state.permissions.guilds.clear();
		state.permissions.clear_cache();
		assert!(
			Page::Members
				.activity_label(&state, guild, Page::Members.label())
				.is_none()
		);
	}
	fn fixture() -> (State, Id, Editor) {
		crate::i18n::set_current(crate::i18n::Language::English);
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let mut permissions = test_support::permission_snapshot(&state);
		for entry in &mut permissions.guilds {
			entry.owner = state.user.as_ref().map(|user| user.id);
		}
		state.permissions.replace(permissions).unwrap();
		let baseline = Settings {
			guild,
			name: "Synthetic server".into(),
			..Settings::default()
		};
		let editor = Editor {
			scope: Some((state.generation, guild)),
			draft: Some(baseline.clone()),
			baseline: Some(baseline),
			..Default::default()
		};
		(state, guild, editor)
	}
	fn key(key: egui::Key) -> egui::Event {
		egui::Event::Key {
			key,
			physical_key: None,
			pressed: true,
			repeat: false,
			modifiers: egui::Modifiers::NONE,
		}
	}
	fn frame(
		ctx: &egui::Context,
		editor: &mut Editor,
		state: &mut State,
		width: f32,
		compact: bool,
		events: Vec<egui::Event>,
	) -> egui::Rect {
		let guild = editor.guild().unwrap();
		let mut rect = egui::Rect::NOTHING;
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(width, 1400.0),
				)),
				events,
				..Default::default()
			},
			|ui| {
				ui.set_width(width);
				editor.navigation(
					ui,
					state,
					guild,
					compact,
					&mut Avatars::default(),
					&mut vec![],
				);
				rect = ui.min_rect();
			},
		);
		output.drop_without_applying_deltas();
		rect
	}
	#[test]
	fn settings_search_returns_authorized_destinations_and_setting_anchors() {
		let (mut state, guild, _) = fixture();
		assert!(
			destinations(&state, guild, "nickname")
				.iter()
				.any(|row| row.page == Page::Members)
		);
		assert!(
			destinations(&state, guild, "verification")
				.iter()
				.any(|row| row.anchor == Some("verification"))
		);
		assert_eq!(destinations(&state, guild, "").len(), PAGES.len());
		state.permissions.guilds.clear();
		state.permissions.clear_cache();
		assert!(destinations(&state, guild, "permission").is_empty());
	}
	#[test]
	fn navigation_search_keyboard_skips_headers_and_compact_picker_stays_short() {
		for compact in [false, true] {
			let (mut state, guild, mut editor) = fixture();
			let ctx = egui::Context::default();
			design::apply(&ctx);
			editor.navigation_search = "filter".into();
			frame(&ctx, &mut editor, &mut state, 300.0, compact, vec![]);
			ctx.memory_mut(|memory| {
				memory.request_focus(egui::Id::unique(("server-settings-search", guild)))
			});
			frame(
				&ctx,
				&mut editor,
				&mut state,
				300.0,
				compact,
				vec![key(egui::Key::ArrowDown)],
			);
			frame(
				&ctx,
				&mut editor,
				&mut state,
				300.0,
				compact,
				vec![key(egui::Key::Enter)],
			);
			assert_eq!(editor.page, Page::Safety);
			assert_eq!(
				ctx.data(|data| data.get_temp::<&'static str>(egui::Id::unique((
					"server-settings-search-anchor",
					guild
				)))),
				Some("filter")
			);
			if compact {
				editor.navigation_search.clear();
				let rect = frame(&ctx, &mut editor, &mut state, 300.0, compact, vec![]);
				assert!(rect.width() <= 301.0 && rect.height() < 160.0, "{rect:?}");
			}
		}
	}
	#[test]
	fn navigation_ime_enter_and_pending_saves_preserve_current_page_and_draft() {
		let (mut state, guild, mut editor) = fixture();
		let ctx = egui::Context::default();
		design::apply(&ctx);
		editor.navigation_search = "roles".into();
		frame(&ctx, &mut editor, &mut state, 300.0, true, vec![]);
		ctx.memory_mut(|memory| {
			memory.request_focus(egui::Id::unique(("server-settings-search", guild)))
		});
		editor.navigation_ime = true;
		frame(
			&ctx,
			&mut editor,
			&mut state,
			300.0,
			true,
			vec![key(egui::Key::Enter)],
		);
		assert_eq!(editor.page, Page::Profile);
		editor.draft.as_mut().unwrap().description = "Unsaved description".into();
		state.server_admin.saving = true;
		assert!(!editor.select_page(Page::Roles, &state));
		assert_eq!(editor.page, Page::Profile);
		state.server_admin.saving = false;
		assert!(editor.select_page(Page::Engagement, &state));
		assert_eq!(
			editor.draft.as_ref().unwrap().description,
			"Unsaved description"
		);
		state.permissions.guilds.clear();
		state.permissions.clear_cache();
		assert!(!editor.select_page(Page::AuditLog, &state));
	}
	#[test]
	fn concurrent_server_settings_edits_require_deliberate_reapplication() {
		let (mut state, guild, mut editor) = fixture();
		let before = editor.baseline.as_ref().unwrap().clone();
		editor.draft.as_mut().unwrap().name = "My server name".into();
		let mut latest = before.clone();
		latest.description = "Changed elsewhere".into();
		assert!(!settings_conflict(
			&before,
			editor.draft.as_ref().unwrap(),
			&latest,
			&Patch::Absent
		));
		latest.name = "Another server name".into();
		assert!(settings_conflict(
			&before,
			editor.draft.as_ref().unwrap(),
			&latest,
			&Patch::Absent
		));
		state.server_settings.guild = Some(guild);
		state.server_settings.snapshot = Some(latest.clone());
		editor.refresh_conflict = true;
		editor.accept_latest(&state, true);
		assert!(!editor.refresh_conflict);
		assert_eq!(editor.draft.as_ref().unwrap().name, "My server name");
		assert_eq!(
			editor.draft.as_ref().unwrap().description,
			"Changed elsewhere"
		);
		assert_eq!(editor.baseline, Some(latest.clone()));
		editor.refresh_conflict = true;
		editor.accept_latest(&state, false);
		assert_eq!(editor.draft, Some(latest));
		assert!(!editor.dirty());
	}
}
