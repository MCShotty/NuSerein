#![no_main]

use client_core::{Envelope, Event, State, auth::Failure, permissions::Event as PermissionEvent};
use libfuzzer_sys::fuzz_target;
use model::{Channel, Guild, Id, Message, MessagePatch, Patch, User, permissions as p};
use std::hash::{Hash, Hasher};

fn user() -> User {
	User {
		member_avatar: None,
		username: None,
		webhook: false,
		kind: Default::default(),
		id: Id(2),
		name: "Synthetic".into(),
		avatar: None,
		discriminator: 0,
		primary_guild: None,
		cosmetics: None,
	}
}

fn ready() -> Event {
	Event::Ready {
		user: user(),
		guilds: vec![Guild {
			default_message_notifications: None,
			stickers: None,
			id: Id(10),
			name: "Synthetic".into(),
			icon: None,
			emojis: None,
		}],
		channels: (20..23)
			.map(|id| Channel {
				id: Id(id),
				guild: Some(Id(10)),
				parent_id: None,
				kind: 0,
				name: "Synthetic".into(),
				icon: None,
				position: 0,
				recipients: vec![],
				last_message: None,
				message_count: None,
				member_list_id: None,
				tags: None,
			})
			.collect(),
		permissions: p::Snapshot {
			guilds: vec![p::Guild {
				id: Id(10),
				owner: Some(Id(999)),
				roles: Some(vec![p::Role {
					icon: None,
					unicode_emoji: None,
					id: Id(10),
					bits: p::VIEW_CHANNEL | p::READ_MESSAGE_HISTORY | p::SEND_MESSAGES,
					name: String::new(),
					color: 0,
					secondary_color: None,
					tertiary_color: None,
					position: 0,
					hoist: false,
				}]),
				member: Some(p::Member {
					roles: vec![],
					timeout_until: None,
				}),
			}],
			channels: (20..23)
				.map(|id| p::Channel {
					id: Id(id),
					guild: Id(10),
					overwrites: Some(vec![]),
				})
				.collect(),
		},
	}
}

fn payload(selector: u8, remaining: &mut usize) -> String {
	// Repeated bytes reach the actual 64 KiB admission boundary and aggregate
	// eviction without needing a huge corpus. Total expansion is capped per input.
	let size = [0, 1, 1024, 65_535, 65_536, 65_537][usize::from(selector % 6)].min(*remaining);
	*remaining -= size;
	"x".repeat(size)
}

fn message(id: Id, channel: Id, content: String) -> Message {
	Message {
		reply_preview: None,
		poll: None,
		sticker_items: vec![],
		flags: 0,
		ephemeral: false,
		components: vec![],
		application_id: None,
		id,
		channel,
		author: user(),
		author_nick: None,
		author_roles: vec![],
		content,
		kind: 0,
		reactions: Some(vec![]),
		edited: false,
		edited_at: None,
		revision: 0,
		nonce: None,
		reply_to: None,
		interaction: None,
		reply_deleted: false,
		forwarded: false,
		unsupported: false,
		extra_content: Default::default(),
		embeds: vec![],
		attachments: vec![],
		mention_roles: vec![],
		mention_everyone: false,
		suppress_notifications: false,
		mentions: vec![],
		embeds_suppressed: false,
	}
}

fn apply(state: &mut State, event: Event) {
	// A message/page cannot replace bodies already known deleted in this window.
	// Do not carry this oracle across selection, logout, or deliberate cache reset.
	let guarded: Vec<Id> = match &event {
		Event::Message(message) if state.selected == Some(message.channel) => state
			.timeline
			.is_deleted(message.id)
			.then_some(message.id)
			.into_iter()
			.collect(),
		Event::History {
			channel, messages, ..
		} if state.selected == Some(*channel) => messages
			.iter()
			.filter(|message| state.timeline.is_deleted(message.id))
			.map(|message| message.id)
			.collect(),
		_ => vec![],
	};
	state.apply(Envelope {
		generation: state.generation,
		event,
	});
	for id in guarded {
		assert!(state.timeline.get(id).is_none());
	}
}

fn invariants(state: &State) {
	assert!(state.timeline.row_count() <= session_cache::MAX_MESSAGES);
	assert!(state.timeline.bytes() <= session_cache::MAX_BYTES);
	assert!(state.resident_window_count() <= 2);
	assert!(state.resident_history_rows() <= 1475);
	assert!(state.resident_history_bytes() <= 16 * 1024 * 1024);
	let mut previous = None;
	for id in state.timeline.row_ids() {
		assert!(previous.is_none_or(|old| old < id));
		previous = Some(id);
		if state.timeline.is_deleted(id) {
			assert!(state.timeline.get(id).is_none());
		}
	}
	assert_eq!(state.timeline.len(), state.timeline.iter().count());
	for message in state.timeline.iter() {
		assert_eq!(Some(message.channel), state.selected);
		assert!(!state.timeline.is_deleted(message.id));
		assert!(state.can_view(message.channel));
	}
}

// Compare stable public reading state for deliberately stale events. The reducer
// may clear its per-event effect buffer, so do not demand whole-State equality.
fn fingerprint(state: &State) -> u64 {
	let mut hash = std::collections::hash_map::DefaultHasher::new();
	// Even an ignored current-generation response increments the UI revision.
	(
		state.generation,
		state.selected,
		state.request,
		state.history_pending,
		state.gateway_connected,
	)
		.hash(&mut hash);
	for channel in &state.channels {
		channel.id.hash(&mut hash);
		state.personal_channel_flags(channel.id).hash(&mut hash);
	}
	for id in state.timeline.row_ids() {
		id.hash(&mut hash);
	}
	for message in state.timeline.iter() {
		(
			message.id,
			message.channel,
			message.revision,
			&message.content,
		)
			.hash(&mut hash);
	}
	(
		state.resident_window_count(),
		state.resident_history_rows(),
		state.resident_history_bytes(),
	)
		.hash(&mut hash);
	hash.finish()
}

fuzz_target!(|data: &[u8]| {
	if data.len() > 16 * 1024 {
		return;
	}
	let mut state = State::default();
	apply(&mut state, ready());
	state.select(Id(20));
	let request = state.request;
	apply(
		&mut state,
		Event::History {
			channel: Id(20),
			request,
			older: false,
			messages: vec![message(Id(1), Id(20), "seed".into())],
		},
	);
	let mut remaining = 24 * 1024 * 1024;
	// Eight-byte operations: opcode, channel, ID little-endian, payload size,
	// page count, flags, spare. Partial trailing operations are intentionally ignored.
	for op in data.as_chunks::<8>().0.iter().take(256) {
		let channel = Id(20 + u64::from(op[1] % 3));
		let id = Id(1 + u64::from(u16::from_le_bytes([op[2], op[3]])));
		match op[0] % 34 {
			0 => {
				state.select(channel);
			}
			1 => {
				let before = (op[6] & 1 != 0).then_some(Id(id.0 + 51));
				state.history(before);
			}
			2 | 3 => {
				let stale = op[0] % 34 == 3;
				let prior = stale.then(|| fingerprint(&state));
				let request = if stale {
					state.request.wrapping_sub(1)
				} else {
					state.request
				};
				let older = state.history_before.is_some();
				let messages = (0..usize::from(op[5] % 52))
					.map(|offset| {
						message(
							Id(id.0 + offset as u64),
							channel,
							payload(op[4], &mut remaining),
						)
					})
					.collect();
				apply(
					&mut state,
					Event::History {
						channel,
						request,
						older,
						messages,
					},
				);
				if let Some(prior) = prior {
					assert_eq!(fingerprint(&state), prior);
				}
			}
			4 => {
				let message = message(id, channel, payload(op[4], &mut remaining));
				apply(&mut state, Event::Message(message));
			}
			5 => {
				let content = match op[6] % 3 {
					0 => Patch::Absent,
					1 => Patch::Null,
					_ => Patch::Value(payload(op[4], &mut remaining)),
				};
				apply(
					&mut state,
					Event::Patch(MessagePatch {
						poll: Patch::Absent,
						sticker_items: Patch::Absent,
						flags: Patch::Absent,
						components: Patch::Absent,
						application_id: Patch::Absent,
						channel,
						id,
						content,
						extra_content: Default::default(),
						reactions: Patch::Absent,
						mentions: Patch::Absent,
						edited: Patch::Absent,
						embeds: Patch::Absent,
						attachments: Patch::Absent,
						embeds_suppressed: Patch::Absent,
					}),
				);
			}
			6 => apply(&mut state, Event::Delete { channel, id }),
			7 => apply(
				&mut state,
				Event::DeleteBulk {
					channel,
					ids: (0..usize::from(op[5] % 101))
						.map(|offset| Id(id.0 + offset as u64))
						.collect(),
				},
			),
			8 | 9 => {
				let revoked = op[0] % 34 == 8;
				apply(
					&mut state,
					Event::Permissions(PermissionEvent::Channel {
						channel,
						guild: Some(Id(10)),
						overwrites: Patch::Value(if revoked {
							vec![p::Overwrite {
								id: Id(2),
								kind: 1,
								allow: 0,
								deny: p::VIEW_CHANNEL,
							}]
						} else {
							vec![]
						}),
					}),
				);
				if revoked && state.selected == Some(channel) {
					assert!(state.timeline.is_empty());
				}
			}
			10 => apply(&mut state, Event::Disconnected),
			11 => apply(&mut state, Event::Resumed),
			12 => apply(&mut state, Event::Resync),
			13 => {
				state.logout();
				assert!(state.timeline.is_empty());
				assert_eq!(state.resident_window_count(), 0);
				assert!(state.pending.is_empty());
				assert!(state.user.is_none());
			}
			14 => apply(&mut state, ready()),
			15 => {
				let before = fingerprint(&state);
				let revision = state.revision;
				state.apply(Envelope {
					generation: state.generation.wrapping_sub(1),
					event: Event::Delete { channel, id },
				});
				assert_eq!(fingerprint(&state), before);
				assert_eq!(state.revision, revision);
			}
			16 => {
				let request = state.request;
				apply(
					&mut state,
					Event::HistoryFailed {
						channel,
						request,
						failure: Failure::Forbidden,
					},
				);
			}
			17 => {
				let mut source = message(Id(id.0 + 1), channel, payload(op[4], &mut remaining));
				source.kind = 19;
				source.reply_to = Some(id);
				source.reply_deleted = true;
				apply(&mut state, Event::Message(source));
			}
			18 => {
				// A bounded live burst also reaches eviction while a page is pending.
				for offset in 0..usize::from(op[5] % 51) {
					let message = message(
						Id(id.0 + offset as u64),
						channel,
						payload(op[4], &mut remaining),
					);
					apply(&mut state, Event::Message(message));
					invariants(&state);
				}
			}
			19 => {
				state.clear_cached_history();
			}
			20 => {
				let mut reply = message(Id(id.0 + 1), channel, "reply".into());
				reply.reply_to = Some(id);
				if op[6] & 1 != 0 {
					reply.reply_preview = model::ReplyPreview::from_message(message(
						id,
						channel,
						payload(op[4], &mut remaining),
					))
					.map(Box::new);
				}
				apply(&mut state, Event::Message(reply));
			}
			21 => {
				let targets: Vec<_> = state
					.timeline
					.iter()
					.filter_map(|message| message.reply_to)
					.take(32)
					.collect();
				for command in state.sync_visible_reply_previews(&targets) {
					if let client_core::Command::ReplyPreview {
						channel,
						message: target,
						request,
					} = command
					{
						let result = match op[6] % 4 {
							0 => Ok(None),
							1 => Err(Failure::Forbidden),
							2 => Err(Failure::Network),
							_ => Ok(model::ReplyPreview::from_message(message(
								target,
								channel,
								payload(op[4], &mut remaining),
							))
							.map(Box::new)),
						};
						apply(
							&mut state,
							Event::ReplyPreview(client_core::reply_previews::ResultEvent {
								channel,
								message: target,
								request: request.wrapping_sub(u64::from(op[7] & 1)),
								result,
							}),
						);
					}
				}
			}
			22 => {
				state.open_reply_target(id);
			}
			23 => {
				state.return_from_reply();
			}
			25 => {
				let flags = u64::from(op[4]) << 32;
				apply(
					&mut state,
					Event::NotificationPreferences(client_core::notifications::Event::Settings {
						entries: vec![client_core::notifications::Setting {
							guild: Some(Id(10)),
							flags: Some(if op[6] & 1 == 0 {
								flags
							} else {
								flags | model::channel_visibility::OPT_IN_CHANNELS_ON
							}),
							overrides_known: true,
							channel_flags: (20..23)
								.map(|id| {
									(
										Id(id),
										Some(
											flags
												| if op[5] & 1 != 0 {
													model::channel_visibility::OPT_IN_ENABLED
												} else {
													0
												},
										),
										Some(false),
									)
								})
								.collect(),
							..Default::default()
						}],
						replace: op[7] & 1 != 0,
					}),
				);
			}
			26 => {
				if let Some(action) = state.personal_visibility_change(channel, op[6] & 1 != 0) {
					state.request_personal_channel_action(Id(10), action);
				}
				let refresh = if op[5] & 1 != 0 {
					state.refresh_channel_preferences(Id(10))
				} else {
					None
				};
				state.flush_channel_preferences();
				if let Some(client_core::Command::ChannelPreferences(request)) =
					refresh.or_else(|| state.poll_channel_preferences(std::time::Instant::now()))
				{
					let mut event = state.demo_channel_preferences(&request);
					match op[4] % 4 {
						1 => event.result = Err(Failure::Ambiguous),
						2 => event.result = Err(Failure::Forbidden),
						3 => {
							let _ = state.retry_channel_preference(Id(10), Some(channel));
						}
						_ => {}
					}
					state.apply(Envelope {
						generation: state.generation.wrapping_sub(u64::from(op[7] & 1)),
						event: Event::ChannelPreferences(event),
					});
				}
				let update = model::guild_preferences::Update {
					guild: Some(Id(10)),
					version: (op[5] & 2 != 0).then_some(u64::from(op[6])),
					fields: model::guild_preferences::Fields {
						hide_muted: if op[4] & 1 == 0 {
							Patch::Null
						} else {
							Patch::Value(op[4] & 2 != 0)
						},
						..Default::default()
					},
					channels_unknown: op[4] & 8 != 0,
					..Default::default()
				};
				apply(
					&mut state,
					Event::NotificationPreferences(client_core::notifications::Event::Updates {
						entries: vec![update],
						replace: false,
					}),
				);
			}

			27 => {
				if let Some(command) = state.prepare_text_send("Synthetic queue rejection") {
					apply(&mut state, Event::CommandRejected(Box::new(command)));
				}
			}
			28 => {
				let auth = state.auth;
				let request = state.request;
				apply(
					&mut state,
					Event::HistoryFailed {
						channel,
						request,
						failure: if op[6] & 1 == 0 {
							Failure::Busy
						} else {
							Failure::ResponseLimit
						},
					},
				);
				assert_eq!(
					state.auth, auth,
					"operation failures cannot revoke authentication"
				);
			}

			29 => {
				apply(
					&mut state,
					Event::MemberIdentity(Box::new(model::MemberIdentity {
						global_name: match op[6] >> 6 {
							0 => model::GlobalNameUpdate::Unchanged,
							1 => model::GlobalNameUpdate::Cleared,
							_ => model::GlobalNameUpdate::Set,
						},
						avatar: match op[6] >> 3 & 3 {
							0 => Patch::Absent,
							1 => Patch::Null,
							2 => Patch::Value("0123456789abcdef0123456789abcdef".into()),
							_ => Patch::Value("../invalid".into()),
						},
						guild: Id(10),
						user: id,
						profile: (op[6] & 64 != 0).then(|| {
							let mut profile = user();
							profile.id = id;
							profile.username = Some("synthetic".into());
							profile
						}),
						nick: if op[6] & 1 == 0 {
							Patch::Null
						} else {
							Patch::Value("Synthetic nickname".into())
						},
						roles: if op[6] & 2 == 0 {
							Patch::Absent
						} else {
							Patch::Value(vec![])
						},
						departed: op[6] & 4 != 0,
					})),
				);
				assert!(
					state.member_labels.bytes() <= 512 * 1024 + 128 * std::mem::size_of::<Id>()
				);
			}
			30 => {
				let started = state
					.member_display_revision()
					.wrapping_sub(u64::from(op[6] & 1));
				let auth = state.auth;
				state.hydrate_member_displays(
					Id(10),
					started,
					vec![model::MemberDisplay {
						guild: Id(10),
						user: id,
						name: "Synthetic account".into(),
						username: Some("synthetic".into()),
						nick: (op[6] & 2 != 0).then(|| "Cached nickname".into()),
					}],
				);
				assert_eq!(state.auth, auth);
			}
			31 => {
				if let Some(client_core::Command::MemberSearch(request)) =
					state.search_members(channel, "Synthetic", 0)
				{
					apply(
						&mut state,
						Event::MemberSearchCooldown {
							request,
							seconds: u32::from(op[6]),
						},
					);
					assert!(!state.member_search[0].finished);
				}
			}

			32 => {
				let selected = state.selected;
				let request = state.request;
				let mut person = user();
				person.id = id;
				person.member_avatar = Some(Box::new(model::GuildAvatar {
					guild: Id(10),
					hash: (op[6] & 1 != 0).then(|| "0123456789abcdef0123456789abcdef".into()),
				}));
				state.apply_force_sync_result(client_core::force_sync_read::ReadResult::Members {
					guild: Id(10),
					items: vec![model::Member {
						user: person,
						roles: vec![],
						nick: Some("Refreshed nickname".into()),
						status: None,
						custom_status: None,
						activities: vec![],
						clients: Default::default(),
					}],
					missing: vec![],
					nick_unknown: match op[6] & 3 {
						0 => vec![],
						1 => vec![id],
						2 => vec![Id(0)],
						_ => vec![id, id],
					},
				});
				assert_eq!(state.selected, selected);
				assert_eq!(state.request, request);
			}
			_ => {
				let prior = fingerprint(&state);
				state.apply(Envelope {
					generation: state.generation.wrapping_sub(1),
					event: Event::ReplyPreview(client_core::reply_previews::ResultEvent {
						channel,
						message: id,
						request: state.request,
						result: Ok(None),
					}),
				});
				assert_eq!(fingerprint(&state), prior);
			}
		}
		invariants(&state);
	}
});
