//! Single UI-thread state owner. Adapters deliver generation-tagged typed events.
mod commands;
mod events;
mod history;
mod member_list;
mod messages;
mod navigation;
mod outgoing;
mod reducer;
mod startup;
pub use commands::Command;
pub use events::{Envelope, Event};
use startup::prepare_navigation;
pub use startup::{PreparedStartup, Startup};
pub mod application_commands;
pub mod archives;
pub mod auth;
pub mod captcha;
pub mod channel_actions;
pub mod community;
pub mod fingerprint;
pub mod forum;
pub mod game_mentions;
pub mod gifs;
pub mod guild_creation;
pub mod guild_folders;
pub mod inbox;
pub mod permissions;
pub mod reporting;
pub mod soundboard;
pub mod stickers;
pub use permissions::ChannelAccess;
#[cfg(test)]
mod permissions_tests;

pub mod account_settings;
pub mod admin_resources;
mod forwarding;
pub mod interactions;
pub mod invites;
pub mod member_identity;
pub mod member_search;
pub mod message_actions;
pub mod messaging_permissions;
pub mod mfa;
pub mod notifications;
pub mod owned_cosmetics;
mod personal_channels;
pub mod polls;
pub mod preferences_sync;
pub mod preload;
pub mod presence;
pub mod profile;
pub mod reactions;
pub mod read_state;
mod replies;
pub mod reply_previews;
pub mod server_profile;
pub use replies::{HistoryNavigation, ParkedReplies, Reply, ReplyDeletions};
pub mod community_admin;
pub mod group_actions;
pub mod member_guide;
pub mod onboarding;
pub mod resident;
pub mod screen;
pub mod search;
pub mod server_access;
pub mod server_actions;
pub mod server_admin;
pub mod server_audit_log;
pub mod server_integrations;
mod server_moderation;
pub mod server_roles;
pub mod server_settings;
mod thread_starter;
mod threads;
mod trail;
#[doc(hidden)]
pub use trail::Trail;
pub mod typing;
pub mod user_actions;
pub mod verification;
mod view_revisions;
pub mod voice;
use model::*;
use session_cache::Timeline;
use std::collections::{BTreeMap, BTreeSet};
use trail::Place;

pub const MAX_DRAFT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CONTENT: usize = 2000;
/// Nitro and Nitro Classic accounts may send longer messages.
pub const MAX_PREMIUM_CONTENT: usize = model::message_options::MAX_PREMIUM_CONTENT;
/// Discord accepts at most ten attachments per message.
pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_NAV: usize = model::account::MAX_ENTRIES;
/// Mark-all-read pairs per command (16 KiB); the rest stay unread for another choice.
pub const MAX_BULK_ACK: usize = 1024;
const MAX_VIEWED_SERVERS: usize = 1024;
pub const MAX_MEMBER_PRESENCE_BYTES: usize = 128 * 1024;
pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
pub const EVENT_SLOTS: usize = 8; // UI drain batch; reliable events share a 32 MiB byte budget.
pub const COMMAND_SLOTS: usize = 16; // ordinary commands <=16 KiB; bulk DM settings <=33 KiB; channel edit <=128 KiB; group icon <=350 KiB

pub struct Pending {
	pub sticker: Option<Sticker>,
	pub channel: Id,
	pub content: String,
	/// Filenames of the files uploaded with this message, in send order.
	pub attachments: Vec<String>,
	pub nonce: String,
	pub delivery: Delivery,
	pub confirmed: Option<Id>,
	/// The reply this send carried, so a failed row can restore it with the caption.
	pub reply: Option<Reply>,
}
impl Pending {
	/// Restoring this row would drop its reply target in favour of a different active one.
	pub fn reply_conflicts(&self, active: Option<Reply>) -> bool {
		self.reply
			.zip(active)
			.is_some_and(|(own, active)| own.target() != active.target())
	}
}
#[derive(Default)]
pub struct NavigationIndex {
	view_revisions: view_revisions::Revisions,
	invalidations: std::cell::Cell<u64>,
	bytes: std::cell::Cell<Option<(usize, usize, usize)>>,
	channels: std::cell::RefCell<BTreeMap<Id, usize>>,
	channel_stamp: std::cell::Cell<Option<(usize, usize)>>,
	last_channel: std::cell::Cell<Option<(Id, usize)>>,
	guilds: std::cell::RefCell<BTreeMap<Id, usize>>,
	guild_stamp: std::cell::Cell<Option<(usize, usize)>>,
}

/// Where a text channel was left during this session.
/// `message` is absent when the reader was on the live edge.
#[derive(Clone, Copy)]
pub struct ReadingCursor {
	pub message: Option<Id>,
	pub inset: f32,
}

pub struct State {
	pub community: community::Community,
	/// The signed-in account's Discord `premium_type`; 0 until READY reports one.
	pub premium_type: u8,
	pub stickers: stickers::Stickers,
	pub soundboard: soundboard::Soundboard,
	pub interactions: interactions::Interactions,
	pub application_commands: application_commands::Catalog,
	pub game_mentions: game_mentions::Catalog,
	pub messaging_permissions: messaging_permissions::Settings,
	pub guild_folders: Option<model::guild_folders::Settings>,
	pub folders_pending: bool,
	pub folders_error: Option<&'static str>,
	/// Discord reported a folder change since the last load.
	pub folders_stale: bool,
	pub user_actions: user_actions::Actions,
	pub server_actions: server_actions::Actions,
	pub channel_actions: channel_actions::Actions,
	pub server_settings: server_settings::Editor,
	pub member_guide: member_guide::View,
	pub onboarding: onboarding::Onboarding,
	pub server_admin: server_admin::View,
	pub server_members_shortcuts: BTreeMap<Id, bool>,
	/// Bounded service-reported security actions; no persistence or local detection.
	pub server_incidents: BTreeMap<Id, model::server_access::Incident>,
	pub group_actions: group_actions::Actions,
	pub typing: typing::Typing,
	pub permissions: permissions::Permissions,
	pub archives: Option<archives::View>,
	pub posting: forum::Posting,
	pub posts: forum::Posts,
	pub archived_thread: Option<Id>,
	pub thread_starter: thread_starter::Starter,
	pub search: Option<search::SearchView>,
	pub inbox: inbox::View,
	pub reporting: reporting::View,
	pub search_request: u64,
	pub gifs: gifs::Gifs,
	/// A pin changed in this channel; the pins view should be reloaded once.
	pub pins_changed: Option<Id>,
	pub message_actions: message_actions::MessageActions,
	pub polls: polls::Polls,
	pub search_target: Option<Id>,
	/// The next consumed `search_target` restores a saved inset instead of centering.
	pub restore_scroll: bool,
	/// The active range was fetched around a target, independently of its consumed scroll cue.
	pub history_targeted: bool,
	/// One exact target awaiting its history page; never a metadata cache.
	#[doc(hidden)]
	pub history_navigation: HistoryNavigation,
	pub reply_previews: reply_previews::Previews,
	/// Session reading cursors. Missing means the channel has not been opened yet.
	pub reading: Vec<(Id, ReadingCursor)>,
	pub reply_deletions: ReplyDeletions,
	pub read_state: read_state::ReadState,
	pub startup_warnings: model::account::Warnings,
	pub notification_preferences: notifications::Preferences,
	pub preference_queue: preferences_sync::Queue,
	pub browse_channels: Option<Id>,
	pub reactions: reactions::Reactions,
	pub reaction_preferences: reactions::Preferences,
	pub profile: Option<profile::ProfileView>,
	pub profile_request: u64,
	pub profile_cache: profile::ProfileCache,
	/// Device preference for which profile cosmetics are shown and loaded.
	pub profile_cosmetics: model::CosmeticPreferences,
	pub own_profile: profile::OwnProfile,
	pub own_server_profile: server_profile::Editor,
	pub account_settings: account_settings::Settings,
	pub owned_cosmetics: owned_cosmetics::Editor,
	pub badge_editor: profile::BadgeEditor,
	pub invites: invites::Cache,
	pub invite_join: invites::Join,
	pub guild_creation: guild_creation::Creation,
	pub voice: voice::State,
	pub generation: u64,
	pub auth: auth::AuthState,
	pub user: Option<User>,
	pub members: Option<MemberList>,
	pub member_chunks: MemberChunks,
	pub member_search: [member_search::View; 2],
	pub member_labels: member_identity::Labels,
	pub member_search_nonce: u64,
	pub direct_presences: Vec<MemberPresence>,
	pub local_game_activity: Option<model::RichActivity>,
	#[doc(hidden)]
	pub direct_presence_bytes: Option<(usize, usize)>,
	#[doc(hidden)]
	pub direct_presence_epoch: u64,
	/// A direct presence was dropped or evicted, or READY's presence was unavailable.
	#[doc(hidden)]
	pub direct_presence_lossy: bool,
	pub member_request: u64,
	pub guilds: Vec<Guild>,
	pub channels: Vec<Channel>,
	#[doc(hidden)]
	pub navigation_index: NavigationIndex,
	pub selected: Option<Id>,
	/// Session-local last opened direct or group message.
	#[doc(hidden)]
	pub last_viewed_dm: Option<Id>,
	/// Session-local guild/channel ID pairs, oldest visit first; at most 16 KiB.
	#[doc(hidden)]
	pub last_viewed_channels: Vec<(Id, Id)>,
	/// Session-local opened threads, newest first; at most 1,024 IDs / 8 KiB.
	#[doc(hidden)]
	pub last_viewed_threads: Vec<Id>,
	pub timeline: Timeline,
	pub preserve_deleted_messages: bool,
	pub resident: resident::Windows,
	pub preload: preload::Preload,
	pub freshness: Freshness,
	pub status: &'static str,
	/// User-copyable cause of the last session failure, cleared by the next READY.
	pub failure_detail: Option<Box<str>>,
	pub drafts: BTreeMap<Id, String>,
	pub pending: Vec<Pending>,
	/// Discord rate-limited a send; the UI shows "sending too quickly" until dismissed.
	pub send_throttled: bool,
	/// The notice was dismissed for this burst; it returns only after the queue drains.
	#[doc(hidden)]
	pub send_throttle_seen: bool,
	/// Search results run oldest first, sorted by Discord rather than per page.
	pub search_oldest: bool,
	pub send_verification: verification::SendVerification,
	pub mfa: mfa::View,
	pub reply: Option<Reply>,
	/// Reply targets of conversations left before sending; at most 64, session RAM only.
	#[doc(hidden)]
	pub parked_replies: ParkedReplies,
	pub send_sequence: u64,
	pub request: u64,
	pub history_before: Option<Id>,
	pub history_after: Option<Id>,
	pub newer_cursor: Option<Id>,
	pub newer_may_have_more: bool,
	pub history_pending: bool,
	pub older_exhausted: bool,
	pub gateway_connected: bool,
	pub revision: u64,
	pub demo: bool,
	#[doc(hidden)]
	pub trail: Trail,
}

const MEMBER_CHUNK: usize = 100;
const MEMBER_CACHE_BYTES: usize = 1024 * 1024;
const MEMBER_CACHE_CHUNKS: usize = 32;

/// Decoded member-list chunks for the open request. The gateway subscription stays on the
/// viewport. This cache is what the sidebar paints when the user scrolls back.
#[derive(Default)]
pub struct MemberChunks {
	request: u64,
	viewport: usize,
	anchor: Option<usize>,
	chunks: BTreeMap<usize, Vec<Option<MemberSlot>>>,
}

impl MemberChunks {
	fn clear(&mut self) {
		*self = Self::default();
	}

	fn slot(&self, index: usize) -> Option<&MemberSlot> {
		let start = (index / MEMBER_CHUNK) * MEMBER_CHUNK;
		self.chunks.get(&start)?.get(index - start)?.as_ref()
	}

	fn occupied(&self) -> bool {
		self.chunks
			.values()
			.any(|chunk| chunk.iter().any(|slot| slot.is_some()))
	}

	fn bytes(&self) -> usize {
		self.chunks
			.values()
			.flat_map(|chunk| chunk.iter().flatten())
			.map(MemberSlot::bytes)
			.sum()
	}

	fn merge(&mut self, list: &MemberList) {
		if !list.lazy {
			return;
		}
		if self.request != list.request {
			self.chunks.clear();
			self.anchor = None;
			self.request = list.request;
		}
		let pending_holes =
			list.freshness != Freshness::Fresh && list.slots.iter().all(|slot| slot.is_none());
		if !pending_holes {
			if self.anchor.is_none() && list.slots.iter().any(|slot| slot.is_some()) {
				self.anchor = Some((list.start / MEMBER_CHUNK) * MEMBER_CHUNK);
			}
			for (offset, slot) in list.slots.iter().enumerate() {
				let absolute = list.start.saturating_add(offset);
				let start = (absolute / MEMBER_CHUNK) * MEMBER_CHUNK;
				let chunk = self
					.chunks
					.entry(start)
					.or_insert_with(|| vec![None; MEMBER_CHUNK]);
				let relative = absolute - start;
				if relative < chunk.len() {
					chunk[relative] = slot.clone();
				}
			}
		}
	}

	fn look_at(&mut self, index: usize, ranges: &[[usize; 2]]) {
		self.viewport = index;
		self.evict(ranges);
	}

	fn evict(&mut self, ranges: &[[usize; 2]]) {
		let anchor = self.anchor;
		let protected = |start: usize| {
			anchor == Some(start)
				|| ranges
					.iter()
					.any(|[from, to]| start <= *to && start + MEMBER_CHUNK > *from)
		};
		let viewport_chunk = (self.viewport / MEMBER_CHUNK) * MEMBER_CHUNK;
		while self.chunks.len() > MEMBER_CACHE_CHUNKS || self.bytes() > MEMBER_CACHE_BYTES {
			let Some(victim) = self
				.chunks
				.keys()
				.copied()
				.filter(|start| !protected(*start))
				.max_by_key(|start| start.abs_diff(viewport_chunk))
			else {
				break;
			};
			self.chunks.remove(&victim);
		}
	}
}

/// Result of a back/forward step that landed. `command` is the optional history fetch.
pub struct NavStep {
	pub command: Option<Command>,
}

// Navigation carries one bounded command; keep this short-lived result allocation-free.
#[allow(clippy::large_enum_variant)]
enum Apply {
	Opened(Option<Command>),
	AlreadyHere,
	Rejected(&'static str),
}

impl Default for State {
	fn default() -> Self {
		Self {
			community: Default::default(),
			premium_type: 0,
			stickers: Default::default(),
			soundboard: Default::default(),
			interactions: Default::default(),
			application_commands: Default::default(),
			messaging_permissions: Default::default(),
			guild_folders: None,
			folders_pending: false,
			folders_error: None,
			folders_stale: false,
			user_actions: user_actions::Actions::default(),
			server_actions: server_actions::Actions::default(),
			channel_actions: channel_actions::Actions::default(),
			server_settings: server_settings::Editor::default(),
			member_guide: member_guide::View::default(),
			onboarding: onboarding::Onboarding::default(),
			server_admin: server_admin::View::default(),
			server_members_shortcuts: BTreeMap::new(),
			server_incidents: BTreeMap::new(),
			group_actions: group_actions::Actions::default(),
			typing: typing::Typing::default(),
			permissions: permissions::Permissions::default(),
			archives: None,
			posting: forum::Posting::default(),
			posts: forum::Posts::default(),
			archived_thread: None,
			thread_starter: Default::default(),
			search: None,
			inbox: Default::default(),
			reporting: Default::default(),
			search_request: 0,
			gifs: gifs::Gifs::default(),
			pins_changed: None,
			message_actions: Default::default(),
			polls: Default::default(),
			search_target: None,
			restore_scroll: false,
			history_targeted: false,
			history_navigation: HistoryNavigation::default(),
			reply_previews: reply_previews::Previews::default(),
			reading: Vec::new(),
			reply_deletions: ReplyDeletions::default(),
			read_state: read_state::ReadState::default(),
			startup_warnings: Default::default(),
			notification_preferences: notifications::Preferences::default(),
			preference_queue: preferences_sync::Queue::default(),
			browse_channels: None,
			reactions: reactions::Reactions::default(),
			reaction_preferences: Default::default(),
			profile: None,
			profile_request: 0,
			profile_cache: Default::default(),
			profile_cosmetics: Default::default(),
			own_profile: Default::default(),
			own_server_profile: Default::default(),
			account_settings: Default::default(),
			owned_cosmetics: Default::default(),
			badge_editor: Default::default(),
			invites: Default::default(),
			invite_join: Default::default(),
			guild_creation: Default::default(),
			voice: voice::State::default(),
			generation: 1,
			auth: auth::AuthState::Unauthenticated,
			user: None,
			members: None,
			member_chunks: MemberChunks::default(),
			member_search: Default::default(),
			member_labels: Default::default(),
			member_search_nonce: 0,
			game_mentions: Default::default(),
			direct_presences: vec![],
			local_game_activity: Default::default(),
			direct_presence_bytes: None,
			direct_presence_epoch: 0,
			direct_presence_lossy: false,
			member_request: 0,
			guilds: vec![],
			channels: vec![],
			navigation_index: NavigationIndex::default(),
			selected: None,
			last_viewed_dm: None,
			last_viewed_channels: Vec::new(),
			last_viewed_threads: Vec::new(),
			timeline: Timeline::default(),
			preserve_deleted_messages: false,
			resident: resident::Windows::default(),
			preload: Default::default(),
			freshness: Freshness::Stale,
			status: "Disconnected",
			failure_detail: None,
			drafts: BTreeMap::new(),
			pending: vec![],
			send_throttled: false,
			send_throttle_seen: false,
			search_oldest: false,
			send_verification: Default::default(),
			mfa: Default::default(),
			reply: None,
			parked_replies: ParkedReplies::default(),
			send_sequence: 0,
			request: 0,
			history_before: None,
			history_after: None,
			newer_cursor: None,
			newer_may_have_more: false,
			history_pending: false,
			older_exhausted: false,
			gateway_connected: false,
			revision: 0,
			demo: false,
			trail: Trail::default(),
		}
	}
}
/// Channels the conversation pane can present: text, voice, and forum containers.
fn navigable(channel: &Channel) -> bool {
	channel.supports_text()
		|| matches!(channel.kind, 2 | 13)
		|| (channel.guild.is_some() && matches!(channel.kind, 15 | 16))
}
impl State {
	const MAX_READING_CURSORS: usize = 64;
}

#[cfg(test)]
mod tests {

	#[test]
	fn member_review_invalidates_only_for_snapshot_mutations_or_session_loss() {
		use model::{moderation, server_admin};
		let review = || moderation::Review {
			guild: Id(2),
			channel: Id(1),
			member: server_admin::Member {
				user: message(50).author,
				nick: None,
				roles: vec![],
				joined_at: None,
				join_source: None,
				invite_code: None,
				flags: None,
				unusual_dm_until: None,
				timeout_until: None,
			},
			roles: vec![],
			messages: [50, 51]
				.into_iter()
				.map(|id| moderation::ReviewMessage {
					id: Id(id),
					channel: Id(1),
					excerpt: "Synthetic review excerpt".into(),
				})
				.collect(),
			audit: vec![],
			audit_window: None,
			messages_error: None,
			audit_error: None,
		};
		let mut state = State::default();
		state.server_admin.member_review = Some(review());
		apply(&mut state, Event::Message(message(50)));
		let retained = state.server_admin.member_review.as_ref().unwrap();
		assert_eq!(retained.messages.len(), 2);
		assert_eq!(retained.messages[0].excerpt, "Synthetic review excerpt");
		assert!(retained.messages_error.is_none());
		apply(
			&mut state,
			Event::Patch(crate::message_actions::content_patch(
				Id(1),
				Id(50),
				"Edited text".into(),
			)),
		);
		let invalidated = state.server_admin.member_review.as_ref().unwrap();
		assert!(invalidated.messages.is_empty() && invalidated.messages_error.is_some());
		for event in [
			Event::Delete {
				channel: Id(1),
				id: Id(50),
			},
			Event::DeleteBulk {
				channel: Id(1),
				ids: vec![Id(50)],
			},
		] {
			state.server_admin.member_review = Some(review());
			apply(&mut state, event);
			let retained = &state.server_admin.member_review.as_ref().unwrap().messages;
			assert_eq!(retained.len(), 1);
			assert_eq!(retained[0].id, Id(51));
		}
		state.server_admin.member_review = Some(review());
		apply(&mut state, Event::Unavailable(Id(1)));
		assert!(
			state
				.server_admin
				.member_review
				.as_ref()
				.unwrap()
				.messages
				.is_empty()
		);
		for event in [
			Event::Disconnected,
			Event::Resync,
			Event::Failure(auth::Failure::Network),
		] {
			state.server_admin.member_review = Some(review());
			apply(&mut state, event);
			assert!(state.server_admin.member_review.is_none());
		}
	}

	#[test]
	#[ignore = "manual release timing; run with --release --ignored --nocapture"]
	fn guild_lookup_benchmark() {
		let state = State {
			guilds: (1..=1_000)
				.map(|id| Guild {
					default_message_notifications: None,
					stickers: None,
					id: Id(id),
					name: "Synthetic".into(),
					icon: None,
					emojis: None,
				})
				.collect(),
			..State::default()
		};
		for run in 0..6 {
			let start = std::time::Instant::now();
			for _ in 0..10_000 {
				assert!(std::hint::black_box(state.guild(std::hint::black_box(Id(500)))).is_some());
				assert!(
					std::hint::black_box(state.guild(std::hint::black_box(Id(1_001)))).is_none()
				);
			}
			println!("guild lookup run {run} (0 = warmup): {:?}", start.elapsed());
		}
	}

	pub(crate) fn grant_permissions(state: &mut State) {
		let Some(user) = &state.user else {
			return;
		};
		state
			.permissions
			.replace(model::permissions::Snapshot {
				guilds: state
					.guilds
					.iter()
					.map(|guild| model::permissions::Guild {
						id: guild.id,
						owner: Some(user.id),
						roles: Some(vec![]),
						member: None,
					})
					.collect(),
				channels: state
					.channels
					.iter()
					.filter_map(|channel| {
						channel.guild.map(|guild| model::permissions::Channel {
							id: channel.id,
							guild,
							overwrites: Some(vec![]),
						})
					})
					.collect(),
			})
			.unwrap();
	}
	#[test]
	fn rate_limited_sends_stay_pending_and_raise_the_slow_down_notice() {
		let mut state = State {
			channels: vec![Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			freshness: Freshness::Fresh,
			gateway_connected: true,
			..State::default()
		};
		let mut queue = || {
			state.drafts.insert(Id(1), "e".into());
			let Command::Send { nonce, .. } = state.prepare_send().unwrap() else {
				panic!()
			};
			nonce
		};
		let (first, second) = (queue(), queue());
		let throttle = |nonce: &str| Event::SendThrottled {
			nonce: nonce.into(),
		};
		apply(&mut state, throttle(&first));
		assert!(state.send_throttled);
		assert_eq!(state.pending[0].delivery, Delivery::Sending);
		// Dismissed once, the notice stays away for the rest of the burst.
		state.dismiss_send_throttle();
		apply(&mut state, throttle(&second));
		assert!(!state.send_throttled);
		// A throttle for a message that is no longer pending raises nothing.
		apply(&mut state, throttle("gone"));
		assert!(!state.send_throttled);
		// Once the queue drains, the next burst is told again.
		state.user = Some(tests::message(1).author);
		for (id, nonce) in [(10, &first), (11, &second)] {
			let mut sent = tests::message(id);
			sent.nonce = Some(nonce.clone());
			apply(
				&mut state,
				Event::SendResult {
					nonce: nonce.clone(),
					result: Ok(sent),
				},
			);
		}
		assert!(state.pending.is_empty());
		state.drafts.insert(Id(1), "e".into());
		let Command::Send { nonce: third, .. } = state.prepare_send().unwrap() else {
			panic!()
		};
		apply(&mut state, throttle(&third));
		assert!(state.send_throttled);
		// Closing a notice after its burst has drained does not hide the next burst's notice.
		let mut sent = tests::message(12);
		sent.nonce = Some(third.clone());
		apply(
			&mut state,
			Event::SendResult {
				nonce: third,
				result: Ok(sent),
			},
		);
		assert!(state.pending.is_empty());
		state.dismiss_send_throttle();
		state.drafts.insert(Id(1), "e".into());
		let Command::Send { nonce: fourth, .. } = state.prepare_send().unwrap() else {
			panic!()
		};
		apply(&mut state, throttle(&fourth));
		assert!(state.send_throttled);
	}
	#[test]
	fn rejected_sends_do_not_invalidate_a_healthy_conversation() {
		let mut state = State {
			channels: vec![Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			freshness: Freshness::Fresh,
			gateway_connected: true,
			..State::default()
		};
		let command = state.prepare_send_with_attachment(Some("a.txt")).unwrap();
		let Command::Send { nonce, .. } = &command else {
			panic!()
		};
		let nonce = nonce.clone();
		state.selected = Some(Id(2));
		state.command_rejected(command);
		assert_eq!(state.pending[0].delivery, Delivery::Rejected);
		assert_eq!(state.freshness, Freshness::Fresh);
		apply(
			&mut state,
			Event::SendResult {
				nonce: nonce.clone(),
				result: Err(auth::Failure::ProtocolAt(
					"Upload cancelled; no message was sent",
				)),
			},
		);
		assert_eq!(state.freshness, Freshness::Fresh);
		assert!(state.gateway_connected);
		apply(
			&mut state,
			Event::SendResult {
				nonce,
				result: Err(auth::Failure::Expired),
			},
		);
		assert_eq!(state.auth, auth::AuthState::Expired);
		assert_eq!(state.freshness, Freshness::Stale);
		assert!(!state.gateway_connected);
	}
	#[test]
	fn attachment_sends_from_an_older_window_keep_caption_and_reply() {
		let mut state = State {
			channels: vec![Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			gateway_connected: true,
			..State::default()
		};
		state.history(None);
		let request = state.request;
		apply(
			&mut state,
			Event::History {
				channel: Id(1),
				request,
				older: false,
				messages: (51..101).map(message).collect(),
			},
		);
		state.older_history().unwrap();
		let request = state.request;
		apply(
			&mut state,
			Event::History {
				channel: Id(1),
				request,
				older: true,
				messages: (1..51).map(message).collect(),
			},
		);
		assert!(state.history_before.is_some() && state.freshness == Freshness::Fresh);
		// Reply to an old message with a captioned image.
		state.reply = Some(Reply::to(Id(10)));
		state.drafts.insert(Id(1), "Caption".into());
		let command = state.prepare_send_with_attachments(&["photo.png"]).unwrap();
		assert!(state.reply.is_none() && !state.drafts.contains_key(&Id(1)));
		assert_eq!(state.pending[0].reply, Some(Reply::to(Id(10))));
		// Send jumps to the latest page before the desktop dispatches the upload. That
		// reload is not a channel state, so the attachment gate stays open.
		state.history(None);
		assert_eq!(state.freshness, Freshness::Loading);
		assert!(state.can_attach(Id(1)));
		assert!(matches!(
			&command,
			Command::Send { reply: Some(reply), .. } if reply.target() == Id(10)
		));
		// A send stopped before transport goes back to the composer intact.
		assert!(state.return_unsent(command, "File not sent"));
		assert!(state.pending.is_empty());
		assert_eq!(state.drafts[&Id(1)], "Caption");
		assert_eq!(state.reply, Some(Reply::to(Id(10))));
		assert_eq!(state.status, "File not sent");
		// A composer that moved on keeps its newer draft; the row fails and keeps the reply.
		let command = state.prepare_send_with_attachments(&["photo.png"]).unwrap();
		state.drafts.insert(Id(1), "Newer".into());
		assert!(!state.return_unsent(command, "File not sent"));
		assert_eq!(state.drafts[&Id(1)], "Newer");
		assert_eq!(state.pending[0].delivery, Delivery::Rejected);
		assert_eq!(state.pending[0].content, "Caption");
		assert_eq!(state.pending[0].reply, Some(Reply::to(Id(10))));
		// Failed rows can be dismissed; a row still sending cannot.
		let nonce = state.pending[0].nonce.clone();
		assert_eq!(state.discard_pending(&nonce), Some(Id(1)));
		assert!(state.pending.is_empty());
		// A reply started meanwhile is never swapped: the row fails and keeps its own target.
		state.drafts.remove(&Id(1));
		state.reply = Some(Reply::to(Id(10)));
		let command = state.prepare_send_with_attachments(&["photo.png"]).unwrap();
		state.reply = Some(Reply::to(Id(20)));
		assert!(!state.return_unsent(command, "File not sent"));
		assert_eq!(state.reply, Some(Reply::to(Id(20))));
		assert!(!state.drafts.contains_key(&Id(1)));
		assert_eq!(state.pending[0].delivery, Delivery::Rejected);
		assert_eq!(state.pending[0].reply, Some(Reply::to(Id(10))));
		// Deleting a target outside the loaded window still forgets it on failed rows.
		state.selected = Some(Id(2));
		apply(
			&mut state,
			Event::Delete {
				channel: Id(1),
				id: Id(10),
			},
		);
		assert_eq!(state.pending[0].reply, None);
		state.pending[0].reply = Some(Reply::to(Id(11)));
		apply(
			&mut state,
			Event::DeleteBulk {
				channel: Id(3),
				ids: vec![Id(11)],
			},
		);
		assert_eq!(
			state.pending[0].reply,
			Some(Reply::to(Id(11))),
			"other channel"
		);
		apply(
			&mut state,
			Event::DeleteBulk {
				channel: Id(1),
				ids: vec![Id(9), Id(11)],
			},
		);
		assert_eq!(state.pending[0].reply, None);
		state.selected = Some(Id(1));
		state.pending.clear();
		state.drafts.insert(Id(1), "Text".into());
		let Command::Send { nonce, .. } = state.prepare_send().unwrap() else {
			panic!()
		};
		state.discard_pending(&nonce);
		assert_eq!(state.pending.len(), 1);
	}
	#[test]
	fn voice_message_send_preserves_draft_and_requires_fresh_attachment_admission() {
		let mut state = State {
			channels: vec![Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			freshness: Freshness::Fresh,
			gateway_connected: true,
			..State::default()
		};
		state
			.drafts
			.insert(Id(1), "@silent text stays separate".into());
		state.reply = Some(Reply::to(Id(7)));
		let command = state.prepare_voice_message().unwrap();
		assert!(
			matches!(command, Command::Send { content, reply: Some(reply), sticker: None, .. } if content.is_empty() && reply.target() == Id(7))
		);
		assert_eq!(state.drafts[&Id(1)], "@silent text stays separate");
		assert_eq!(state.pending[0].attachments, ["voice-message.ogg"]);
		assert!(state.reply.is_none());
		state.gateway_connected = false;
		assert!(state.prepare_voice_message().is_none());
		state.gateway_connected = true;
		state.freshness = Freshness::Stale;
		assert!(state.prepare_voice_message().is_none());
		state.freshness = Freshness::Fresh;
		state.auth = auth::AuthState::Expired;
		assert!(state.prepare_voice_message().is_none());
		assert_eq!(state.pending.len(), 1);
	}

	#[test]
	fn attachment_only_sends_are_bounded_and_keep_existing_confirmation() {
		let mut state = State {
			channels: vec![Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic DM".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			freshness: Freshness::Fresh,
			gateway_connected: true,
			..State::default()
		};
		assert!(state.prepare_send().is_none());
		state.drafts.insert(Id(1), "@silent".into());
		state.reply = Some(Reply::to(Id(7)));
		assert!(state.prepare_send().is_none());
		assert_eq!(state.drafts[&Id(1)], "@silent");
		assert!(state.pending.is_empty() && state.reply.is_some());
		let full = format!("@silent {}", "é".repeat(MAX_CONTENT));
		state.drafts.insert(Id(1), full.clone());
		assert!(
			matches!(state.prepare_send(), Some(Command::Send { content, .. }) if content == full)
		);
		assert_eq!(state.pending[0].content, full);
		state.pending.clear();
		state
			.drafts
			.insert(Id(1), format!("@silent {}", "x".repeat(MAX_CONTENT + 1)));
		assert!(state.prepare_send().is_none());
		assert!(!state.drafts[&Id(1)].is_empty() && state.pending.is_empty());
		state.drafts.clear();
		for invalid in [
			"",
			" ",
			".",
			"..",
			"../secret.txt",
			"C:\\secret.txt",
			"a\nb",
		] {
			assert!(state.prepare_send_with_attachment(Some(invalid)).is_none());
		}
		assert!(
			state
				.prepare_send_with_attachment(Some(&"é".repeat(129)))
				.is_none()
		);
		assert!(
			state
				.prepare_send_with_attachments(&["a.txt"; MAX_ATTACHMENTS + 1])
				.is_none()
		);
		assert!(
			state
				.prepare_send_with_attachments(&["a.txt", "../b.txt"])
				.is_none()
		);
		assert!(state.pending.is_empty());
		let batch = state
			.prepare_send_with_attachments(&["a.png", "b.png", "c.pdf"])
			.unwrap();
		assert_eq!(state.pending[0].attachments, ["a.png", "b.png", "c.pdf"]);
		state.command_rejected(batch);
		state.pending.clear();
		state.reply = Some(Reply::to(Id(4)));
		let Command::Send {
			content,
			nonce,
			reply,
			..
		} = state
			.prepare_send_with_attachment(Some("résumé.txt"))
			.unwrap()
		else {
			panic!()
		};
		assert!(content.is_empty());
		assert_eq!(reply, Some(Reply::to(Id(4))));
		assert_eq!(state.pending[0].attachments, ["résumé.txt"]);
		assert!(state.draft_bytes() >= "résumé.txt".len() + nonce.len() + size_of::<Pending>());
		apply(
			&mut state,
			Event::SendResult {
				nonce: nonce.clone(),
				result: Err(auth::Failure::Ambiguous),
			},
		);
		assert_eq!(state.pending[0].delivery, Delivery::Ambiguous);
		let mut confirmed = message(10);
		confirmed.nonce = Some(nonce.clone());
		apply(
			&mut state,
			Event::SendResult {
				nonce,
				result: Ok(confirmed),
			},
		);
		assert!(state.pending.is_empty());
		state.freshness = Freshness::Fresh;
		state.drafts.insert(
			Id(2),
			"x".repeat(MAX_DRAFT_BYTES - size_of::<Pending>() - 32),
		);
		assert!(state.prepare_send_with_attachment(Some("a.txt")).is_none());
		state.logout();
		assert!(!state.has_unsent());
	}
	use super::*;

	#[test]
	fn server_selection_restores_viewed_channels_and_revalidates_access() {
		{
			use model::permissions as p;
			let mut state = State {
				user: Some(message(1).author),
				guilds: (1..=2)
					.map(|id| Guild {
						default_message_notifications: None,
						stickers: None,
						id: Id(id),
						name: "Synthetic".into(),
						icon: None,
						emojis: None,
					})
					.collect(),
				channels: [(10, 1, 0), (11, 1, 0), (12, 1, 2), (20, 2, 0), (30, 0, 1)]
					.into_iter()
					.map(|(id, guild, kind)| Channel {
						id: Id(id),
						guild: (guild != 0).then_some(Id(guild)),
						kind,
						name: "Synthetic".into(),
						position: id as i32,
						parent_id: None,
						recipients: vec![],
						last_message: None,
						icon: None,
						member_list_id: None,
						tags: None,
						message_count: None,
					})
					.collect(),
				..State::default()
			};
			state
				.permissions
				.replace(p::Snapshot {
					guilds: (1..=2)
						.map(|id| p::Guild {
							id: Id(id),
							owner: Some(Id(999)),
							member: Some(p::Member {
								roles: vec![],
								timeout_until: None,
							}),
							roles: Some(vec![p::Role {
								icon: None,
								unicode_emoji: None,
								id: Id(id),
								name: String::new(),
								color: 0,
								secondary_color: None,
								tertiary_color: None,
								position: 0,
								hoist: false,
								bits: p::VIEW_CHANNEL | p::READ_MESSAGE_HISTORY,
							}]),
						})
						.collect(),
					channels: state
						.channels
						.iter()
						.filter_map(|c| {
							c.guild.map(|guild| p::Channel {
								id: c.id,
								guild,
								overwrites: Some(vec![]),
							})
						})
						.collect(),
				})
				.unwrap();
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(10),
					..
				})
			));
			// A joined server opens once delivered; choosing another server cancels that.
			state.invite_join.navigate = Some((std::time::Instant::now(), Id(2)));
			assert!(state.navigate_after_join().is_some());
			assert_eq!(
				state
					.selected
					.and_then(|id| state.channel(id))
					.and_then(|c| c.guild),
				Some(Id(2))
			);
			assert!(state.navigate_after_join().is_none());
			state.invite_join.navigate = Some((std::time::Instant::now(), Id(2)));
			state.select(Id(11));
			assert!(state.navigate_after_join().is_none());
			assert_eq!(state.selected, Some(Id(11)));
			state.select(Id(20));
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(11),
					..
				})
			));
			state.drafts.insert(Id(11), "Unsent draft".into());
			let request = state.request;
			assert!(state.select_guild(Id(1)).is_none());
			assert_eq!(state.request, request);
			assert_eq!(state.drafts[&Id(11)], "Unsent draft");
			state.select(Id(30));
			assert_eq!(
				state.last_viewed_channels.len(),
				2,
				"DMs are not server history"
			);
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(11),
					..
				})
			));
			state.select(Id(20));
			state
				.permissions
				.channels
				.get_mut(&Id(11))
				.unwrap()
				.overwrites = Some(vec![p::Overwrite {
				id: Id(1),
				kind: 0,
				allow: 0,
				deny: p::VIEW_CHANNEL,
			}]);
			state.permissions.clear_cache();
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(10),
					..
				})
			));
			state.select(Id(20));
			state.channels.retain(|c| c.id != Id(10));
			state.invalidate_navigation();
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(12),
					..
				})
			));
			assert!(state.voice.active.is_none());
			assert_eq!(
				state.selected,
				Some(Id(12)),
				"voice is only viewed, never joined"
			);
			state.select(Id(20));
			assert!(matches!(
				state.select_guild(Id(1)),
				Some(Command::History {
					channel: Id(12),
					..
				})
			));
			assert!(state.voice.active.is_none());
			assert_eq!(
				state.selected,
				Some(Id(12)),
				"remember a viewed voice channel too"
			);
			state.select(Id(20));
			state.permissions.guilds.remove(&Id(1));
			state.permissions.clear_cache();
			assert!(state.select_guild(Id(1)).is_none());
			assert_eq!(
				state.selected,
				Some(Id(20)),
				"no accessible channel leaves the conversation intact"
			);
			state.last_viewed_channels = (100..100 + MAX_VIEWED_SERVERS as u64)
				.map(|id| (Id(id), Id(id)))
				.collect();
			state.remember_channel(Id(20));
			assert_eq!(state.last_viewed_channels.len(), MAX_VIEWED_SERVERS);
			assert!(state.last_viewed_channels.capacity() * size_of::<(Id, Id)>() <= 16 * 1024);
			assert!(
				!state
					.last_viewed_channels
					.iter()
					.any(|(id, _)| *id == Id(100))
			);
			state.logout();
			assert!(state.last_viewed_channels.is_empty());
		}
		{
			let guild = |id| Guild {
				default_message_notifications: None,
				stickers: None,
				id: Id(id),
				name: "Synthetic".into(),
				icon: None,
				emojis: None,
			};
			let mut state = State {
				guilds: vec![guild(1)],
				..State::default()
			};
			assert!(state.guild(Id(2)).is_none());
			{
				// A cached miss must not rebuild (and mutably borrow) the index.
				let _index = state.navigation_index.guilds.borrow();
				for _ in 0..3 {
					assert!(state.guild(Id(2)).is_none());
					assert_eq!(state.guild(Id(1)).unwrap().id, Id(1));
				}
			}
			apply(&mut state, Event::GuildJoined(guild(2)));
			assert_eq!(state.guild(Id(2)).unwrap().id, Id(2));
			assert_eq!(state.guild(Id(1)).unwrap().id, Id(1));
			state.guilds.swap(0, 1);
			assert_eq!(state.guild(Id(2)).unwrap().id, Id(2));
			state.guilds[0] = guild(3);
			state.invalidate_navigation();
			assert!(state.guild(Id(1)).is_none());
			assert_eq!(state.guild(Id(3)).unwrap().id, Id(3));
			state.guilds.retain(|guild| guild.id != Id(3));
			assert!(state.guild(Id(3)).is_none());
			assert_eq!(state.guild(Id(2)).unwrap().id, Id(2));
		}
		{
			let channel = Channel {
				id: Id(1),
				guild: Some(Id(10)),
				parent_id: None,
				kind: 0,
				name: "Original".into(),
				position: 7,
				recipients: vec![],
				last_message: Some(Id(80)),
				icon: None,
				member_list_id: Some("known-list".into()),
				tags: None,
				message_count: None,
			};
			let mut state = State {
				user: Some(message(1).author),
				channels: vec![channel.clone()],
				guilds: vec![Guild {
					default_message_notifications: None,
					stickers: None,
					id: Id(10),
					name: "Synthetic".into(),
					icon: None,
					emojis: None,
				}],
				selected: Some(Id(1)),
				auth: auth::AuthState::Authenticated,
				gateway_connected: true,
				freshness: Freshness::Fresh,
				..State::default()
			};
			grant_permissions(&mut state);
			let candidate = Channel {
				name: "Restored".into(),
				position: 0,
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
				..channel.clone()
			};
			apply(&mut state, Event::ChannelRestored(candidate.clone()));
			assert!(
				state.channels[0] == channel,
				"A candidate cannot replace a known channel's absent fields"
			);
			apply(
				&mut state,
				Event::ChannelChanged(ChannelPatch {
					icon: model::Patch::Absent,
					id: Id(1),
					name: Patch::Value("Renamed".into()),
					last_message: Patch::Absent,
					parent_id: Patch::Absent,
					position: Patch::Absent,
					kind: Patch::Absent,
					message_count: Patch::Absent,
					tags: Patch::Absent,
				}),
			);
			assert_eq!(state.channels[0].name, "Renamed");
			assert_eq!(state.channels[0].last_message, Some(Id(80)));
			assert_eq!(
				state.channels[0].member_list_id.as_deref(),
				Some("known-list")
			);
			state.drafts.insert(Id(1), "Preserved draft".into());
			state.history(None);
			let old_request = state.request;
			apply(&mut state, Event::Unavailable(Id(1)));
			for invalid in [
				Channel {
					guild: None,
					..candidate.clone()
				},
				Channel {
					guild: Some(Id(99)),
					..candidate.clone()
				},
				Channel {
					guild: Some(Id(0)),
					..candidate.clone()
				},
				Channel {
					id: Id(0),
					..candidate.clone()
				},
				Channel {
					kind: 1,
					..candidate.clone()
				},
				Channel {
					kind: 10,
					..candidate.clone()
				},
				Channel {
					kind: 11,
					..candidate.clone()
				},
				Channel {
					kind: 12,
					..candidate.clone()
				},
				Channel {
					kind: 255,
					..candidate.clone()
				},
			] {
				apply(&mut state, Event::ChannelRestored(invalid));
				assert!(state.channels.is_empty());
			}
			apply(&mut state, Event::ChannelRestored(candidate.clone()));
			assert!(state.channels[0] == candidate);
			assert_eq!(state.freshness, Freshness::Unavailable);
			assert!(!state.history_pending);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request: old_request,
					older: false,
					messages: vec![message(99)],
				},
			);
			assert!(state.timeline.is_empty());
			assert_eq!(state.drafts[&Id(1)], "Preserved draft");
			assert!(matches!(state.select(Id(1)), Some(Command::History { .. })));
			let request = state.request;
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: vec![message(100)],
				},
			);
			assert_eq!(state.freshness, Freshness::Fresh);
			assert!(state.timeline.get(Id(100)).is_some());
		}
	}

	#[test]
	fn reaction_readback_coalesces_races_and_never_replays_uncertain_writes() {
		use reactions::{Command as R, Event as E};
		let channel = Id(1);
		let id = Id(10);
		let emoji = ReactionEmoji {
			id: None,
			name: Some("👍".into()),
		};
		let values = vec![Reaction {
			emoji: emoji.clone(),
			count: 3,
			me: true,
			me_burst: false,
		}];
		let mut state = State {
			selected: Some(channel),
			channels: vec![Channel {
				id: channel,
				guild: None,
				parent_id: None,
				kind: 1,
				name: "Synthetic reaction conversation".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			gateway_connected: true,
			auth: auth::AuthState::Authenticated,
			freshness: Freshness::Fresh,
			..State::default()
		};
		state.timeline.insert(message(10), false, false).unwrap();
		let Command::Reactions(R::Set { request, add, .. }) =
			state.prepare_reaction(id, emoji.clone()).unwrap()
		else {
			panic!()
		};
		assert!(add);
		assert!(state.prepare_reaction(id, emoji.clone()).is_none());
		apply(
			&mut state,
			Event::Reactions(E::Written {
				channel,
				message: id,
				request,
				result: Err(auth::Failure::Ambiguous),
			}),
		);
		assert!(state.reactions.writing.is_none());
		let Command::Reactions(R::Read { request, .. }) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		for _ in 0..100 {
			apply(
				&mut state,
				Event::Reactions(E::Changed {
					channel,
					message: id,
				}),
			);
		}
		assert!(state.next_reaction_read().is_none());
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Ok(values.clone()),
			}),
		);
		assert!(state.timeline.get(id).unwrap().reactions.is_none());
		let Command::Reactions(R::Read { request, .. }) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Ok(values.clone()),
			}),
		);
		assert_eq!(
			state.timeline.get(id).unwrap().reactions,
			Some(values.clone())
		);
		assert!(state.next_reaction_read().is_none());
		let command = state.prepare_reaction(id, emoji.clone()).unwrap();
		assert!(matches!(
			&command,
			Command::Reactions(R::Set { add: false, .. })
		));
		state.command_rejected(command);
		assert!(state.reactions.writing.is_none());
		assert_eq!(state.freshness, Freshness::Fresh);
		// Successful reaction removal/readback changes only reactions, not chat access.
		let original_content = state.timeline.get(id).unwrap().content.clone();
		let Command::Reactions(R::Set {
			request,
			add: false,
			..
		}) = state.prepare_reaction(id, emoji.clone()).unwrap()
		else {
			panic!()
		};
		apply(
			&mut state,
			Event::Reactions(E::Written {
				channel,
				message: id,
				request,
				result: Ok(()),
			}),
		);
		assert_eq!(state.timeline.get(id).unwrap().content, original_content);
		let Command::Reactions(R::Read { request, .. }) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Ok(vec![]),
			}),
		);
		assert_eq!(state.timeline.get(id).unwrap().content, original_content);
		assert_eq!(state.timeline.get(id).unwrap().reactions, Some(vec![]));
		assert_eq!(state.freshness, Freshness::Fresh);
		assert!(state.next_reaction_read().is_none());
		// Reaction events during history loading cannot be overwritten by that page.
		let _ = state.history(None);
		apply(
			&mut state,
			Event::Reactions(E::Changed {
				channel,
				message: Id(11),
			}),
		);
		let history_request = state.request;
		apply(
			&mut state,
			Event::History {
				channel,
				request: history_request,
				older: false,
				messages: vec![message(11)],
			},
		);
		assert!(state.timeline.get(Id(11)).unwrap().reactions.is_none());
		let Command::Reactions(R::Read {
			message: id,
			request,
			..
		}) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		// Deletion wins over an in-flight reaction response.
		apply(&mut state, Event::Delete { channel, id });
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Ok(values.clone()),
			}),
		);
		assert!(state.timeline.get(id).is_none());
		// Rate limits leave an explicit retry, never an automatic loop.
		state.timeline.insert(message(12), false, false).unwrap();
		state.refresh_reactions(Id(12));
		let Command::Reactions(R::Read {
			message: id,
			request,
			..
		}) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Err(auth::Failure::RateLimited),
			}),
		);
		assert!(state.next_reaction_read().is_none());
		state.refresh_reactions(id);
		let Command::Reactions(R::Read { request, .. }) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		state.reactions.reset(); // navigation/disconnect cancels this request
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Ok(values),
			}),
		);
		assert!(state.timeline.get(id).unwrap().reactions.is_none());
		state.refresh_reactions(id);
		let Command::Reactions(R::Read { request, .. }) = state.next_reaction_read().unwrap()
		else {
			panic!()
		};
		apply(
			&mut state,
			Event::Reactions(E::Read {
				channel,
				message: id,
				request,
				result: Err(auth::Failure::Forbidden),
			}),
		);
		assert!(state.timeline.is_empty());
		assert_eq!(state.freshness, Freshness::Unavailable);
		state.logout();
		assert!(state.next_reaction_read().is_none());
	}

	#[test]
	fn guild_emoji_updates_replace_catalog_and_reject_stale_or_oversized_data() {
		{
			let emoji = CustomEmoji {
				id: Id(4),
				name: "wave".into(),
				animated: false,
				available: true,
				managed: false,
				roles: Some(vec![]),
			};
			let mut state = State {
				guilds: vec![Guild {
					default_message_notifications: None,
					stickers: None,
					id: Id(2),
					name: "Synthetic".into(),
					icon: None,
					emojis: None,
				}],
				..State::default()
			};
			let event = Event::GuildEmojis {
				guild: Id(2),
				emojis: vec![emoji.clone()],
			};
			assert!(event.bytes() >= size_of::<Event>() + custom_emoji_bytes(&vec![emoji.clone()]));
			apply(&mut state, event);
			assert_eq!(
				state.guilds[0].emojis.as_ref().unwrap()[0].markup(),
				"<:wave:4>"
			);
			state.apply(Envelope {
				generation: state.generation + 1,
				event: Event::GuildEmojis {
					guild: Id(2),
					emojis: vec![],
				},
			});
			assert_eq!(state.guilds[0].emojis.as_ref().unwrap().len(), 1);
			apply(
				&mut state,
				Event::GuildEmojis {
					guild: Id(2),
					emojis: vec![],
				},
			);
			assert!(state.guilds[0].emojis.as_ref().unwrap().is_empty());
			let mut huge = emoji.clone();
			huge.name = String::with_capacity(MAX_GUILD_EMOJI_BYTES);
			huge.name.push_str("wave");
			apply(
				&mut state,
				Event::GuildEmojis {
					guild: Id(2),
					emojis: vec![huge],
				},
			);
			assert!(state.guilds[0].emojis.is_none());
			apply(
				&mut state,
				Event::GuildEmojis {
					guild: Id(2),
					emojis: vec![emoji; MAX_GUILD_EMOJIS + 1],
				},
			);
			assert!(state.guilds[0].emojis.is_none());
			state.logout();
			assert!(state.guilds.is_empty());
		}
		{
			let emoji = CustomEmoji {
				id: Id(4),
				name: "wave".into(),
				animated: false,
				available: true,
				managed: false,
				roles: Some(vec![]),
			};
			let mut state = State::default();
			let mut guilds: Vec<_> = (1..=700)
				.map(|id| Guild {
					default_message_notifications: None,
					stickers: None,
					id: Id(id),
					name: "Synthetic".into(),
					icon: None,
					emojis: Some(vec![emoji.clone()]),
				})
				.collect();
			for guild in &mut guilds {
				let name = &mut guild.emojis.as_mut().unwrap()[0].name;
				name.reserve(192 * 1024);
			}
			assert!(
				guilds
					.iter()
					.all(|g| valid_custom_emojis(g.emojis.as_ref().unwrap()))
			);
			apply(
				&mut state,
				Event::Ready {
					permissions: model::permissions::Snapshot::default(),
					user: User {
						username: None,
						primary_guild: None,
						cosmetics: None,
						id: Id(1),
						name: "Synthetic".into(),
						avatar: None,
						webhook: false,
						kind: Default::default(),
						discriminator: 0,
					},
					guilds,
					channels: vec![],
				},
			);
			assert!(state.guilds.is_empty());
			assert!(matches!(state.auth, auth::AuthState::Failed));
		}
	}

	#[test]
	fn guild_identity_patches_preserve_omitted_fields_and_change_icon_keys() {
		let mut state = State {
			guilds: vec![Guild {
				default_message_notifications: None,
				stickers: None,
				emojis: None,
				id: Id(2),
				name: "Synthetic server".into(),
				icon: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
			}],
			..State::default()
		};
		let key = state.guilds[0].icon_key();
		apply(
			&mut state,
			Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(2),
				name: Patch::Value("Renamed".into()),
				icon: Patch::Absent,
			}),
		);
		assert_eq!(state.guilds[0].name, "Renamed");
		assert_eq!(state.guilds[0].icon_key(), key);
		apply(
			&mut state,
			Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(2),
				name: Patch::Absent,
				icon: Patch::Value("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
			}),
		);
		assert_ne!(state.guilds[0].icon_key(), key);
		assert_eq!(state.guilds[0].name, "Renamed");
		apply(
			&mut state,
			Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(2),
				name: Patch::Absent,
				icon: Patch::Null,
			}),
		);
		assert!(state.guilds[0].icon_key().is_none());
		apply(
			&mut state,
			Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(2),
				name: Patch::Value("x".repeat(1024)),
				icon: Patch::Value("../../invalid".into()),
			}),
		);
		assert_eq!(state.guilds[0].name.len(), 128);
		assert!(state.guilds[0].icon.is_none());
		apply(
			&mut state,
			Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(3),
				name: Patch::Value("Unknown".into()),
				icon: Patch::Absent,
			}),
		);
		assert_eq!(state.guilds.len(), 1);
		state.apply(Envelope {
			generation: state.generation + 1,
			event: Event::GuildChanged(GuildPatch {
				default_message_notifications: model::Patch::Absent,
				id: Id(2),
				name: Patch::Value("Late event".into()),
				icon: Patch::Absent,
			}),
		});
		assert_eq!(state.guilds[0].name.len(), 128);
	}

	#[test]
	fn channel_mutations_preserve_partial_metadata_and_remove_deleted_categories() {
		{
			let mut state = State {
				user: Some(message(1).author),
				guilds: vec![Guild {
					default_message_notifications: None,
					stickers: None,
					id: Id(1),
					name: "Synthetic".into(),
					icon: None,
					emojis: None,
				}],
				..State::default()
			};
			let channel = Channel {
				last_message: None,
				id: Id(2),
				guild: Some(Id(1)),
				parent_id: Some(Id(3)),
				position: 7,
				name: "Synthetic channel".into(),
				kind: 0,
				recipients: vec![],
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			};
			apply(&mut state, Event::ChannelCreated(channel.clone()));
			apply(&mut state, Event::ChannelCreated(channel));
			assert_eq!(state.channels.len(), 1);
			apply(
				&mut state,
				Event::ChannelChanged(ChannelPatch {
					icon: model::Patch::Absent,
					last_message: model::Patch::Absent,
					id: Id(2),
					name: Patch::Absent,
					parent_id: Patch::Null,
					position: Patch::Value(0),
					kind: Patch::Absent,
					message_count: Patch::Absent,
					tags: Patch::Absent,
				}),
			);
			assert_eq!(state.channels[0].parent_id, None);
			assert_eq!(state.channels[0].position, 0);
			assert_eq!(state.channels[0].name, "Synthetic channel");
			assert_eq!(state.channels[0].kind, 0);
			grant_permissions(&mut state);
			assert!(state.select(Id(2)).is_some());
			apply(
				&mut state,
				Event::ChannelChanged(ChannelPatch {
					icon: model::Patch::Absent,
					last_message: model::Patch::Absent,
					id: Id(2),
					name: Patch::Absent,
					parent_id: Patch::Absent,
					position: Patch::Absent,
					kind: Patch::Value(4),
					message_count: Patch::Absent,
					tags: Patch::Absent,
				}),
			);
			assert!(state.selected.is_none());
			assert!(!state.history_pending);
			assert!(state.select(Id(2)).is_none());
			apply(&mut state, Event::Unavailable(Id(2)));
			assert!(state.channels.is_empty());
			for id in 1..=MAX_NAV + 1 {
				apply(
					&mut state,
					Event::ChannelCreated(Channel {
						last_message: None,
						id: Id(id as u64),
						guild: None,
						parent_id: None,
						position: 0,
						name: String::new(),
						kind: 4,
						recipients: vec![],
						icon: None,
						member_list_id: None,
						tags: None,
						message_count: None,
					}),
				);
			}
			assert_eq!(state.channels.len() + state.guilds.len(), MAX_NAV);
			assert_eq!(state.status, auth::Failure::Capacity.label());
		}
		{
			let mut state = State {
				user: Some(message(1).author),
				gateway_connected: true,
				auth: auth::AuthState::Authenticated,
				selected: Some(Id(1)),
				channels: vec![Channel {
					id: Id(1),
					guild: None,
					parent_id: None,
					kind: 1,
					position: 0,
					name: "Synthetic DM".into(),
					recipients: vec![],
					last_message: None,
					icon: None,
					member_list_id: None,
					tags: None,
					message_count: None,
				}],
				..State::default()
			};
			state.drafts.insert(Id(1), "unsent draft".into());
			state.history(None);
			let request = state.request;
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: (100..150).map(message).collect(),
				},
			);
			let positions = state.timeline.row_ids().collect::<Vec<_>>();
			state.reply = Some(Reply::to(Id(100)));
			apply(
				&mut state,
				Event::Delete {
					channel: Id(2),
					id: Id(100),
				},
			);
			assert_eq!(state.reply_target(), Some(Id(100)));
			assert!(state.timeline.get(Id(100)).is_some());
			apply(
				&mut state,
				Event::Delete {
					channel: Id(1),
					id: Id(100),
				},
			);
			assert_eq!(state.reply, None);
			state.reply = Some(Reply::to(Id(101)));
			let mut ids: Vec<_> = (101..150).map(Id).collect();
			ids.extend([Id(101), Id(999)]); // Duplicate and unknown IDs create no extra rows.
			apply(
				&mut state,
				Event::DeleteBulk {
					channel: Id(1),
					ids,
				},
			);
			assert_eq!(state.reply, None);
			assert_eq!(state.timeline.row_ids().collect::<Vec<_>>(), positions);
			assert!(state.timeline.is_empty());
			assert_eq!(state.timeline.bytes(), 0);
			assert!(state.timeline.get_display(Id(100)).is_none());
			assert!(state.can_load_older());
			assert!(!state.can_edit(Id(1), Id(100)));
			assert!(matches!(
				state.older_history(),
				Some(Command::History {
					before: Some(Id(100)),
					..
				})
			));
			let request = state.request;
			apply(
				&mut state,
				Event::SendResult {
					nonce: "synthetic-late".into(),
					result: Ok(message(100)),
				},
			);
			assert!(state.timeline.get(Id(100)).is_none());
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: true,
					messages: vec![message(99)],
				},
			);
			assert_eq!(state.timeline.len(), 1);
			assert_eq!(state.timeline.row_count(), 51);
			assert_eq!(state.timeline.row_ids().next(), Some(Id(99)));
			state.history(None);
			let request = state.request;
			apply(
				&mut state,
				Event::Delete {
					channel: Id(1),
					id: Id(99),
				},
			);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: vec![message(99), message(150)],
				},
			);
			assert_eq!(
				state.timeline.row_ids().collect::<Vec<_>>(),
				[Id(99), Id(150)]
			);
			assert!(state.timeline.get(Id(99)).is_none());
			assert!(state.timeline.get_display(Id(99)).is_none());
			assert!(state.timeline.get(Id(150)).is_some());
			state.history(None);
			let request = state.request;
			apply(&mut state, Event::Resync);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: vec![message(99)],
				},
			);
			assert_eq!(state.timeline.row_count(), 0);
			assert_eq!(state.drafts[&Id(1)], "unsent draft");
		}
	}

	fn apply(state: &mut State, event: Event) {
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	}

	#[test]
	fn confirmed_send_survives_replacing_history_pages_before_gateway_echo() {
		for echoed in [false, true] {
			for included_in_page in [false, true] {
				let mut state = State {
					user: Some(message(1).author),
					auth: auth::AuthState::Authenticated,
					gateway_connected: true,
					selected: Some(Id(1)),
					channels: vec![Channel {
						id: Id(1),
						guild: None,
						parent_id: None,
						kind: 1,
						name: "Synthetic DM".into(),
						position: 0,
						recipients: vec![],
						last_message: None,
						icon: None,
						member_list_id: None,
						tags: None,
						message_count: None,
					}],
					..State::default()
				};
				state.history(None);
				let request = state.request;
				state.drafts.insert(Id(1), "Sent response".into());
				let Some(Command::Send { nonce, .. }) = state.prepare_send() else {
					panic!("send during recent history load expected");
				};
				let mut sent = message(100);
				sent.nonce = Some(nonce.clone());
				sent.content = "Sent response".into();
				if echoed {
					apply(&mut state, Event::Message(sent.clone()));
				}
				apply(
					&mut state,
					Event::SendResult {
						nonce,
						result: Ok(sent),
					},
				);
				assert!(state.pending.is_empty());
				assert_eq!(
					state.timeline.get(Id(100)).unwrap().content,
					"Sent response"
				);
				let mut messages: Vec<_> = (50..99).map(message).collect();
				if included_in_page {
					messages.push(message(100));
				}
				apply(
					&mut state,
					Event::History {
						channel: Id(1),
						request,
						older: false,
						messages,
					},
				);
				assert_eq!(
					state.timeline.get(Id(100)).unwrap().content,
					"Sent response"
				);
				assert_eq!(state.freshness, Freshness::Fresh);
				assert!(state.timeline.row_count() <= session_cache::MAX_MESSAGES);
				assert!(state.timeline.retained_bytes() <= session_cache::MAX_BYTES);
			}
		}
	}
	pub(super) fn message(id: u64) -> Message {
		Message {
			reply_preview: None,
			poll: None,
			sticker_items: Vec::new(),
			reactions: Some(vec![]),
			id: Id(id),
			channel: Id(1),
			author: User {
				username: None,
				primary_guild: None,
				cosmetics: None,
				id: Id(2),
				name: "Synthetic".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
			},
			content: "Synthetic history".into(),
			edited: false,
			edited_at: None,
			revision: 0,
			nonce: None,
			reply_to: None,
			kind: 0,
			reply_deleted: false,
			interaction: None,
			forwarded: false,
			unsupported: false,
			components: vec![],
			application_id: None,
			flags: 0,
			ephemeral: false,
			extra_content: Default::default(),
			embeds: vec![],
			attachments: vec![],
			author_nick: None,
			author_roles: vec![],
			mention_roles: vec![],
			mention_everyone: false,
			suppress_notifications: false,
			mentions: Vec::new(),
			embeds_suppressed: false,
		}
	}
	#[test]
	fn members_reject_late_requests_and_permission_invalidations() {
		let user = message(1).author;
		let mut state = State {
			user: Some(user.clone()),
			gateway_connected: true,
			auth: auth::AuthState::Authenticated,
			channels: vec![Channel {
				last_message: None,
				id: Id(1),
				guild: None,
				parent_id: None,
				position: 0,
				name: "DM".into(),
				kind: 1,
				recipients: vec![user.clone()],
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			..State::default()
		};
		state.select(Id(1));
		state.request_members();
		assert_eq!(state.members.as_ref().unwrap().slots.len(), 1);
		let previous = state.members.clone().unwrap();
		state.close_members();
		apply(&mut state, Event::Members(previous.clone()));
		assert!(state.members.is_none());
		state.request_members();
		apply(&mut state, Event::PermissionsChanged);
		apply(&mut state, Event::Members(previous));
		assert!(state.members.is_none());
		apply(&mut state, Event::Resumed);
		assert!(state.members.is_none());
		state.request_members();
		apply(
			&mut state,
			Event::RecipientAdded {
				channel: Id(1),
				user: User {
					username: None,
					primary_guild: None,
					cosmetics: None,
					id: Id(3),
					name: "Other".into(),
					avatar: None,
					webhook: false,
					kind: Default::default(),
					discriminator: 0,
				},
			},
		);
		assert_eq!(state.members.as_ref().unwrap().slots.len(), 2);
		apply(
			&mut state,
			Event::RecipientRemoved {
				channel: Id(1),
				user: Id(3),
			},
		);
		assert_eq!(state.members.as_ref().unwrap().slots.len(), 1);
		let user = state.members.as_ref().unwrap().slots[0]
			.as_ref()
			.and_then(|slot| match slot {
				MemberSlot::Person(member) => Some(member.user.clone()),
				MemberSlot::Group(_) => None,
			})
			.unwrap();
		let kept = MemberList {
			guild: Some(Id(10)),
			channel: Id(1),
			request: 7,
			start: 0,
			slots: vec![Some(MemberSlot::Person(Member {
				roles: vec![],
				user: user.clone(),
				nick: Some("Kept".into()),
				status: None,
				custom_status: None,
				activities: vec![],
				clients: model::ClientPlatforms::default(),
			}))],
			total: 250,
			lazy: true,
			freshness: Freshness::Fresh,
			groups: vec![],
			ranges: vec![[0, 99]],
		};
		state.members = Some(kept.clone());
		state.member_chunks.merge(&kept);
		let holes = MemberList {
			start: 100,
			slots: vec![None; 100],
			ranges: vec![[100, 199]],
			freshness: Freshness::Loading,
			..kept.clone()
		};
		apply(&mut state, Event::Members(holes));
		match state.member_slot(0) {
			Some(MemberSlot::Person(member)) => assert_eq!(member.nick.as_deref(), Some("Kept")),
			_ => panic!("loading snapshot keeps the scrolled-away chunk"),
		}
		assert_eq!(state.members.as_ref().unwrap().ranges, vec![[0, 99]]);
		let mut later_slots = vec![None; 100];
		later_slots[0] = Some(MemberSlot::Person(Member {
			roles: vec![],
			user: user.clone(),
			nick: Some("Later".into()),
			status: None,
			custom_status: None,
			activities: vec![],
			clients: model::ClientPlatforms::default(),
		}));
		apply(
			&mut state,
			Event::Members(MemberList {
				start: 100,
				slots: later_slots,
				ranges: vec![[100, 199]],
				freshness: Freshness::Fresh,
				..kept.clone()
			}),
		);
		match state.member_slot(100) {
			Some(MemberSlot::Person(member)) => assert_eq!(member.nick.as_deref(), Some("Later")),
			_ => panic!("a snapshot for a left range stays in the cache"),
		}
		assert_eq!(state.members.as_ref().unwrap().ranges, vec![[0, 99]]);
		apply(
			&mut state,
			Event::Members(MemberList {
				start: 0,
				slots: vec![None; 100],
				ranges: vec![[0, 99]],
				freshness: Freshness::Loading,
				..kept
			}),
		);
		match state.member_slot(0) {
			Some(MemberSlot::Person(member)) => assert_eq!(member.nick.as_deref(), Some("Kept")),
			_ => panic!("a loading snapshot for the open range keeps the cached row"),
		}
	}

	#[test]
	fn voice_navigation_survives_ready_but_revocation_ends_the_call_before_restoration() {
		let channel = Channel {
			id: Id(1),
			guild: Some(Id(10)),
			parent_id: None,
			kind: 2,
			name: "Synthetic voice".into(),
			position: 0,
			recipients: vec![],
			last_message: None,
			icon: None,
			member_list_id: None,
			tags: None,
			message_count: None,
		};
		let guild = Guild {
			default_message_notifications: None,
			stickers: None,
			id: Id(10),
			name: "Synthetic".into(),
			icon: None,
			emojis: None,
		};
		let user = message(1).author;
		let mut state = State {
			user: Some(user.clone()),
			channels: vec![channel.clone()],
			guilds: vec![guild.clone()],
			selected: Some(Id(1)),
			auth: auth::AuthState::Authenticated,
			gateway_connected: true,
			..State::default()
		};
		grant_permissions(&mut state);
		assert!(state.start_call(Id(1), false).is_some());
		let permissions = model::permissions::Snapshot {
			guilds: state.permissions.guilds.values().cloned().collect(),
			channels: state.permissions.channels.values().cloned().collect(),
		};
		apply(
			&mut state,
			Event::Ready {
				permissions,
				user,
				guilds: vec![guild],
				channels: vec![channel.clone()],
			},
		);
		assert_eq!(
			state.selected,
			Some(Id(1)),
			"A visible guild voice selection survives READY"
		);
		assert!(!state.history_pending);
		state.voice.roster.push(voice::RosterEntry {
			guild: Id(10),
			channel: Id(1),
			member: None,
			participant: voice::Participant {
				user: Id(2),
				muted: false,
				deafened: false,
				server_muted: false,
				server_deafened: false,
				stage: Default::default(),
				video: false,
				streaming: false,
			},
		});
		apply(&mut state, Event::Unavailable(Id(1)));
		assert!(state.voice.active.is_none());
		assert!(state.voice.roster.is_empty());
		assert!(!state.can_call(Id(1)));
		assert!(matches!(state.history(None), Command::CancelSearch));
		apply(&mut state, Event::ChannelRestored(channel));
		assert!(
			state.voice.active.is_none(),
			"Restoring metadata never rejoins a call"
		);
		assert!(state.can_call(Id(1)));
		assert!(
			matches!(
				state.select(Id(1)),
				Some(Command::History { channel: Id(1), .. })
			),
			"Voice navigation requests its channel chat history"
		);
		assert_eq!(state.selected, Some(Id(1)));
		assert!(state.history_pending);
		assert!(state.start_call(Id(1), false).is_some());
	}

	#[test]
	fn ready_replacement_and_removed_reload_cannot_restore_unavailable_history() {
		{
			let channel = |kind| Channel {
				id: Id(1),
				guild: None,
				parent_id: None,
				kind,
				name: "Synthetic".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			};
			let user = || User {
				username: None,
				primary_guild: None,
				cosmetics: None,
				id: Id(2),
				name: "Synthetic".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
			};
			for replacement in [None, Some(channel(4))] {
				let mut state = State {
					user: Some(user()),
					channels: vec![channel(1)],
					selected: Some(Id(1)),
					auth: auth::AuthState::Authenticated,
					gateway_connected: true,
					freshness: Freshness::Fresh,
					..State::default()
				};
				state.timeline.insert(message(80), true, false).unwrap();
				state.drafts.insert(Id(1), "Preserve this draft".into());
				state.reply = Some(Reply::to(Id(80)));
				assert!(matches!(state.history(None), Command::History { .. }));
				let request = state.request;
				apply(
					&mut state,
					Event::Ready {
						permissions: model::permissions::Snapshot::default(),
						user: user(),
						guilds: vec![],
						channels: replacement.into_iter().collect(),
					},
				);
				assert!(state.selected.is_none());
				assert!(state.timeline.is_empty());
				assert!(state.reply.is_none());
				assert_eq!(state.freshness, Freshness::Unavailable);
				assert!(!state.history_pending);
				assert_ne!(state.request, request);
				assert_eq!(state.drafts[&Id(1)], "Preserve this draft");
				assert!(
					matches!(state.history(None), Command::CancelSearch),
					"Reload cannot schedule HTTP or a cache load for an unavailable channel"
				);
				apply(
					&mut state,
					Event::History {
						channel: Id(1),
						request,
						older: false,
						messages: vec![message(99)],
					},
				);
				apply(&mut state, Event::Message(message(100)));
				assert!(state.timeline.is_empty());
				assert!(!state.history_pending);
				assert_eq!(state.freshness, Freshness::Unavailable);
			}

			let mut state = State {
				user: Some(user()),
				channels: vec![channel(1)],
				selected: Some(Id(1)),
				auth: auth::AuthState::Authenticated,
				gateway_connected: true,
				freshness: Freshness::Fresh,
				..State::default()
			};
			state.timeline.insert(message(80), true, false).unwrap();
			state.history(None);
			let old_request = state.request;
			apply(
				&mut state,
				Event::Ready {
					permissions: model::permissions::Snapshot::default(),
					user: user(),
					guilds: vec![],
					channels: vec![channel(1)],
				},
			);
			assert_eq!(state.selected, Some(Id(1)));
			assert_eq!(state.freshness, Freshness::Stale);
			assert!(!state.history_pending);
			assert_ne!(state.request, old_request);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request: old_request,
					older: false,
					messages: vec![message(99)],
				},
			);
			assert!(state.timeline.get(Id(99)).is_none());
			assert_eq!(state.freshness, Freshness::Stale);
			let Command::History { request, .. } = state.history(None) else {
				panic!()
			};
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: vec![message(100)],
				},
			);
			assert_eq!(state.freshness, Freshness::Fresh);
			assert!(state.timeline.get(Id(100)).is_some());
			apply(&mut state, Event::Unavailable(Id(1)));
			assert_eq!(
				state.selected,
				Some(Id(1)),
				"The existing unavailable state may retain its old selection"
			);
			assert!(matches!(state.history(None), Command::CancelSearch));
			assert!(!state.history_pending);
			assert!(state.timeline.is_empty());
			assert_eq!(state.freshness, Freshness::Unavailable);
			assert!(matches!(
				State::default().history(None),
				Command::CancelSearch
			));
		}
		{
			let mut state = State::default();
			apply(
				&mut state,
				Event::Ready {
					permissions: model::permissions::Snapshot::default(),
					user: User {
						username: None,
						primary_guild: None,
						cosmetics: None,
						id: Id(2),
						name: "Synthetic".into(),
						avatar: None,
						webhook: false,
						kind: Default::default(),
						discriminator: 0,
					},
					guilds: vec![],
					channels: vec![Channel {
						last_message: None,
						id: Id(1),
						guild: None,
						parent_id: None,
						position: 0,
						name: "Synthetic".into(),
						kind: 1,
						recipients: vec![],
						icon: None,
						member_list_id: None,
						tags: None,
						message_count: None,
					}],
				},
			);
			state.select(Id(1));
			let old_request = state.request;
			apply(&mut state, Event::Disconnected);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request: old_request,
					older: false,
					messages: vec![message(99)],
				},
			);
			assert_eq!(state.freshness, Freshness::Stale);
			assert!(state.timeline.is_empty());
			assert!(!state.can_load_older());

			apply(&mut state, Event::Resumed);
			state.history(None);
			let request = state.request;
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: (51..101).map(message).collect(),
				},
			);
			assert_eq!(state.freshness, Freshness::Fresh);
			assert!(state.can_load_older());
			assert!(matches!(
				state.older_history(),
				Some(Command::History {
					before: Some(Id(51)),
					..
				})
			));
			assert!(state.older_history().is_none()); // no duplicate concurrent page
			let request = state.request;
			apply(
				&mut state,
				Event::HistoryFailed {
					channel: Id(1),
					request: old_request,
					failure: auth::Failure::Forbidden,
				},
			);
			assert_eq!(state.freshness, Freshness::Loading);
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: true,
					messages: vec![message(50)],
				},
			);
			assert_eq!(state.timeline.len(), 51);
			assert!(!state.can_load_older()); // short final page
			apply(
				&mut state,
				Event::DeleteBulk {
					channel: Id(1),
					ids: vec![Id(50), Id(51)],
				},
			);
			assert_eq!(state.timeline.len(), 49);
			assert!(state.timeline.get(Id(50)).is_none());

			state.history(None);
			let request = state.request;
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: false,
					messages: vec![message(100)],
				},
			);
			assert_eq!(state.timeline.len(), 1); // old records absent from fresh page disappear
			state.history(Some(Id(100)));
			let request = state.request;
			apply(
				&mut state,
				Event::History {
					channel: Id(1),
					request,
					older: true,
					messages: vec![message(100)],
				},
			);
			assert_eq!(state.freshness, Freshness::Stale); // before boundary cannot repeat itself
			state.history(None);
			let request = state.request;
			apply(
				&mut state,
				Event::HistoryFailed {
					channel: Id(1),
					request,
					failure: auth::Failure::Forbidden,
				},
			);
			assert_eq!(state.freshness, Freshness::Unavailable);
			assert!(state.timeline.is_empty());
			apply(&mut state, Event::Message(message(200)));
			assert!(state.timeline.is_empty()); // queued live content cannot undo revocation
		}
	}
}
