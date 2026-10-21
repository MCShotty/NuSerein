// Identical synthetic wire inputs for bc35a629 and feat/conversation-qol.
// Copy this function into profile_preview.rs in BOTH evidence worktrees.
// No new-model fields or new-only APIs are referenced here.
fn prime_conversation_qol(state: &mut client_core::State, messaging: &mut ui::MessagingUi) {
	use client_core::{Envelope, Event};
	use model::{Freshness, Id};
	use serde_json::json;

	const CHANNEL: Id = Id(20);
	const GUILD: Id = Id(10);
	const FIRST: u64 = 1_549_000_000_000_000_000;
	const ORIGINAL: u64 = 1_510_000_000_000_000_000;
	const STEP: u64 = 1_258_291_200; // Five minutes in snowflake time.
	let apply = |state: &mut client_core::State, event| {
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	};
	state.auth = client_core::auth::AuthState::Authenticated;
	state.gateway_connected = true;
	state.freshness = Freshness::Fresh;
	if state.selected != Some(CHANNEL) {
		let _ = state.select(CHANNEL);
	}
	state.history_pending = false;
	state.timeline.clear();

	// Same ordinary role events on both revisions. The baseline ignores cosmetic fields.
	for role in [
		json!({"id":"310", "permissions":"0", "name":"Reviewers", "color":10984442,
            "position":3, "hoist":false, "icon":"cccccccccccccccccccccccccccccccc"}),
		json!({"id":"311", "permissions":"0", "name":"Release team", "color":5763719,
            "position":2, "hoist":false, "unicode_emoji":"⭐"}),
	] {
		let payload = serde_json::to_vec(&json!({"guild_id":"10", "role":role})).unwrap();
		let (guild, role) = discord_protocol::permissions::role(&payload).expect("synthetic role");
		apply(
			state,
			Event::Permissions(client_core::permissions::Event::Role { guild, role }),
		);
	}

	// Duplicate nicknames deliberately expose the need for the actual usernames in the picker.
	let people = [
		json!({"member":{"user":{"id":"2", "username":"robin.dev", "global_name":"Robin Ellis"},
            "nick":"Robin", "roles":["310"]}, "presence":{"status":"online", "activities":[]}}),
		json!({"member":{"user":{"id":"3", "username":"river_writes", "global_name":"River"},
            "nick":"Robin", "roles":["311"]}, "presence":{"status":"idle", "activities":[]}}),
	];
	let _ = state.request_members();
	let mut members = state.members.clone().expect("synthetic member request");
	members.slots = vec![Some(model::MemberSlot::Group("online".into()))];
	for person in &people {
		let row = discord_protocol::decode::<discord_protocol::MemberItem>(
			&serde_json::to_vec(person).unwrap(),
		)
		.expect("synthetic member");
		members.slots.push(row.into_slot());
	}
	members.start = 0;
	members.total = members.slots.len() as u64;
	members.lazy = false;
	members.freshness = Freshness::Fresh;
	members.groups = vec![("online".into(), 2)];
	apply(state, Event::Members(members));

	let texts = [
		"Found the original notes — I'll keep the checklist up to date.",
		"Morning! The draft is ready for review.",
		"The screenshots look good. I'll check the remaining copy today.",
		"Thanks! There are a few comments in the release thread.",
		"I can take the first review after lunch.",
		"The notes and examples are all in the same place now.",
		"Sounds good. Let's keep the discussion in this channel.",
		"I'll post an update when the final changes are ready.",
	];
	for (index, content) in texts.into_iter().enumerate() {
		let person = &people[index % people.len()]["member"];
		let mut wire = json!({
			"id":(FIRST + index as u64 * STEP).to_string(), "channel_id":"20",
			"author":person["user"], "member":{"nick":person["nick"], "roles":person["roles"]},
			"content":content, "mentions":[], "type":0
		});
		if index == 0 {
			wire["type"] = json!(19);
			wire["message_reference"] =
				json!({"message_id":ORIGINAL.to_string(), "channel_id":"20", "type":0});
			wire["referenced_message"] = json!({
				"id":ORIGINAL.to_string(), "channel_id":"20", "author":people[1]["member"]["user"],
				"member":{"nick":"Robin", "roles":["311"]}, "mentions":[], "type":0,
				"content":"Let's put the release notes in one place so everyone can find them."
			});
		}
		let message = discord_protocol::decode::<discord_protocol::MessageDto>(
			&serde_json::to_vec(&wire).unwrap(),
		)
		.expect("synthetic message")
		.into_model();
		apply(state, Event::Message(message));
	}

	// Make service read-state markers coherent with each conversation's known latest ID.
	for id in [Id(21), Id(27), Id(28)] {
		let mut channel = state
			.channel(id)
			.expect("synthetic sidebar channel")
			.clone();
		channel.last_message = Some(Id(FIRST + 100 * STEP));
		apply(state, Event::ChannelCreated(channel));
	}
	state.last_viewed_threads.clear();
	state
		.apply_notification_preferences(client_core::notifications::Event::Settings {
			replace: true,
			entries: vec![client_core::notifications::Setting {
				overrides_known: true,
				guild: Some(GUILD),
				muted: Some(false),
				level: Some(3),
				suppress_everyone: Some(false),
				suppress_roles: Some(false),
				hide_muted_channels: Some(true),
				channels: vec![(Id(21), Some(true), Some(3))],
				..Default::default()
			}],
		})
		.expect("synthetic notification preferences");
	let read = discord_protocol::decode::<discord_protocol::read_state::Snapshot>(
		&serde_json::to_vec(&json!({"version":100, "partial":false, "entries":[
			{"id":"20", "last_message_id":(FIRST + 7 * STEP).to_string(), "mention_count":0},
			{"id":"21", "last_message_id":FIRST.to_string(), "mention_count":2},
			{"id":"27", "last_message_id":FIRST.to_string(), "mention_count":4},
			{"id":"28", "last_message_id":FIRST.to_string(), "mention_count":3}
		]}))
		.unwrap(),
	)
	.expect("synthetic read state");
	apply(
		state,
		Event::ReadState(client_core::read_state::Event::Snapshot {
			entries: Some(
				read.entries
					.into_iter()
					.map(|entry| (entry.id, entry.last_message_id, entry.mention_count))
					.collect(),
			),
			version: read.version,
			partial: read.partial,
		}),
	);
	messaging.reading_preferences.show_members = true;
	state.drafts.insert(CHANNEL, "@R".into());
	state.status = "Offline fixture · no network access";
	assert!(
		state.timeline.get(Id(ORIGINAL)).is_none(),
		"the old original must stay outside history"
	);
	assert_eq!(state.mention_count(Id(21)), 2);
	assert_eq!(state.mention_count(Id(27)), 4);
	assert_eq!(state.mention_count(Id(28)), 3);
}

/// Identical bounded workload for measuring layout invalidation from unrelated state changes.
fn prime_timeline_churn(state: &mut client_core::State) {
	let channel = state.selected.expect("synthetic channel");
	let messages = (1..=500)
		.map(|id| {
			let mut message = test_support::message(id, channel);
			message.content = format!("Synthetic conversation row {id}: a short review update.");
			message.author.cosmetics = None;
			message.author.primary_guild = None;
			message.attachments.clear();
			message.embeds.clear();
			message.reactions = Some(vec![]);
			message
		})
		.collect();
	state.timeline.clear();
	state
		.timeline
		.seed_cache(messages)
		.expect("bounded synthetic timeline");
	state.remember_reading(
		channel,
		client_core::ReadingCursor {
			message: Some(model::Id(250)),
			inset: 5.0,
		},
	);
	state.read_state.reset();
	state.older_exhausted = true;
	state.freshness = model::Freshness::Fresh;
	state.history_pending = false;
}
