use client_core::{Envelope, Event};
use model::Id;
use std::time::Instant;
mod soak;
fn main() {
	if std::env::args().nth(1).as_deref() == Some("--loading") {
		loading();
		return;
	}
	if std::env::args().nth(1).as_deref() == Some("--soak") {
		let seconds = std::env::args()
			.nth(2)
			.map_or(Ok(60), |value| value.parse::<u64>())
			.expect("Soak seconds must be an integer from 1 to 3600");
		assert!(
			(1..=3600).contains(&seconds),
			"Soak seconds must be from 1 to 3600"
		);
		soak::run(std::time::Duration::from_secs(seconds));
		return;
	}
	if std::env::args().nth(1).as_deref() == Some("--navigation") {
		navigation();
		return;
	}
	if std::env::args().nth(1).as_deref() == Some("--wide-channels") {
		wide_channels();
		return;
	}
	if std::env::args().nth(1).as_deref() == Some("--member-watch") {
		member_watch();
		return;
	}
	if std::env::args().nth(1).as_deref() == Some("--member-cache") {
		member_cache();
		return;
	}
	let start = Instant::now();
	let mut state = test_support::demo_state();
	let mut samples = Vec::new();
	for cycle in 0..200_u64 {
		for id in 1..=500 {
			state.apply(Envelope {
				generation: state.generation,
				event: Event::Message(test_support::message(1000 + cycle * 500 + id, Id(20))),
			});
		}
		samples.push(state.timeline.bytes());
		assert!(state.timeline.len() <= 500);
		assert!(state.timeline.bytes() <= 4 * 1024 * 1024);
	}
	let tail = &samples[100..];
	println!(
		"Synthetic reducer replay: 100000 events in {:?}; retained timeline {}..{} estimated bytes, {} records. This is not process RSS, UI frame time, or live compatibility.",
		start.elapsed(),
		tail.iter().min().unwrap(),
		tail.iter().max().unwrap(),
		state.timeline.len()
	);
	state.logout();
	assert_eq!(state.timeline.bytes(), 0);
	assert!(!state.has_unsent());
}

/// Rebuild the production member watch for a busy conversation with cached identities.
fn member_watch() {
	let mut state = loading_state();
	state.timeline.clear();
	for id in 1..=500 {
		let mut message = test_support::message(4_000_000 + id, Id(20));
		message.author.id = Id(10_000 + (id % 128));
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Message(message),
		});
	}
	for id in 0..128 {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::MemberIdentity(Box::new(model::MemberIdentity {
				guild: Id(10),
				user: Id(10_000 + id),
				profile: None,
				avatar: model::Patch::Absent,
				nick: model::Patch::Value(format!("Synthetic {id}")),
				roles: model::Patch::Absent,
				departed: false,
			})),
		});
	}
	let start = Instant::now();
	for _ in 0..10_000 {
		let (guild, channel, users) = std::hint::black_box(state.watched_member_users()).unwrap();
		assert_eq!((guild, channel), (Id(10), Id(20)));
		assert_eq!(users.len(), 128);
		std::hint::black_box(users);
	}
	println!(
		"Synthetic member watch: 10000 rebuilds of 128 distinct authors from a 500-message timeline in {:?}. Excludes UI, network and RSS.",
		start.elapsed()
	);
}

/// Validate display rows after live nickname updates without touching the storage worker.
fn member_cache() {
	let mut state = loading_state();
	for id in 0..128 {
		let mut user = test_support::message(1, Id(20)).author;
		user.id = Id(10_000 + id);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::MemberIdentity(Box::new(model::MemberIdentity {
				guild: Id(10),
				user: user.id,
				profile: Some(user),
				avatar: model::Patch::Absent,
				nick: model::Patch::Value(format!("Synthetic {id}")),
				roles: model::Patch::Absent,
				departed: false,
			})),
		});
	}
	let start = Instant::now();
	for index in 0..100_000 {
		assert!(std::hint::black_box(
			state.has_cached_member_display(Id(10), Id(10_000 + index % 128),)
		));
	}
	println!(
		"Synthetic member-cache validation: 100000 checks across 128 live identities in {:?}. Excludes storage, UI, network and RSS.",
		start.elapsed()
	);
}

fn wide_channels() {
	let mut state = test_support::demo_state();
	let target = state.channel(Id(20)).expect("demo conversation").clone();
	state.channels = (0..20_000_u64)
		.map(|index| {
			let mut channel = target.clone();
			channel.id = Id(100_000 + index);
			channel
		})
		.chain(std::iter::once(target.clone()))
		.collect();
	state.invalidate_navigation();
	let start = Instant::now();
	for id in 1..=10_000 {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Message(test_support::message(1_000_000 + id, Id(20))),
		});
	}
	println!(
		"Synthetic wide-channel reducer: 10000 messages into the last of 20001 channels in {:?}; {} retained records / {} estimated bytes. Excludes startup, RSS, UI and live compatibility.",
		start.elapsed(),
		state.timeline.len(),
		state.timeline.bytes()
	);
	assert_eq!(state.timeline.len(), 500);
	state.logout();
	assert_eq!(state.timeline.bytes(), 0);
}

fn navigation() {
	let mut state = test_support::demo_state();
	let channels = [Id(20), Id(21), Id(22)];
	state.channels = channels
		.into_iter()
		.map(|id| model::Channel {
			id,
			guild: None,
			parent_id: None,
			kind: 1,
			position: 0,
			name: format!("Synthetic conversation {id}"),
			recipients: vec![],
			last_message: None,
			member_list_id: None,
			tags: None,
			message_count: None,
			icon: None,
		})
		.collect();
	state.selected = None;
	state.timeline.clear();
	let mut samples = Vec::with_capacity(10_000);
	let mut hits = 0;
	let mut requests = 0;
	for step in 0..10_003 {
		let channel = channels[step % channels.len()];
		let started = Instant::now();
		let command = state.select(channel);
		let elapsed = started.elapsed();
		let Some(client_core::Command::History {
			channel: selected,
			before: None,
			after: None,
			request,
		}) = command
		else {
			// An untouched dormant window that ends at the latest message reopens as current.
			assert!(
				command.is_none(),
				"Navigation may only revalidate or reopen"
			);
			assert_eq!(state.freshness, model::Freshness::Fresh);
			assert_eq!(state.timeline.len(), 50);
			if step >= 3 {
				samples.push(elapsed);
				hits += 1;
			}
			continue;
		};
		assert_eq!(selected, channel);
		assert_eq!(state.freshness, model::Freshness::Loading);
		assert!(
			state
				.timeline
				.iter()
				.all(|message| message.channel == channel)
		);
		if step >= 3 {
			samples.push(elapsed);
			hits += usize::from(state.timeline.row_count() != 0);
			requests += 1;
		}
		state.apply(Envelope {
			generation: state.generation,
			event: Event::History {
				channel,
				request,
				older: false,
				messages: (1..=50)
					.map(|id| test_support::message(channel.0 * 1000 + id, channel))
					.collect(),
			},
		});
		assert_eq!(state.timeline.len(), 50);
		assert_eq!(state.freshness, model::Freshness::Fresh);
	}
	samples.sort_unstable();
	println!(
		"Synthetic navigation: 10000 selections across three 50-message conversations; immediate previews {hits}/10000; revalidation requests {requests}; select median {:?}, p95 {:?}. Measures core selection only, not native display latency, RSS or Discord compatibility.",
		samples[5000], samples[9499],
	);
	println!(
		"Navigation retained: {} dormant windows; {} active plus dormant rows; {} estimated retained bytes (not RSS).",
		state.resident_window_count(),
		state.resident_history_rows(),
		state.resident_history_bytes()
	);
	state.logout();
	assert_eq!(state.timeline.row_count(), 0);
	assert!(!state.has_unsent());
	hover_navigation();
}

/// First visits to conversations, each hovered for the dwell before its click, against a sweep
/// across rows that are never clicked. Counts requests and opens that need no wait.
fn hover_navigation() {
	use client_core::{Command, preload::CONVERSATION_DWELL};
	let mut state = test_support::demo_state();
	state.demo = false;
	state.auth = client_core::auth::AuthState::Authenticated;
	state.gateway_connected = true;
	state.selected = None;
	state.timeline.clear();
	state.channels = (100..140)
		.map(|id| model::Channel {
			id: Id(id),
			guild: None,
			parent_id: None,
			kind: 1,
			position: 0,
			name: format!("Synthetic conversation {id}"),
			recipients: vec![],
			last_message: Some(Id(id * 1000 + 50)),
			member_list_id: None,
			tags: None,
			message_count: None,
			icon: None,
		})
		.collect();
	state.invalidate_navigation();
	let preloaded = |command: Option<Command>| match command? {
		Command::Prefetch(command) => match *command {
			Command::History { request, .. } => Some(request),
			_ => None,
		},
		_ => None,
	};
	let page = |channel: Id, request| Event::History {
		channel,
		request,
		older: false,
		messages: (1..=50)
			.map(|id| test_support::message(channel.0 * 1000 + id, channel))
			.collect(),
	};
	let start = std::time::Instant::now();
	let (mut requests, mut instant) = (0, 0);
	for (index, channel) in (100..112).map(Id).enumerate() {
		let at = start + std::time::Duration::from_secs(index as u64);
		assert!(
			state
				.preload_conversation(Some(channel), None, at)
				.is_none()
		);
		if let Some(request) =
			preloaded(state.preload_conversation(Some(channel), None, at + CONVERSATION_DWELL))
		{
			requests += 1;
			state.apply(Envelope {
				generation: state.generation,
				event: page(channel, request),
			});
		}
		match state.select(channel) {
			None => {
				assert_eq!(state.selected, Some(channel));
				instant += 1;
			}
			Some(Command::History { request, .. }) => {
				requests += 1;
				state.apply(Envelope {
					generation: state.generation,
					event: page(channel, request),
				});
			}
			Some(_) => panic!("first visits only load history"),
		}
		assert_eq!(state.freshness, model::Freshness::Fresh);
	}
	assert_eq!((requests, instant), (12, 12));
	let later = start + std::time::Duration::from_secs(120);
	let mut sweep = 0;
	for (index, channel) in (112..140).map(Id).enumerate() {
		let at = later + std::time::Duration::from_millis(index as u64 * 200);
		state.preload_conversation(Some(channel), None, at);
		if let Some(request) =
			preloaded(state.preload_conversation(Some(channel), None, at + CONVERSATION_DWELL))
		{
			sweep += 1;
			state.apply(Envelope {
				generation: state.generation,
				event: page(channel, request),
			});
		}
	}
	println!(
		"Synthetic hover navigation: 12 first visits rested on for the dwell then clicked: {instant}/12 opened without waiting, {requests} history requests (a click-only first visit sends 1 each); a 28-row sweep without clicks sent {sweep} preloads (cap 12/min). Synthetic responses arrive before the click; excludes network latency and Discord compatibility."
	);
	println!(
		"Hover navigation retained: {} dormant windows; {} estimated retained bytes (not RSS).",
		state.resident_window_count(),
		state.resident_history_bytes()
	);
}

/// Busy navigation and member-cache availability. The service page is deliberately withheld
/// until after selection/lookup observations; no wall-clock delay is inferred from this fixture.
fn loading() {
	println!(
		"Synthetic loading workload: 1000 busy 50-row channel switches and 100 sidebar cycles; one warmup + five measured repetitions; release core-only, no SQLite, HTTP, rendering, presented-frame latency or live Discord compatibility."
	);
	let _ = loading_sample();
	let mut times = Vec::with_capacity(5);
	for sample in 0..5 {
		let result = loading_sample();
		println!(
			"loading sample={sample} elapsed_ms={:.3} immediate_previews={}/1000 history_requests={} delayed_preview_observations={} known_member_hits={}/1000 author_requests={} sidebar_chunk_hits={}/100 pane_reopen_hits={}/100 subscription_requests={} select_p50_us={:.3} select_p95_us={:.3} lookup_p50_us={:.3} lookup_p95_us={:.3} retained_rows={} retained_bytes={}",
			result.elapsed.as_secs_f64() * 1000.0,
			result.previews,
			result.histories,
			result.delayed_previews,
			result.known_members,
			result.author_requests,
			result.chunk_hits,
			result.reopen_hits,
			result.subscriptions,
			result.selects[500].as_secs_f64() * 1e6,
			result.selects[949].as_secs_f64() * 1e6,
			result.lookups[500].as_secs_f64() * 1e6,
			result.lookups[949].as_secs_f64() * 1e6,
			result.retained_rows,
			result.retained_bytes,
		);
		times.push(result.elapsed);
	}
	times.sort_unstable();
	println!(
		"Synthetic loading median elapsed {:?}; availability/request counters above are deterministic, CPU timings exclude the withheld service response delay. Retained bytes are core estimates, not process RSS.",
		times[2]
	);
}

#[derive(Default)]
struct LoadingSample {
	elapsed: std::time::Duration,
	previews: usize,
	histories: usize,
	delayed_previews: usize,
	known_members: usize,
	author_requests: usize,
	chunk_hits: usize,
	reopen_hits: usize,
	subscriptions: usize,
	selects: Vec<std::time::Duration>,
	lookups: Vec<std::time::Duration>,
	retained_rows: usize,
	retained_bytes: usize,
}
fn loading_state() -> client_core::State {
	let mut state = test_support::demo_state();
	state.demo = false;
	state.auth = client_core::auth::AuthState::Authenticated;
	state.gateway_connected = true;
	state
}
fn loading_person(id: u64) -> model::Member {
	let mut user = test_support::message(2, Id(20)).author;
	user.id = Id(id);
	user.name = format!("Synthetic member {id}");
	user.username = Some(format!("synthetic_{id}").into());
	user.cosmetics = None;
	model::Member {
		user,
		nick: Some(format!("Synthetic nickname {id}")),
		roles: vec![],
		status: None,
		custom_status: None,
		activities: vec![],
		clients: Default::default(),
	}
}
fn loading_event(state: &mut client_core::State, event: Event) {
	state.apply(Envelope {
		generation: state.generation,
		event,
	});
}
fn loading_page(state: &mut client_core::State, channel: Id, request: u64, latest: u64) {
	loading_event(
		state,
		Event::History {
			channel,
			request,
			older: false,
			messages: ((latest - 49)..=latest)
				.map(|id| test_support::message(id, channel))
				.collect(),
		},
	);
	assert_eq!(state.timeline.len(), 50);
	assert_eq!(state.freshness, model::Freshness::Fresh);
}
fn loading_resolve_author(state: &mut client_core::State, counters: &mut LoadingSample) {
	if let Some(client_core::Command::MemberSearch(request)) =
		state.request_author_members(&[Id(987654321)])
	{
		counters.author_requests += 1;
		loading_event(
			state,
			Event::MemberSearch {
				request,
				result: Ok(vec![loading_person(987654321)]),
			},
		);
	}
}
fn loading_sample() -> LoadingSample {
	use client_core::Command;
	let mut result = LoadingSample::default();
	let mut state = loading_state();
	let channels = [Id(20), Id(21)];
	let mut latest = [20_050u64, 21_050];
	// Seed two equally sized, authoritative windows before measuring busy revisits.
	for index in 0..2 {
		let _ = state.select(channels[index]);
		let Command::History { request, .. } = state.history(None) else {
			unreachable!()
		};
		loading_page(&mut state, channels[index], request, latest[index]);
	}
	loading_resolve_author(&mut state, &mut result);
	let started = Instant::now();
	for step in 0..1000 {
		let index = step % 2;
		let channel = channels[index];
		latest[index] += 1;
		loading_event(
			&mut state,
			Event::Message(test_support::message(latest[index], channel)),
		);
		let at = Instant::now();
		let command = state.select(channel);
		result.selects.push(at.elapsed());
		let preview = state.timeline.row_count() != 0;
		result.previews += usize::from(preview);
		assert!(
			state
				.timeline
				.iter()
				.all(|message| message.channel == channel)
		);
		let at = Instant::now();
		let known = state
			.member_nickname(channel, Id(987654321))
			.flatten()
			.is_some();
		result.lookups.push(at.elapsed());
		result.known_members += usize::from(known);
		// Simulate several UI observations while the current HTTP response is held.
		// These observations must neither create rows nor imply a network-time measurement.
		for _ in 0..8 {
			result.delayed_previews +=
				usize::from(std::hint::black_box(state.timeline.row_count() != 0));
			assert_eq!(state.timeline.row_count() != 0, preview);
		}
		if let Some(Command::History { request, .. }) = command {
			result.histories += 1;
			assert!(state.history_pending);
			loading_page(&mut state, channel, request, latest[index]);
		} else {
			assert!(!state.history_pending);
		}
		loading_resolve_author(&mut state, &mut result);
	}
	result.retained_rows = state.resident_history_rows();
	result.retained_bytes = state.resident_history_bytes();
	assert!(result.retained_rows <= 1475);
	assert!(result.retained_bytes <= 16 * 1024 * 1024);
	// Each cycle isolates sidebar range retention from the composer query result.
	for _ in 0..100 {
		let mut pane = loading_state();
		result.subscriptions += usize::from(pane.request_members().is_some());
		let request = pane.members.as_ref().unwrap().request;
		let list = |start, user, ranges| model::MemberList {
			missing_identity: vec![],
			guild: Some(Id(10)),
			channel: Id(20),
			request,
			start,
			total: 300,
			slots: vec![Some(model::MemberSlot::Person(loading_person(user)))],
			lazy: true,
			freshness: model::Freshness::Fresh,
			groups: vec![],
			ranges,
		};
		loading_event(&mut pane, Event::Members(list(0, 987654321, vec![[0, 99]])));
		result.subscriptions += usize::from(pane.focus_member_ranges(100, 110).is_some());
		loading_event(
			&mut pane,
			Event::Members(list(100, 987654322, vec![[100, 199]])),
		);
		assert!(pane.member_slot(0).is_some());
		result.chunk_hits += usize::from(
			pane.member_nickname(Id(20), Id(987654321))
				.flatten()
				.is_some(),
		);
		let _ = pane.close_members();
		result.subscriptions += usize::from(pane.request_members().is_some());
		result.reopen_hits += usize::from(pane.members_cached());
		pane.logout();
	}
	result.elapsed = started.elapsed();
	result.selects.sort_unstable();
	result.lookups.sort_unstable();
	state.logout();
	assert_eq!(state.timeline.row_count(), 0);
	assert!(!state.has_unsent());
	result
}
