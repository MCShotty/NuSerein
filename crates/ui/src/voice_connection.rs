//! Local voice connection details; diagnostic snapshots never generate service commands.
use crate::{MessagingUi, design, i18n};
use client_core::State;
use model::{
	Id,
	voice_diagnostics::{
		ConnectionKind, ConnectionSnapshot, IncomingPackets, Latency, ParticipantStatistics,
	},
};
use std::{
	fmt::Write,
	time::{Duration, Instant},
};

type Context = (u64, Id, u64);
const HISTORY: usize = 60;
const KINDS: [ConnectionKind; 3] = [
	ConnectionKind::Call,
	ConnectionKind::ScreenSend,
	ConnectionKind::ScreenReceive,
];
fn slot(kind: ConnectionKind) -> usize {
	match kind {
		ConnectionKind::Call => 0,
		ConnectionKind::ScreenSend => 1,
		ConnectionKind::ScreenReceive => 2,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::voice_diagnostics::{MediaCounters, PacketTotals, VoiceServer};
	fn sample(now: Instant, kind: ConnectionKind) -> ConnectionSnapshot {
		ConnectionSnapshot {
			server: VoiceServer::new("rotterdam123.discord.media").map(std::sync::Arc::new),
			kind,
			started: now - Duration::from_secs(60),
			at: now,
			latency_generation: 0,
			udp_ping: Some(Latency {
				at: now,
				milliseconds: 32,
			}),
			websocket_ping: Some(Latency {
				at: now,
				milliseconds: 48,
			}),
			sent: PacketTotals::default(),
			received: PacketTotals::default(),
			send_errors: 0,
			send_bps: 64000,
			receive_bps: 128000,
			secure: true,
			reported_loss: None,
			media: MediaCounters::default(),
			video: None,
			received_video_codec: None,
			decoded_fps_milli: 0,
			participants: Vec::new(),
		}
	}
	#[test]
	fn diagnostics_are_isolated_by_call_and_source_and_ignore_retired_snapshots() {
		let now = Instant::now();
		let context = (1, Id(2), 3);
		let mut d = VoiceDiagnostics::default();
		d.update(context, sample(now, ConnectionKind::Call));
		d.update(context, sample(now, ConnectionKind::ScreenSend));
		d.update(context, sample(now, ConnectionKind::ScreenReceive));
		d.remove(ConnectionKind::ScreenSend);
		assert!(d.get(context, ConnectionKind::Call).is_some());
		assert!(d.get(context, ConnectionKind::ScreenSend).is_none());
		assert!(d.get(context, ConnectionKind::ScreenReceive).is_some());
		let old = sample(now - Duration::from_secs(10), ConnectionKind::Call);
		d.update(context, old);
		assert_eq!(d.get(context, ConnectionKind::Call).unwrap().at, now);
		let replacement = (4, Id(2), 3); // A different account generation, same channel/request.
		d.update(replacement, sample(now, ConnectionKind::Call));
		assert!(d.get(context, ConnectionKind::Call).is_none());
		assert!(d.get(replacement, ConnectionKind::ScreenReceive).is_none());
	}
	#[test]
	fn graph_is_bounded_deduplicates_samples_and_retires_on_resume() {
		let now = Instant::now();
		let mut history = History::default();
		let mut s = sample(now, ConnectionKind::Call);
		for index in 0..200 {
			s.at = now + Duration::from_secs(index);
			s.udp_ping = Some(Latency {
				at: s.at,
				milliseconds: index as u32,
			});
			history.observe(Some(&s), s.at);
			history.observe(Some(&s), s.at);
		}
		assert_eq!(history.samples().count(), HISTORY);
		assert_eq!(history.samples().next().unwrap().milliseconds, 140);
		s.latency_generation += 1;
		s.udp_ping = None;
		history.observe(Some(&s), s.at);
		assert_eq!(history.samples().count(), 0);
		history.observe(None, s.at);
		assert!(history.identity.is_none());
	}
	#[test]
	fn clipboard_is_bounded_and_omits_account_and_encryption_material() {
		let now = Instant::now();
		let context = (987654321, Id(998877665), 876543219);
		let mut d = VoiceDiagnostics::default();
		for kind in KINDS {
			d.update(context, sample(now, kind));
		}
		let copied = report(&d, context, now);
		assert!(copied.len() < 8192);
		assert!(copied.contains("udp_ping=32 ms") && copied.contains("rotterdam123.discord.media"));
		for id in ["987654321", "998877665", "876543219"] {
			assert!(!copied.contains(id));
		}
		let stale = report(&d, context, now + Duration::from_secs(20));
		assert!(stale.contains("current=false") && stale.contains("udp_ping=unavailable"));
		assert!(!stale.contains("udp_ping=32 ms"));
	}

	fn frame(
		ctx: &egui::Context,
		view: &mut MessagingUi,
		state: &mut State,
		events: Vec<egui::Event>,
		time: f64,
	) -> egui::FullOutput {
		let mut output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1120.0, 760.0),
				)),
				events,
				time: Some(time),
				..Default::default()
			},
			|ui| {
				assert!(
					!view
						.show(ui, state)
						.iter()
						.any(|c| matches!(c, client_core::Command::Voice(_)))
				);
			},
		);
		// These tests inspect CPU shapes; no renderer consumes texture uploads, even on panic.
		output.textures_delta.clear();
		output
	}
	fn find(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
		output
			.shapes
			.iter()
			.rev()
			.find_map(|shape| match &shape.shape {
				egui::Shape::Text(t) if t.galley.job.text == label => {
					Some(t.galley.rect.translate(t.pos.to_vec2()))
				}
				_ => None,
			})
	}
	fn click(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
		vec![
			egui::Event::PointerMoved(pos),
			egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed,
				modifiers: egui::Modifiers::NONE,
			},
		]
	}
	#[test]
	fn voice_card_opens_connection_details_and_debug_without_call_commands() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::community_state(true);
		let context = call_context(&state).unwrap();
		let mut view = MessagingUi {
			language: i18n::Language::English,
			..Default::default()
		};
		view.voice_privacy_code = Some("DO-NOT-COPY-THIS-CODE".into());
		view.voice_diagnostics
			.update(context, sample(Instant::now(), ConnectionKind::Call));
		let mut output = frame(&ctx, &mut view, &mut state, vec![], 0.0);
		output.drop_without_applying_deltas();
		output = frame(&ctx, &mut view, &mut state, vec![], 0.1);
		let title = find(&output, "Voice preview")
			.expect("visible account-card connection control")
			.center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(title, true), 0.2).drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(title, false), 0.3).drop_without_applying_deltas();
		output = frame(&ctx, &mut view, &mut state, vec![], 0.4);
		assert!(find(&output, "Voice Connection").is_some());
		assert!(find(&output, "32 ms").is_some());
		assert!(find(&output, "rotterdam123.discord.media").is_some());
		let debug = find(&output, "Voice Debug").unwrap().center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(debug, true), 0.5).drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(debug, false), 0.6).drop_without_applying_deltas();
		assert!(
			view.voice_connection_view.debug_open,
			"Debug action must open the detail window"
		);
		frame(&ctx, &mut view, &mut state, vec![], 0.65).drop_without_applying_deltas();
		output = frame(&ctx, &mut view, &mut state, vec![], 0.7);
		let copy = find(&output, "Copy diagnostics")
			.expect("debug window has copy action")
			.center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(copy, true), 0.8).drop_without_applying_deltas();
		output = frame(&ctx, &mut view, &mut state, click(copy, false), 0.9);
		assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(s) if s.contains("udp_ping=32 ms") && !s.contains("DO-NOT-COPY-THIS-CODE"))));
		output.drop_without_applying_deltas();
		state.voice.active = None;
		frame(&ctx, &mut view, &mut state, vec![], 1.0).drop_without_applying_deltas();
		assert!(!view.voice_connection_view.debug_open && view.voice_diagnostics.context.is_none());
	}

	#[test]
	fn voice_connection_control_supports_keyboard_and_hover_details() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::community_state(true);
		let mut view = MessagingUi {
			language: i18n::Language::English,
			..Default::default()
		};
		view.voice_diagnostics.update(
			call_context(&state).unwrap(),
			sample(Instant::now(), ConnectionKind::Call),
		);
		frame(&ctx, &mut view, &mut state, vec![], 0.0).drop_without_applying_deltas();
		let output = frame(&ctx, &mut view, &mut state, vec![], 0.1);
		let title = find(&output, "Voice preview").unwrap().center();
		output.drop_without_applying_deltas();
		frame(
			&ctx,
			&mut view,
			&mut state,
			vec![egui::Event::PointerMoved(title)],
			0.2,
		)
		.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, vec![], 1.0).drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, vec![], 1.1).drop_without_applying_deltas();
		let output = frame(&ctx, &mut view, &mut state, vec![], 1.2);
		assert!(find(&output, "32 ms").is_some());
		assert!(find(&output, "Rotterdam").is_some());
		output.drop_without_applying_deltas();
		let key = |key| {
			vec![egui::Event::Key {
				key,
				physical_key: Some(key),
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}]
		};
		let mut focused = false;
		for index in 0..100 {
			let output = frame(
				&ctx,
				&mut view,
				&mut state,
				key(egui::Key::Tab),
				2.0 + f64::from(index) / 10.0,
			);
			focused = output
				.platform_output
				.events
				.iter()
				.any(|event| event.widget_info().label.as_deref() == Some("Voice Connection"));
			output.drop_without_applying_deltas();
			if focused {
				break;
			}
		}
		assert!(focused, "connection control must be keyboard focusable");
		frame(&ctx, &mut view, &mut state, key(egui::Key::Enter), 13.0)
			.drop_without_applying_deltas();
		assert!(view.voice_connection_view.popup_open);
		frame(&ctx, &mut view, &mut state, key(egui::Key::Escape), 13.1)
			.drop_without_applying_deltas();
		assert!(!view.voice_connection_view.popup_open);
	}

	#[test]
	fn participant_selector_keeps_active_sources_reachable_in_large_rosters() {
		let mut state = test_support::community_state(true);
		let template = state.voice.roster[0].clone();
		state.voice.roster = (2..=256)
			.map(|user| {
				let mut entry = template.clone();
				entry.participant.user = Id(user);
				entry.member = None;
				entry
			})
			.collect();
		state.voice.active.as_mut().unwrap().participants = state
			.voice
			.roster
			.iter()
			.take(client_core::voice::MAX_PARTICIPANTS)
			.map(|entry| entry.participant)
			.collect();
		let mut snapshot = sample(Instant::now(), ConnectionKind::Call);
		snapshot
			.participants
			.push(preview_participant(256, snapshot.at));
		let options = participant_options(&state, Some(&snapshot));
		assert!(
			options.contains(&Id(256)),
			"an active source must not be displaced by idle listeners"
		);
		assert!(options.len() <= client_core::voice::MAX_PARTICIPANTS + 16);
		assert!(!options.contains(&state.user.as_ref().unwrap().id));
		assert!(options.windows(2).all(|pair| pair[0] < pair[1]));
	}

	#[test]
	fn participant_selector_shows_attributed_media_and_retires_removed_people() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::call_demo_state();
		let context = call_context(&state).unwrap();
		let mut view = MessagingUi {
			language: i18n::Language::English,
			..Default::default()
		};
		let mut snapshot = sample(Instant::now(), ConnectionKind::Call);
		snapshot
			.participants
			.push(preview_participant(2, snapshot.at));
		view.voice_diagnostics.update(context, snapshot.clone());
		frame(&ctx, &mut view, &mut state, vec![], 0.0).drop_without_applying_deltas();
		view.voice_connection_view.debug_open = true;
		frame(&ctx, &mut view, &mut state, vec![], 0.1).drop_without_applying_deltas();
		let output = frame(&ctx, &mut view, &mut state, vec![], 0.2);
		let combo = find(&output, "Connection").unwrap().center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(combo, true), 0.3).drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(combo, false), 0.4).drop_without_applying_deltas();
		let output = frame(&ctx, &mut view, &mut state, vec![], 0.5);
		let label = participant_name(&state, context.1, Id(2));
		let option = find(&output, &label)
			.expect("participant reachable in selector")
			.center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(option, true), 0.6).drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(option, false), 0.7)
			.drop_without_applying_deltas();
		assert_eq!(view.voice_connection_view.participant, Some(Id(2)));
		let output = frame(&ctx, &mut view, &mut state, vec![], 0.8);
		assert!(find(&output, "Incoming audio").is_some());
		assert!(find(&output, "1.25 ms").is_some());
		let copy = find(&output, "Copy diagnostics").unwrap().center();
		output.drop_without_applying_deltas();
		frame(&ctx, &mut view, &mut state, click(copy, true), 0.9).drop_without_applying_deltas();
		let output = frame(&ctx, &mut view, &mut state, click(copy, false), 1.0);
		assert!(output.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(s) if s.contains("Selected participant") && s.contains("audio_jitter=1.25 ms") && !s.contains(&label))));
		output.drop_without_applying_deltas();
		// The selected participant can have no forwarded media without borrowing another row.
		view.voice_diagnostics
			.update(context, sample(Instant::now(), ConnectionKind::Call));
		let output = frame(&ctx, &mut view, &mut state, vec![], 1.1);
		assert!(
			find(
				&output,
				"No incoming media measurements for this participant yet."
			)
			.is_some()
		);
		output.drop_without_applying_deltas();
		state
			.voice
			.active
			.as_mut()
			.unwrap()
			.participants
			.retain(|p| p.user != Id(2));
		state.voice.roster.retain(|p| p.participant.user != Id(2));
		frame(&ctx, &mut view, &mut state, vec![], 1.2).drop_without_applying_deltas();
		assert!(view.voice_connection_view.participant.is_none());
		let mut peer = preview_participant(887766554433, Instant::now());
		let copied = participant_report(&peer, Instant::now(), true);
		assert!(copied.len() < 2048 && !copied.contains("887766554433"));
		peer.audio.as_mut().unwrap().packets.last_received =
			Some(Instant::now() - Duration::from_secs(30));
		assert!(
			participant_report(&peer, Instant::now(), true).contains("audio_jitter=unavailable")
		);
	}

	#[test]
	fn debug_defers_escape_to_settings_and_releases_data_without_rendering_a_window() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::community_state(true);
		let mut view = MessagingUi {
			language: i18n::Language::English,
			..Default::default()
		};
		frame(&ctx, &mut view, &mut state, vec![], 0.0).drop_without_applying_deltas();
		view.voice_connection_view.debug_open = true;
		view.settings.open = true;
		frame(&ctx, &mut view, &mut state, vec![], 0.1).drop_without_applying_deltas();
		frame(
			&ctx,
			&mut view,
			&mut state,
			vec![egui::Event::Key {
				key: egui::Key::Escape,
				physical_key: Some(egui::Key::Escape),
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}],
			0.2,
		)
		.drop_without_applying_deltas();
		assert!(
			!view.settings.open,
			"the settings page owns Escape while debug is hidden"
		);
		assert!(view.voice_connection_view.debug_open);
		state.voice.active = None;
		view.sync_voice_connection(&state);
		assert!(!view.voice_connection_view.debug_open && view.voice_diagnostics.context.is_none());
	}
}
fn text(key: &str) -> String {
	i18n::translate(key)
}

/// The desktop associates each latest-value snapshot with the owning account/call request.
#[derive(Default)]
pub struct VoiceDiagnostics {
	context: Option<Context>,
	sources: [Option<std::sync::Arc<ConnectionSnapshot>>; 3],
}
impl VoiceDiagnostics {
	pub fn update(
		&mut self,
		context: Context,
		snapshot: impl Into<std::sync::Arc<ConnectionSnapshot>>,
	) {
		let snapshot = snapshot.into();
		if self.context != Some(context) {
			self.clear();
			self.context = Some(context);
		}
		let index = slot(snapshot.kind);
		if self.sources[index].as_ref().is_none_or(|old| {
			snapshot.started > old.started
				|| (snapshot.started == old.started && snapshot.at >= old.at)
		}) {
			self.sources[index] = Some(snapshot);
		}
	}
	pub fn remove(&mut self, kind: ConnectionKind) {
		self.sources[slot(kind)] = None;
	}
	pub fn clear(&mut self) {
		self.context = None;
		self.sources = Default::default();
	}
	pub fn get(&self, context: Context, kind: ConnectionKind) -> Option<&ConnectionSnapshot> {
		(self.context == Some(context))
			.then(|| self.sources[slot(kind)].as_deref())
			.flatten()
	}
}

struct History {
	identity: Option<(Instant, u64)>,
	samples: [Option<Latency>; HISTORY],
	next: usize,
	last: Option<Instant>,
}
impl Default for History {
	fn default() -> Self {
		Self {
			identity: None,
			samples: [None; HISTORY],
			next: 0,
			last: None,
		}
	}
}
impl History {
	fn observe(&mut self, snapshot: Option<&ConnectionSnapshot>, now: Instant) {
		let Some(snapshot) = snapshot else {
			*self = Self::default();
			return;
		};
		let identity = (snapshot.started, snapshot.latency_generation);
		if self.identity != Some(identity) {
			*self = Self::default();
			self.identity = Some(identity);
		}
		if snapshot.fresh(now)
			&& let Some(sample) = snapshot.udp_ping
			&& sample.fresh(now).is_some()
			&& self.last != Some(sample.at)
		{
			self.samples[self.next] = Some(sample);
			self.next = (self.next + 1) % HISTORY;
			self.last = Some(sample.at);
		}
	}
	fn samples(&self) -> impl Iterator<Item = Latency> + '_ {
		(0..HISTORY).filter_map(|offset| self.samples[(self.next + offset) % HISTORY])
	}
}

#[derive(Default)]
pub(super) struct ConnectionView {
	context: Option<Context>,
	popup_open: bool,
	debug_open: bool,
	selected: ConnectionKind,
	participant: Option<Id>,
	histories: [History; 3],
	copied: Option<Instant>,
}

fn call_context(state: &State) -> Option<Context> {
	state
		.voice
		.active
		.as_ref()
		.map(|call| (state.generation, call.channel, call.request))
}
fn current(snapshot: Option<&ConnectionSnapshot>, now: Instant) -> Option<&ConnectionSnapshot> {
	snapshot.filter(|snapshot| snapshot.fresh(now))
}
fn ping(snapshot: Option<&ConnectionSnapshot>, websocket: bool, now: Instant) -> String {
	let latency = current(snapshot, now).and_then(|s| {
		if websocket {
			s.websocket_ping
		} else {
			s.udp_ping
		}
	});
	latency.and_then(|sample| sample.fresh(now)).map_or_else(
		|| text("voice-connection-unavailable"),
		|ping| format!("{ping} ms"),
	)
}
fn row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
	ui.label(egui::RichText::new(label).color(design::palette(ui).muted));
	ui.add(egui::Label::new(value.into()).wrap().selectable(true));
	ui.end_row();
}
fn overview(ui: &mut egui::Ui, snapshot: Option<&ConnectionSnapshot>, now: Instant) {
	let server = snapshot.and_then(|s| s.server.as_ref());
	egui::Grid::new(ui.scope_id().with("connection-details"))
		.num_columns(2)
		.spacing([12.0, 6.0])
		.show(ui, |ui| {
			row(
				ui,
				&text("voice-connection-ping"),
				ping(snapshot, false, now),
			);
			row(
				ui,
				&text("voice-connection-server"),
				server.map_or_else(
					|| text("voice-connection-unavailable"),
					|s| s.host().to_owned(),
				),
			);
			row(
				ui,
				&text("voice-connection-region"),
				server
					.and_then(|s| s.inferred_region())
					.map_or_else(|| text("voice-connection-unavailable"), str::to_owned),
			);
		});
}
fn graph(ui: &mut egui::Ui, history: &History) {
	let colors = design::palette(ui);
	let (rect, _) =
		ui.allocate_exact_size(egui::vec2(ui.available_width(), 72.0), egui::Sense::hover());
	ui.painter().rect_filled(rect, 6, colors.base);
	let samples: Vec<_> = history.samples().collect();
	if samples.is_empty() {
		ui.painter().text(
			rect.center(),
			egui::Align2::CENTER_CENTER,
			text("voice-connection-measuring"),
			egui::FontId::proportional(12.0),
			colors.muted,
		);
		return;
	}
	let inner = rect.shrink(8.0);
	let maximum = samples
		.iter()
		.map(|s| s.milliseconds)
		.max()
		.unwrap_or(1)
		.max(50) as f32;
	let point = |index: usize, sample: &Latency| {
		egui::pos2(
			inner.left()
				+ inner.width() * index as f32 / samples.len().saturating_sub(1).max(1) as f32,
			inner.bottom() - inner.height() * sample.milliseconds as f32 / maximum,
		)
	};
	for index in 1..samples.len() {
		ui.painter().line_segment(
			[
				point(index - 1, &samples[index - 1]),
				point(index, &samples[index]),
			],
			egui::Stroke::new(2.0, colors.positive),
		);
	}
	if samples.len() == 1 {
		ui.painter()
			.circle_filled(point(0, &samples[0]), 2.5, colors.positive);
	}
}

fn participant_name(state: &State, channel: Id, user: Id) -> String {
	state
		.voice
		.roster
		.iter()
		.find(|entry| entry.channel == channel && entry.participant.user == user)
		.map(|entry| crate::voice::resolve_member(state, entry).1)
		.or_else(|| {
			crate::voice::participant_user(state, channel, user).map(|user| user.name.as_str())
		})
		.unwrap_or("Participant")
		.to_owned()
}
fn participant_options(state: &State, snapshot: Option<&ConnectionSnapshot>) -> Vec<Id> {
	let Some(call) = state.voice.active.as_ref() else {
		return Vec::new();
	};
	const LIMIT: usize = client_core::voice::MAX_PARTICIPANTS + 16;
	// Forwarded sources must remain reachable even in a Stage with many idle listeners.
	let mut users: Vec<_> = snapshot
		.into_iter()
		.flat_map(|s| &s.participants)
		.map(|p| Id(p.user))
		.take(LIMIT)
		.collect();
	for id in call
		.participants
		.iter()
		.map(|p| p.user)
		.chain(
			state
				.voice
				.roster
				.iter()
				.filter(|entry| entry.channel == call.channel)
				.map(|entry| entry.participant.user),
		)
		.take(client_core::voice::MAX_PARTICIPANTS * 2)
	{
		if users.len() >= LIMIT {
			break;
		}
		if state.user.as_ref().is_none_or(|own| own.id != id) && !users.contains(&id) {
			users.push(id);
		}
	}
	users.retain(|id| state.user.as_ref().is_none_or(|own| own.id != *id));
	users.sort_unstable();
	users.dedup();
	users
}

fn incoming_rows(ui: &mut egui::Ui, packets: IncomingPackets, fresh: bool) {
	row(
		ui,
		&text("voice-connection-peer-packets"),
		format!(
			"{} / {} B",
			packets.received.packets, packets.received.bytes
		),
	);
	row(
		ui,
		&text("voice-connection-peer-rate"),
		if fresh {
			format!("{:.1} kbit/s", packets.bitrate as f64 / 1000.0)
		} else {
			text("voice-connection-unavailable")
		},
	);
	row(
		ui,
		&text("voice-connection-peer-loss"),
		if packets.finalized != 0 {
			format!(
				"{:.2}% ({} / {})",
				packets.missing as f64 * 100.0 / packets.finalized as f64,
				packets.missing,
				packets.finalized
			)
		} else {
			text("voice-connection-unavailable")
		},
	);
	row(
		ui,
		&text("voice-connection-peer-duplicates"),
		packets.duplicates.to_string(),
	);
}
fn participant_details(
	ui: &mut egui::Ui,
	peer: Option<&ParticipantStatistics>,
	now: Instant,
	fresh: bool,
) {
	ui.small(text("voice-connection-peer-shared-route"));
	ui.small(text("voice-connection-peer-loss-help"));
	let Some(peer) = peer else {
		ui.label(text("voice-connection-peer-no-data"));
		return;
	};
	ui.separator();
	ui.strong(text("voice-connection-peer-audio"));
	if let Some(audio) = peer.audio {
		egui::Grid::new("voice-debug-peer-audio")
			.num_columns(2)
			.spacing([16.0, 6.0])
			.show(ui, |ui| {
				row(ui, &text("voice-connection-audio-codec"), "Opus · 48 kHz");
				incoming_rows(ui, audio.packets, fresh);
				row(
					ui,
					&text("voice-connection-peer-jitter"),
					audio
						.jitter_ms
						.filter(|_| {
							fresh
								&& audio.packets.last_received.is_some_and(|at| {
									now.saturating_duration_since(at)
										< model::voice_diagnostics::SAMPLE_LIFETIME
								})
						})
						.map_or_else(
							|| text("voice-connection-unavailable"),
							|v| format!("{v:.2} ms"),
						),
				);
				row(
					ui,
					&text("voice-connection-peer-audio-decoded"),
					audio.decoded.to_string(),
				);
				row(
					ui,
					&text("voice-connection-peer-concealed"),
					audio.concealed.to_string(),
				);
				row(
					ui,
					&text("voice-connection-decoder-errors"),
					audio.decoder_errors.to_string(),
				);
			});
		ui.small(text("voice-connection-peer-concealed-help"));
	} else {
		ui.label(text("voice-connection-peer-no-audio"));
	}
	ui.separator();
	ui.strong(text("voice-connection-peer-video"));
	if let Some(video) = peer.video {
		egui::Grid::new("voice-debug-peer-video")
			.num_columns(2)
			.spacing([16.0, 6.0])
			.show(ui, |ui| {
				row(
					ui,
					&text("voice-connection-received-codec"),
					video.codec.map_or_else(
						|| text("voice-connection-unavailable"),
						|v| crate::voice::video_codec_name(v).into(),
					),
				);
				incoming_rows(ui, video.packets, fresh);
				row(
					ui,
					&text("voice-connection-peer-decoded-size"),
					video.dimensions.map_or_else(
						|| text("voice-connection-unavailable"),
						|(w, h)| format!("{w} × {h}"),
					),
				);
				row(
					ui,
					&text("voice-connection-decoded-fps"),
					if fresh {
						format!("{:.1}", video.decoded_fps_milli as f64 / 1000.0)
					} else {
						text("voice-connection-unavailable")
					},
				);
				row(
					ui,
					&text("voice-connection-peer-awaiting"),
					if fresh {
						text(if video.awaiting_keyframe {
							"voice-connection-yes"
						} else {
							"voice-connection-no"
						})
					} else {
						text("voice-connection-unavailable")
					},
				);
				for (key, value) in [
					("voice-connection-complete", video.complete),
					("voice-connection-incomplete", video.incomplete),
					("voice-connection-pictures", video.decoded),
					("voice-connection-decoder-errors", video.decoder_errors),
					("voice-connection-queue-drops", video.queue_drops),
					("voice-connection-stale", video.stale),
				] {
					row(ui, &text(key), value.to_string());
				}
			});
	} else {
		ui.label(text("voice-connection-peer-no-video"));
	}
}
/// Only the selected participant's numeric data is copied. No ID/name or roster.
fn participant_report(peer: &ParticipantStatistics, now: Instant, fresh: bool) -> String {
	let mut output = format!("\nSelected participant receive statistics: current={fresh}\n");
	let mut packets = |kind: &str, p: IncomingPackets| {
		let _ = writeln!(
			output,
			"{kind}: packets={} bytes={} bitrate={} finalized_packets={} missing_packets={} duplicates={}",
			p.received.packets,
			p.received.bytes,
			if fresh {
				p.bitrate.to_string()
			} else {
				"unavailable".into()
			},
			p.finalized,
			p.missing,
			p.duplicates
		);
	};
	if let Some(audio) = peer.audio {
		packets("audio", audio.packets);
	}
	if let Some(video) = peer.video {
		packets("video", video.packets);
	}
	if let Some(audio) = peer.audio {
		let jitter = audio
			.jitter_ms
			.filter(|_| {
				fresh
					&& audio.packets.last_received.is_some_and(|at| {
						now.saturating_duration_since(at)
							< model::voice_diagnostics::SAMPLE_LIFETIME
					})
			})
			.map_or_else(|| "unavailable".into(), |v| format!("{v:.2} ms"));
		let _ = writeln!(
			output,
			"audio_jitter={jitter} decoded_packets={} concealment_requests={} decoder_errors={}",
			audio.decoded, audio.concealed, audio.decoder_errors
		);
	}
	if let Some(video) = peer.video {
		let _ = writeln!(
			output,
			"video_codec={:?} dimensions={:?} reassembled={} incomplete={} queue_drops={} decoded={} decoder_errors={} expired={} decoded_fps_milli={}",
			video.codec,
			video.dimensions,
			video.complete,
			video.incomplete,
			video.queue_drops,
			video.decoded,
			video.decoder_errors,
			video.stale,
			if fresh {
				video.decoded_fps_milli.to_string()
			} else {
				"unavailable".into()
			}
		);
	}
	output
}

#[cfg(any(test, feature = "demo"))]
fn preview_participant(user: u64, now: Instant) -> ParticipantStatistics {
	use model::voice_diagnostics::{AudioStatistics, PacketTotals, VideoStatistics};
	let started = now - Duration::from_secs(75);
	ParticipantStatistics {
		user,
		audio: Some(AudioStatistics {
			started,
			packets: IncomingPackets {
				last_received: Some(now),
				received: PacketTotals {
					packets: 3600,
					bytes: 580000,
				},
				bitrate: 64000,
				finalized: 3536,
				missing: 2,
				duplicates: 1,
			},
			jitter_ms: Some(1.25),
			decoded: 3598,
			concealed: 3,
			decoder_errors: 0,
		}),
		video: Some(VideoStatistics {
			started,
			packets: IncomingPackets {
				last_received: Some(now),
				received: PacketTotals {
					packets: 20000,
					bytes: 18000000,
				},
				bitrate: 2_000_000,
				finalized: 19936,
				missing: 4,
				duplicates: 2,
			},
			codec: Some(model::voice_settings::VideoCodec::H264),
			complete: 2100,
			incomplete: 1,
			queue_drops: 0,
			awaiting_keyframe: false,
			decoded: 2100,
			decoder_errors: 0,
			stale: 0,
			dimensions: Some((1920, 1080)),
			decoded_fps_milli: 30000,
		}),
	}
}

fn report(diagnostics: &VoiceDiagnostics, context: Context, now: Instant) -> String {
	let mut output = String::from(
		"NuSerein voice diagnostics (local snapshot; no credentials or participant identifiers)\n",
	);
	for kind in KINDS {
		let Some(s) = diagnostics.get(context, kind) else {
			continue;
		};
		let _ = writeln!(
			output,
			"\n{kind:?}: current={} secure={}",
			s.fresh(now),
			s.secure
		);
		let _ = writeln!(
			output,
			"server={} region={} (hostname inference)",
			s.server.as_ref().map_or("unavailable", |s| s.host()),
			s.server
				.as_ref()
				.and_then(|s| s.inferred_region())
				.unwrap_or("unavailable")
		);
		let latency = |value: Option<Latency>| {
			value
				.and_then(|v| v.fresh(now))
				.filter(|_| s.fresh(now))
				.map_or_else(|| "unavailable".to_owned(), |v| format!("{v} ms"))
		};
		let _ = writeln!(
			output,
			"udp_ping={} websocket_ping={}",
			latency(s.udp_ping),
			latency(s.websocket_ping)
		);
		let _ = writeln!(
			output,
			"udp_sent_packets={} udp_sent_bytes={} udp_received_packets={} udp_received_bytes={} send_errors={}",
			s.sent.packets, s.sent.bytes, s.received.packets, s.received.bytes, s.send_errors
		);
		let _ = writeln!(
			output,
			"udp_send_bps={} udp_receive_bps={} reported_video_loss_percent={}",
			s.send_bps,
			s.receive_bps,
			s.loss_percent(now)
				.filter(|_| s.fresh(now))
				.map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.2}"))
		);
		let _ = writeln!(output, "audio_codec=Opus audio_sample_rate=48000");
		if let Some(video) = s.video {
			let _ = writeln!(
				output,
				"outgoing_video={:?} {}x{} configured_fps={} target_bitrate={}",
				video.codec, video.width, video.height, video.fps, video.target_bitrate
			);
		}
		let _ = writeln!(
			output,
			"received_video_codec={:?} decoded_fps_milli={} media={:?}",
			s.received_video_codec, s.decoded_fps_milli, s.media
		);
	}
	output
}

impl MessagingUi {
	pub(super) fn voice_connection_trigger(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		rect: egui::Rect,
	) {
		let Some(context) = call_context(state) else {
			return;
		};
		let response = ui.interact(
			rect,
			ui.make_persistent_id("voice-connection-trigger"),
			egui::Sense::click(),
		);
		response.widget_info(|| {
			egui::WidgetInfo::labeled(egui::Role::Button, true, text("voice-connection-title"))
		});
		if response.has_focus() {
			ui.painter().rect_stroke(
				rect.expand(2.0),
				6,
				ui.visuals().selection.stroke,
				egui::StrokeKind::Inside,
			);
		}
		let snapshot = self.voice_diagnostics.get(context, ConnectionKind::Call);
		let now = Instant::now();
		let response = response
			.on_hover_cursor(egui::CursorIcon::PointingHand)
			.on_hover_ui(|ui| {
				ui.set_max_width(300.0);
				overview(ui, snapshot, now);
			});
		if response.clicked() {
			self.voice_connection_view.popup_open = !self.voice_connection_view.popup_open;
		}
		let mut open = self.voice_connection_view.popup_open;
		let mut debug = false;
		egui::Popup::menu(&response)
			.open_bool(&mut open)
			.width(320.0)
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.show(|ui| {
				ui.label(design::semibold(ui, text("voice-connection-title"), 17.0));
				if state.demo {
					ui.small(text("voice-connection-synthetic"));
				}
				overview(ui, snapshot, now);
				graph(ui, &self.voice_connection_view.histories[0]);
				debug = ui.button(text("voice-connection-debug")).clicked();
			});
		self.voice_connection_view.popup_open = open && !debug;
		if debug {
			self.voice_connection_view.debug_open = true;
			ui.ctx().request_repaint();
		}
	}

	pub(super) fn sync_voice_connection(&mut self, state: &State) {
		let context = call_context(state);
		if self.voice_connection_view.context != context {
			self.voice_connection_view = ConnectionView {
				context,
				..Default::default()
			};
		}
		let Some(context) = context else {
			self.voice_diagnostics.clear();
			return;
		};
		if self.voice_diagnostics.context != Some(context) {
			self.voice_diagnostics.clear();
		}
		let now = Instant::now();
		for kind in KINDS {
			self.voice_connection_view.histories[slot(kind)]
				.observe(self.voice_diagnostics.get(context, kind), now);
		}
	}

	pub(super) fn show_voice_connection_debug(&mut self, ctx: &egui::Context, state: &State) {
		let Some(context) = call_context(state) else {
			return;
		};
		let now = Instant::now();
		if self.settings.open || ctx.memory(|memory| memory.top_modal_layer().is_some()) {
			return;
		}
		if !self.voice_connection_view.debug_open {
			return;
		}
		if self.voice_connection_view.selected != ConnectionKind::Call
			&& self
				.voice_diagnostics
				.get(context, self.voice_connection_view.selected)
				.is_none()
		{
			self.voice_connection_view.selected = ConnectionKind::Call;
		}
		let mut open = true;
		let width = (ctx.content_rect().width() - 32.0).clamp(280.0, 560.0);
		let height = (ctx.content_rect().height() - 64.0).clamp(280.0, 680.0);
		egui::Window::new(text("voice-connection-debug"))
			.id(egui::Id::unique("voice-connection-debug"))
			.open(&mut open)
			.default_width(width)
			.default_height(height)
			.max_width(width)
			.max_height((ctx.content_rect().height() - 24.0).max(280.0))
			.resizable(true)
			.collapsible(false)
			.show(ctx, |ui| {
				if state.demo {
					ui.label(text("voice-connection-synthetic"));
				}
				if let Some(call) = state.voice.active.as_ref() {
					ui.label(format!(
						"{}: {}",
						text("voice-connection-call-state"),
						call.phase.label()
					));
					if let Some(error) = call.error {
						ui.colored_label(design::palette(ui).warning, error);
					}
				}
				ui.horizontal_wrapped(|ui| {
					for (kind, key) in KINDS.into_iter().zip([
						"voice-connection-call",
						"voice-connection-sending",
						"voice-connection-watching",
					]) {
						let available = kind == ConnectionKind::Call
							|| self.voice_diagnostics.get(context, kind).is_some();
						if ui
							.add_enabled(
								available,
								egui::Button::selectable(
									self.voice_connection_view.selected == kind,
									text(key),
								),
							)
							.clicked()
						{
							self.voice_connection_view.selected = kind;
						}
					}
				});
				let kind = self.voice_connection_view.selected;
				let snapshot = self.voice_diagnostics.get(context, kind);
				let options = participant_options(state, snapshot);
				if self
					.voice_connection_view
					.participant
					.is_some_and(|id| !options.contains(&id))
				{
					self.voice_connection_view.participant = None;
				}
				ui.horizontal(|ui| {
					ui.label(text("voice-connection-statistics"));
					egui::ComboBox::from_id_salt("voice-debug-participant")
						.selected_text(self.voice_connection_view.participant.map_or_else(
							|| text("voice-connection-whole-connection"),
							|id| participant_name(state, context.1, id),
						))
						.width(ui.available_width().min(300.0))
						.show_ui(ui, |ui| {
							ui.selectable_value(
								&mut self.voice_connection_view.participant,
								None,
								text("voice-connection-whole-connection"),
							);
							for id in options {
								ui.selectable_value(
									&mut self.voice_connection_view.participant,
									Some(id),
									participant_name(state, context.1, id),
								);
							}
						});
				});
				egui::ScrollArea::vertical()
					.max_height((ui.available_height() - 64.0).max(96.0))
					.show(ui, |ui| {
						if let Some(id) = self.voice_connection_view.participant {
							let peer = snapshot
								.and_then(|s| s.participants.iter().find(|peer| peer.user == id.0));
							participant_details(
								ui,
								peer,
								now,
								snapshot.is_some_and(|s| s.fresh(now)),
							);
							return;
						}
						overview(ui, snapshot, now);
						graph(ui, &self.voice_connection_view.histories[slot(kind)]);
						ui.small(text("voice-connection-history"));
						if snapshot.is_none_or(|s| !s.fresh(now)) {
							ui.colored_label(
								design::palette(ui).warning,
								text("voice-connection-no-current-data"),
							);
						}
						if snapshot
							.and_then(|s| s.server.as_ref())
							.and_then(|s| s.inferred_region())
							.is_some()
						{
							ui.small(text("voice-connection-inferred-region"));
						}
						if let Some(s) = snapshot {
							ui.separator();
							egui::Grid::new("voice-debug-values")
								.num_columns(2)
								.spacing([16.0, 6.0])
								.show(ui, |ui| {
									row(
										ui,
										&text("voice-connection-media-ready"),
										text(if !s.fresh(now) {
											"voice-connection-unavailable"
										} else if s.secure {
											"voice-connection-yes"
										} else {
											"voice-connection-no"
										}),
									);
									row(
										ui,
										&text("voice-connection-websocket-ping"),
										ping(Some(s), true, now),
									);
									row(
										ui,
										&text("voice-connection-session-duration"),
										format!(
											"{} s",
											now.saturating_duration_since(s.started).as_secs()
										),
									);
									row(
										ui,
										&text("voice-connection-udp-sent"),
										format!("{} / {} B", s.sent.packets, s.sent.bytes),
									);
									row(
										ui,
										&text("voice-connection-udp-received"),
										format!("{} / {} B", s.received.packets, s.received.bytes),
									);
									row(
										ui,
										&text("voice-connection-send-rate"),
										if s.fresh(now) {
											format!("{:.1} kbit/s", s.send_bps as f64 / 1000.0)
										} else {
											text("voice-connection-unavailable")
										},
									);
									row(
										ui,
										&text("voice-connection-receive-rate"),
										if s.fresh(now) {
											format!("{:.1} kbit/s", s.receive_bps as f64 / 1000.0)
										} else {
											text("voice-connection-unavailable")
										},
									);
									row(
										ui,
										&text("voice-connection-send-errors"),
										s.send_errors.to_string(),
									);
									row(
										ui,
										&text("voice-connection-loss"),
										s.loss_percent(now).filter(|_| s.fresh(now)).map_or_else(
											|| text("voice-connection-unavailable"),
											|v| format!("{v:.1}%"),
										),
									);
									row(ui, &text("voice-connection-audio-codec"), "Opus · 48 kHz");
									if let Some(v) = s.video {
										row(
											ui,
											&text("voice-connection-video-codec"),
											crate::voice::video_codec_name(v.codec),
										);
										row(
											ui,
											&text("voice-connection-video-size"),
											format!("{} × {}", v.width, v.height),
										);
										row(
											ui,
											&text("voice-connection-configured-fps"),
											v.fps.to_string(),
										);
										row(
											ui,
											&text("voice-connection-video-target"),
											format!(
												"{:.1} Mbit/s",
												f64::from(v.target_bitrate) / 1_000_000.0
											),
										);
									}
									if let Some(codec) = s.received_video_codec {
										row(
											ui,
											&text("voice-connection-received-codec"),
											crate::voice::video_codec_name(codec),
										);
									}
									row(
										ui,
										&text("voice-connection-decoded-fps"),
										if s.fresh(now) {
											format!("{:.1}", s.decoded_fps_milli as f64 / 1000.0)
										} else {
											text("voice-connection-unavailable")
										},
									);
									for (key, value) in [
										("voice-connection-video-packets", s.media.video_packets),
										("voice-connection-not-ready", s.media.video_not_ready),
										(
											"voice-connection-unknown-source",
											s.media.unknown_video_sources,
										),
										(
											"voice-connection-complete",
											s.media.complete_video_frames,
										),
										("voice-connection-gated", s.media.gated_video_frames),
										("voice-connection-keyframes", s.media.received_keyframes),
										(
											"voice-connection-missing-parameters",
											s.media.keyframes_without_parameters,
										),
										(
											"voice-connection-picture-gap",
											s.media.max_picture_gap_ms,
										),
										(
											"voice-connection-queue-delay",
											s.media.max_decode_queue_ms,
										),
										("voice-connection-pictures", s.media.decoded_pictures),
										("voice-connection-rtx", s.media.rtx_packets),
										("voice-connection-pli", s.media.pli_sent),
										("voice-connection-incomplete", s.media.incomplete_frames),
										("voice-connection-queue-drops", s.media.queue_drops),
										("voice-connection-stale", s.media.stale_frames),
										("voice-connection-decoder-errors", s.media.decoder_errors),
										("voice-connection-rejected", s.media.rejected_frames),
										(
											"voice-connection-auth-errors",
											s.media.authentication_failures,
										),
										(
											"voice-connection-decrypt-errors",
											s.media.decryption_failures,
										),
										("voice-connection-local-drops", s.media.local_media_drops),
										("voice-connection-stalls", s.media.media_stalls),
									] {
										row(ui, &text(key), value.to_string());
									}
								});
						}
					});
				ui.separator();
				if ui.button(text("voice-connection-copy")).clicked() {
					let mut copied = report(&self.voice_diagnostics, context, now);
					if let Some(peer) = self.voice_connection_view.participant.and_then(|id| {
						snapshot.and_then(|s| s.participants.iter().find(|p| p.user == id.0))
					}) {
						copied.push_str(&participant_report(
							peer,
							now,
							snapshot.is_some_and(|s| s.fresh(now)),
						));
					}
					if let Some(error) = state.voice.active.as_ref().and_then(|call| call.error) {
						let _ = writeln!(copied, "call_error={error}");
					}
					ctx.copy_text(copied);
					self.voice_connection_view.copied = Some(now);
				}
				if self
					.voice_connection_view
					.copied
					.is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(2))
				{
					ui.small(text("voice-connection-copied"));
					ctx.request_repaint_after(Duration::from_secs(2));
				}
			});
		if !egui::Popup::is_any_open(ctx)
			&& ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
		{
			open = false;
		}
		self.voice_connection_view.debug_open = open;
		ctx.request_repaint_after(Duration::from_secs(1));
	}

	/// Explicit offline preview data; never read from a real transport or account.
	#[cfg(feature = "demo")]
	pub fn preview_voice_connection(&mut self, state: &State, debug: bool) {
		use model::voice_diagnostics::{
			MediaCounters, PacketTotals, VideoConfiguration, VoiceServer,
		};
		let Some(context) = call_context(state) else {
			return;
		};
		let now = Instant::now();
		let started = now - Duration::from_secs(75);
		let snapshot = ConnectionSnapshot {
			server: VoiceServer::new("rotterdam123.discord.media").map(std::sync::Arc::new),
			kind: ConnectionKind::Call,
			started,
			at: now,
			latency_generation: 0,
			udp_ping: Some(Latency {
				at: now,
				milliseconds: 32,
			}),
			websocket_ping: Some(Latency {
				at: now,
				milliseconds: 45,
			}),
			sent: PacketTotals {
				packets: 3750,
				bytes: 775000,
			},
			received: PacketTotals {
				packets: 7400,
				bytes: 3300000,
			},
			send_errors: 0,
			send_bps: 80000,
			receive_bps: 460000,
			secure: true,
			reported_loss: Some((1, now)),
			media: MediaCounters {
				decoded_pictures: 720,
				rtx_packets: 3,
				pli_sent: 1,
				..Default::default()
			},
			video: Some(VideoConfiguration {
				codec: model::voice_settings::VideoCodec::H264,
				width: 1920,
				height: 1080,
				fps: 30,
				target_bitrate: 4_000_000,
			}),
			received_video_codec: Some(model::voice_settings::VideoCodec::H264),
			decoded_fps_milli: 30000,
			participants: vec![preview_participant(2, now)],
		};
		self.voice_diagnostics.update(context, snapshot.clone());
		self.voice_connection_view = ConnectionView {
			context: Some(context),
			debug_open: debug,
			participant: debug.then_some(Id(2)),
			popup_open: !debug,
			..Default::default()
		};
		for (offset, ping) in [35, 31, 34, 29, 32].into_iter().enumerate() {
			let mut s = snapshot.clone();
			s.udp_ping = Some(Latency {
				at: now - Duration::from_secs(10 - offset as u64 * 2),
				milliseconds: ping,
			});
			self.voice_connection_view.histories[0].observe(Some(&s), now);
		}
	}
}
