//! Native event browsing/editing and explicit Stage controls. All drafts stay session-only.
use crate::extensions_ui::TextInputStates;
use client_core::{Command, State};
use model::{Id, community::*};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy)]
enum SavedDraft {
	Event,
	Occurrence,
	Confirmation,
}

enum DraftReplacement {
	Event(Box<Editor>),
	Occurrence(Box<OccurrenceEditor>),
}

#[derive(Default)]
pub(super) struct CommunityUi {
	guild: Option<Id>,
	generation: u64,
	selected: Option<Id>,
	draft: Option<Editor>,
	browsing_draft: bool,
	browsing_occurrence: bool,
	occurrence_draft: Option<OccurrenceEditor>,
	replacement: Option<DraftReplacement>,
	confirm: Option<Action>,
	submitted: Option<(u64, SavedDraft)>,
	link: String,
	link_error: Option<&'static str>,
	pub stage_channel: Option<Id>,
	stage_topic: String,
	stage_event: Option<Id>,
	stage_draft_channel: Option<Id>,
	stage_dirty: bool,
	stage_draft_conflict: bool,
	stage_submitted: Option<u64>,
	requested_stage: Option<Id>,
	audience_page: usize,
	end_stage_confirm: bool,
	inputs: Option<TextInputStates>,
}
struct Editor {
	inputs: TextInputStates,
	guild: Id,
	before: Option<ScheduledEvent>,
	name: String,
	description: String,
	entity_type: u8,
	channel: Option<Id>,
	location: String,
	start: String,
	end: String,
	repeat: u8,
}

struct OccurrenceEditor {
	inputs: TextInputStates,
	before: ScheduledEvent,
	original: i64,
	start: String,
	end: String,
	canceled: bool,
}
impl OccurrenceEditor {
	fn new(before: ScheduledEvent, original: i64) -> Self {
		let exception =
			occurrence_id(original).and_then(|id| before.exceptions.iter().find(|e| e.id == id));
		let default_end = before
			.draft
			.end
			.map(|e| original + (e - before.draft.start));
		Self {
			inputs: TextInputStates::new(
				egui::Id::unique((
					"community-occurrence-editor",
					before.guild,
					before.id,
					original,
				)),
				6,
			),
			original,
			start: format_time(exception.and_then(|e| e.start).unwrap_or(original)),
			end: exception
				.and_then(|e| e.end)
				.or(default_end)
				.map(format_time)
				.unwrap_or_default(),
			canceled: exception.is_some_and(|e| e.canceled),
			before,
		}
	}
	fn action(&self) -> Result<Action, &'static str> {
		let start = parse_event_time(&self.start)?;
		let end = if self.end.trim().is_empty() {
			None
		} else {
			Some(parse_event_time(&self.end)?)
		};
		if end.is_some_and(|e| e <= start) {
			return Err("The occurrence end must be after its start");
		}
		let default_end = self
			.before
			.draft
			.end
			.map(|e| self.original + (e - self.before.draft.start));
		Ok(Action::EditOccurrence {
			before: self.before.clone(),
			original: self.original,
			canceled: self.canceled,
			start: (start != self.original).then_some(start),
			end: end.filter(|e| Some(*e) != default_end),
		})
	}
}
impl Editor {
	fn new(guild: Id, event: Option<ScheduledEvent>) -> Self {
		let d = event
			.as_ref()
			.map(|e| e.draft.clone())
			.unwrap_or_else(|| EventDraft {
				name: String::new(),
				description: String::new(),
				entity_type: 3,
				channel: None,
				location: String::new(),
				start: (OffsetDateTime::now_utc() + time::Duration::hours(1)).unix_timestamp(),
				end: Some((OffsetDateTime::now_utc() + time::Duration::hours(2)).unix_timestamp()),
				recurrence: None,
			});
		Self {
			inputs: TextInputStates::new(
				egui::Id::unique((
					"community-event-editor",
					guild,
					event.as_ref().map(|e| e.id),
				)),
				9,
			),
			guild,
			name: d.name,
			description: d.description,
			entity_type: d.entity_type,
			channel: d.channel,
			location: d.location,
			start: format_time(d.start),
			end: d.end.map(format_time).unwrap_or_default(),
			repeat: if d.recurrence.is_some() { 1 } else { 0 },
			before: event,
		}
	}
	fn reapply_on(&mut self, current: ScheduledEvent) {
		let Some(before) = self.before.clone() else {
			return;
		};
		let old = Self::new(self.guild, Some(before));
		let mut latest = Self::new(self.guild, Some(current));
		if self.name != old.name {
			latest.name.clone_from(&self.name);
		}
		if self.description != old.description {
			latest.description.clone_from(&self.description);
		}
		if (self.entity_type, self.channel, &self.location)
			!= (old.entity_type, old.channel, &old.location)
		{
			latest.entity_type = self.entity_type;
			latest.channel = self.channel;
			latest.location.clone_from(&self.location);
		}
		if self.start != old.start {
			latest.start.clone_from(&self.start);
		}
		if self.end != old.end {
			latest.end.clone_from(&self.end);
		}
		if self.repeat != old.repeat {
			latest.repeat = self.repeat;
		}
		*self = latest;
	}

	fn draft(&self) -> Result<EventDraft, &'static str> {
		let start = parse_event_time(&self.start)?;
		let end = if self.end.trim().is_empty() {
			None
		} else {
			Some(parse_event_time(&self.end)?)
		};
		let at = OffsetDateTime::from_unix_timestamp(start)
			.map_err(|_| "Start date is outside the supported range")?;
		let recurrence = if self.repeat == 1 {
			self.before
				.as_ref()
				.and_then(|e| e.draft.recurrence.clone())
		} else if self.repeat >= 2 {
			let mut r = Recurrence {
				start,
				end: None,
				frequency: 3,
				interval: 1,
				by_weekday: vec![],
				by_n_weekday: vec![],
				by_month: vec![],
				by_month_day: vec![],
				by_year_day: vec![],
				count: None,
			};
			match self.repeat {
				2 => {}
				3 => r.by_weekday = vec![0, 1, 2, 3, 4],
				4 | 5 => {
					r.frequency = 2;
					r.interval = if self.repeat == 5 { 2 } else { 1 };
					r.by_weekday = vec![at.weekday().number_days_from_monday()];
				}
				6 => {
					r.frequency = 1;
					r.by_n_weekday = vec![NWeekday {
						n: (at.day() - 1) / 7 + 1,
						day: at.weekday().number_days_from_monday(),
					}];
				}
				7 => {
					r.frequency = 0;
					r.by_month = vec![u8::from(at.month())];
					r.by_month_day = vec![at.day()];
				}
				_ => return Err("Choose a supported recurrence"),
			}
			Some(r)
		} else {
			None
		};
		let draft = EventDraft {
			name: self.name.trim().to_owned(),
			description: self.description.clone(),
			entity_type: self.entity_type,
			channel: if self.entity_type == 3 {
				None
			} else {
				self.channel
			},
			location: if self.entity_type == 3 {
				self.location.trim().to_owned()
			} else {
				String::new()
			},
			start,
			end,
			recurrence,
		};
		if !draft.valid() {
			return Err(
				"Enter a name, matching channel or location, and an end time after the start",
			);
		}
		Ok(draft)
	}
}
fn parse_time(value: &str) -> Result<i64, &'static str> {
	if value.len() > 64 {
		return Err("Timestamp is too long");
	}
	let value = value.trim();
	let normalized = value.split_once('T').and_then(|(date, rest)| {
		let zone = rest.find(['+', '-', 'Z', 'z'])?;
		(zone == 5).then(|| format!("{date}T{}:00{}", &rest[..zone], &rest[zone..]))
	});
	OffsetDateTime::parse(normalized.as_deref().unwrap_or(value), &Rfc3339)
		.map(|t| t.unix_timestamp())
		.map_err(|_| "event-clock-invalid")
}
/// An explicit offset preserves the captured instant. Blank means the configured wall time.
fn event_wall_time(value: &str) -> Result<Vec<OffsetDateTime>, &'static str> {
	// Validate syntax and the calendar before distinguishing a clock gap from an invalid date.
	parse_time(&format!("{value}Z"))?;
	if crate::local_time::timezone_status() == Some("timezone-updating") {
		return Err("timezone-updating");
	}
	let candidates = crate::local_time::wall_time(value);
	if candidates.is_empty() {
		Err("event-clock-gap")
	} else {
		Ok(candidates)
	}
}

fn parse_event_time(value: &str) -> Result<i64, &'static str> {
	let value = value.trim();
	if value.len() > 64 {
		return Err("event-clock-invalid");
	}
	let (_, time) = value.split_once('T').ok_or("event-clock-invalid")?;
	if time.contains(['+', '-', 'Z', 'z']) {
		return parse_time(value);
	}
	match event_wall_time(value)?.as_slice() {
		[at] => Ok(at.unix_timestamp()),
		_ => Err("event-clock-fold"),
	}
}

#[cfg(test)]
#[test]
fn scheduling_accepts_minutes_and_preserves_existing_second_precision() {
	assert_eq!(
		parse_time("2027-01-09T18:30+05:30"),
		parse_time("2027-01-09T13:00:00Z")
	);
	assert_eq!(
		parse_time("2027-01-09T18:30:21+05:30"),
		parse_time("2027-01-09T13:00:21Z")
	);
	assert!(parse_time("2027-01-09T18:30").is_err());
	assert!(parse_time("2027-02-30T18:30+05:30").is_err());
}

#[cfg(test)]
#[test]
fn scheduling_requires_a_choice_at_the_fold_and_rejects_the_gap() {
	let _zone = crate::local_time::pin_timezone("Europe/London");
	assert_eq!(parse_event_time("2026-03-29T01:30"), Err("event-clock-gap"));
	assert_eq!(
		parse_event_time("2026-02-30T01:30"),
		Err("event-clock-invalid")
	);
	let wall = "2026-10-25T01:30:21";
	assert_eq!(parse_event_time(wall), Err("event-clock-fold"));
	let candidates = event_wall_time(wall).unwrap();
	assert_eq!(candidates.len(), 2);
	assert_eq!((candidates[1] - candidates[0]).whole_hours(), 1);
	for (index, candidate) in candidates.iter().enumerate() {
		let explicit = candidate.format(&Rfc3339).unwrap();
		assert_eq!(
			parse_event_time(&explicit),
			Ok(candidates[index].unix_timestamp())
		);
		assert_eq!(candidate.second(), 21);
	}
	assert_eq!(
		parse_event_time("2026-06-20T13:30"),
		parse_time("2026-06-20T12:30:00Z")
	);
}

fn can_create_event_type(state: &State, guild: Id, entity_type: u8) -> bool {
	let channel_kind = match entity_type {
		1 => 13,
		2 => 2,
		3 => return state.can_create_event_at(guild, entity_type, None),
		_ => return false,
	};
	state.channels.iter().any(|channel| {
		channel.guild == Some(guild)
			&& channel.kind == channel_kind
			&& state.can_create_event_at(guild, entity_type, Some(channel.id))
	})
}

fn owned_input(inputs: &mut TextInputStates, response: egui::Response) -> egui::Response {
	inputs.track(&response);
	response
}
fn format_time(value: i64) -> String {
	OffsetDateTime::from_unix_timestamp(value)
		.ok()
		.and_then(|t| crate::local_time::local(t).format(&Rfc3339).ok())
		.unwrap_or_default()
}
fn time_label(value: i64) -> String {
	OffsetDateTime::from_unix_timestamp(value)
		.ok()
		.map(crate::local_time::full)
		.unwrap_or_else(|| "Unknown time".into())
}
fn submit(
	state: &mut State,
	guild: Id,
	action: Action,
	commands: &mut Vec<Command>,
) -> Option<u64> {
	let command = state.request_community(guild, action)?;
	let request = if let Command::Community { request, .. } = &command {
		*request
	} else {
		return None;
	};
	commands.push(command);
	Some(request)
}
impl CommunityUi {
	fn open_stage_editor(
		&mut self,
		state: &mut State,
		channel: Id,
		topic: String,
		event: Option<Id>,
	) -> bool {
		if self.stage_dirty
			&& (self.stage_draft_channel != Some(channel) || self.stage_event != event)
		{
			self.stage_channel = self.stage_draft_channel;
			self.stage_draft_conflict = true;
			state.status = "A different Stage draft is preserved; review or discard it before starting this event";
			return false;
		}
		self.stage_draft_conflict = false;
		self.stage_channel = Some(channel);
		if !self.stage_dirty {
			self.inputs = None;
			self.stage_topic = topic;
			self.stage_event = event;
			self.stage_draft_channel = Some(channel);
		}
		self.end_stage_confirm = false;
		true
	}
	pub fn is_open(&self) -> bool {
		self.guild.is_some() || self.stage_channel.is_some()
	}

	#[cfg(any(test, feature = "demo"))]
	pub fn preview(&mut self, state: &mut State, guild: Id, detail: bool, edit: bool) {
		self.generation = state.generation;
		self.guild = Some(guild);
		state.community.opened = Some(guild);
		self.selected = if detail {
			state
				.community
				.events
				.as_ref()
				.and_then(|p| p.events.first())
				.map(|e| e.id)
		} else {
			None
		};
		if edit {
			self.draft = Some(Editor::new(
				guild,
				state
					.community
					.events
					.as_ref()
					.and_then(|p| p.events.first())
					.cloned(),
			));
		}
	}
	pub fn open_link(
		&mut self,
		state: &mut State,
		guild: Id,
		event: Id,
		commands: &mut Vec<Command>,
	) {
		if !state.can_browse_events(guild) {
			state.status = "The event server is unavailable";
			return;
		}
		if self.generation != state.generation {
			*self = Self::default();
			self.generation = state.generation;
		}
		self.guild = Some(guild);
		self.selected = Some(event);
		self.confirm = None;
		self.replacement = None;
		state.community.opened = Some(guild);
		if state
			.community
			.events
			.as_ref()
			.is_some_and(|p| p.guild != guild)
		{
			state.community.events = None;
		}
		submit(state, guild, Action::GetEvent(event), commands);
	}

	pub fn open_events(&mut self, state: &mut State, guild: Id, commands: &mut Vec<Command>) {
		if !state.can_browse_events(guild) {
			return;
		}
		if self.generation != state.generation {
			*self = Self::default();
			self.generation = state.generation;
		}
		self.guild = Some(guild);
		self.selected = None;
		self.confirm = None;
		self.replacement = None;
		state.community.opened = Some(guild);
		submit(state, guild, Action::ListEvents, commands);
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		if self.generation != state.generation {
			*self = Self::default();
			return;
		}
		self.stage_dialog(ctx, state, commands);
		let Some(guild) = self.guild else {
			return;
		};
		if !state.can_browse_events(guild) {
			self.guild = None;
			state.close_events();
			return;
		}
		if let Some((request, saved)) = self.submitted
			&& state.community.completed == request
		{
			self.submitted = None;
			match saved {
				SavedDraft::Event => self.draft = None,
				SavedDraft::Occurrence => self.occurrence_draft = None,
				SavedDraft::Confirmation => {}
			}
			self.confirm = None;
		}
		if !state.community.pending() && state.community.error.is_some() {
			self.submitted = None;
		}
		if !self.browsing_draft
			&& self.confirm.is_none()
			&& (self.occurrence_draft.is_none() || self.browsing_occurrence)
			&& self
				.draft
				.as_ref()
				.is_some_and(|draft| draft.guild == guild)
		{
			self.editor(ctx, state, guild, commands);
			return;
		}
		let server = state
			.guild(guild)
			.map_or_else(String::new, |g| g.name.clone());
		let confirm = self.confirm.clone();
		let occurrence = !self.browsing_occurrence
			&& self
				.occurrence_draft
				.as_ref()
				.is_some_and(|draft| draft.before.guild == guild);
		let allowed = confirm.as_ref().is_some_and(|action| {
			state.community_action_allowed(guild, action)
				&& !state.community.needs_refresh
				&& if action.stage_channel().is_some() {
					!state.community.stage_stale
				} else {
					!state.community.stale
				}
		});
		let response = crate::dialog::Dialog::new(("server-event-browser", self.generation, guild), crate::i18n::translate("server-events-title"))
			.subtitle(server)
			.width(640.0)
			.show(ctx, |body| {
				body.scroll(180.0, |ui| {
					ui.horizontal_wrapped(|ui| {
						if ui
							.add_enabled(!state.community.pending(), egui::Button::new(crate::i18n::translate("server-events-refresh")))
							.clicked()
						{
							submit(state, guild, Action::ListEvents, commands);
						}
						if confirm.is_none() && !occurrence && self.replacement.is_none()
							&& let Some(kind) = [3, 2, 1]
								.into_iter()
								.find(|kind| can_create_event_type(state, guild, *kind))
							&& ui.add_enabled(
								!state.community.pending(),
								egui::Button::new(crate::i18n::translate("server-events-create")),
							).clicked()
						{
							let mut draft = Editor::new(guild, None);
							draft.entity_type = kind;
							if self.draft.is_some() {
								self.replacement = Some(DraftReplacement::Event(Box::new(draft)));
							} else {
								self.draft = Some(draft);
								self.browsing_draft = false;
								self.selected = None;
							}
						}
						if state.community.pending() {
							ui.add(crate::design::Busy::new());
							ui.label("Loading…");
						}
					});
					if state.community.stale {
						ui.colored_label(
							crate::design::palette(ui).warning,
							"Event information changed. Refresh before making changes.",
						);
					}
					if let Some(error) = state.community.error {
						ui.colored_label(crate::design::palette(ui).danger, error);
					}
					if let Some(replacement) = self.replacement.as_ref() {
						let (old_name, new_name) = match replacement {
							DraftReplacement::Event(next) => (self.draft.as_ref().map(|old| old.name.as_str()), next.name.as_str()),
							DraftReplacement::Occurrence(next) => (self.occurrence_draft.as_ref().map(|old| old.before.draft.name.as_str()), next.before.draft.name.as_str()),
						};
						let untitled = crate::i18n::translate("event-draft-untitled");
						let old = old_name.filter(|name| !name.trim().is_empty()).unwrap_or(&untitled);
						let next = if new_name.trim().is_empty() { untitled.as_str() } else { new_name };
						ui.label(crate::i18n::translate_args("event-draft-replace-note", &[("old", old), ("next", next)]));
						return;
					}
					if let Some(action) = &confirm {
						ui.separator();
						let (id, occurrence) = match action {
							Action::EditOccurrence { before, original, .. } => (Some(before.id), Some(time_label(*original))),
							Action::RestoreOccurrence { before, exception } => (Some(before.id), Some(occurrence_time(*exception).map_or_else(|| exception.to_string(), time_label))),
							Action::Delete(id) | Action::Status { event: id, .. } => (Some(*id), None),
							_ => (None, None),
						};
						if let Some(id) = id {
							let captured = match action {
								Action::EditOccurrence { before, .. } | Action::RestoreOccurrence { before, .. } if before.guild == guild => Some(before),
								_ => None,
							};
							let event = captured.or_else(|| state.community.events.as_ref().filter(|page| page.guild == guild)
								.and_then(|page| page.events.iter().find(|event| event.id == id)));
							if let Some(event) = event { ui.strong(&event.draft.name); }
							else { ui.strong(crate::i18n::translate_args("event-confirm-id", &[("id", &id.to_string())])); }
							if let Some(when) = &occurrence {
								ui.label(crate::i18n::translate_args("event-confirm-occurrence", &[("time", when)]));
							} else if event.is_some_and(|event| event.draft.recurrence.is_some()) {
								design_event_series_scope(ui, action);
							}
						}
						ui.label(match action {
							Action::Delete(_) => "Permanently delete this event?",
							Action::EditOccurrence { canceled: true, .. } => {
								"Cancel only this occurrence? The recurring series remains scheduled."
							}
							Action::EditOccurrence { .. } => "Update only this occurrence?",
							Action::RestoreOccurrence { .. } => {
								"Restore this occurrence to the series schedule?"
							}
							Action::Status { status: 4, .. } => {
								"Cancel this event? Interested members will see it as canceled."
							}
							Action::Status { status: 3, .. } => "End this event?",
							_ => "Start this event now?",
						});
						if !allowed { ui.small("The event or its permissions changed. Refresh, go back and review the current event before confirming."); }
						return;
					}
					if occurrence {
						self.occurrence_editor(ui, state, guild);
						return;
					}
					if let Some(draft) = &self.occurrence_draft {
						let here = draft.before.guild == guild;
						ui.label(crate::i18n::translate_args("event-draft-saved-name", &[("name", &draft.before.draft.name)]));
							if here && crate::design::text_action(ui, "event-occurrence-continue").clicked() {
							self.browsing_occurrence = false;
						} else if !here {
							crate::design::hint(ui, "event-occurrence-different-server");
						}
						if !state.community.pending() && crate::design::text_action(ui, "event-occurrence-discard").clicked() {
							self.occurrence_draft = None;
						}
					}
					if let Some(draft) = &self.draft {
						let untitled = crate::i18n::translate("event-draft-untitled");
							let saved = if draft.name.trim().is_empty() { untitled.as_str() } else { &draft.name };
							let owner = state.guild(draft.guild).map_or_else(|| draft.guild.to_string(), |server| server.name.clone());
							ui.label(crate::i18n::translate_args("event-draft-saved-scope", &[("name", saved), ("server", &owner)]));
							if draft.guild == guild {
								if crate::design::text_action(ui, "server-events-continue").clicked() {
								self.browsing_draft = false;
							}
						} else {
							crate::design::hint(ui, "server-events-different-server");
							if crate::design::text_action(ui, "server-events-discard-draft").clicked() {
								self.draft = None;
							}
						}
					}
					ui.separator();
					ui.horizontal_wrapped(|ui| {
						ui.label("Event link");
						let inputs=self.inputs.get_or_insert_with(||TextInputStates::new(egui::Id::unique("community-inputs"),8));
						owned_input(inputs,ui.add(
							egui::TextEdit::singleline(&mut self.link)
								.id(egui::Id::unique(("community-event-link",self.generation,guild)))
								.char_limit(128)
								.hint_text("https://discord.com/events/server/event"),
						));
						if ui
							.add_enabled(!state.community.pending(), egui::Button::new("Open"))
							.clicked()
						{
							if let Some((linked, id)) = event_link(self.link.trim()) {
								if linked == guild {
									self.link_error = None;
									self.selected = Some(id);
									submit(state, guild, Action::GetEvent(id), commands);
								} else {
									self.link_error = Some("Open the link's server first");
								}
							} else {
								self.link_error = Some("Enter a canonical Discord event link");
							}
						}
					});
					if let Some(error) = self.link_error {
						ui.colored_label(crate::design::palette(ui).danger, error);
					}
					if let Some(id) = self.selected {
						if ui.button("← All events").clicked() {
							self.selected = None;
						}
						let event = state
							.community
							.events
							.as_ref()
							.filter(|p| p.guild == guild)
							.and_then(|p| {
								p.events
									.iter()
									.find(|e| e.id == id && state.event_visible(e))
							})
							.cloned();
						if let Some(event) = event {
							self.detail(ui, state, event, commands);
						} else if !state.community.pending() {
							ui.label("Event is unavailable or no longer visible.");
						}
						return;
					}
					let events = state
						.community
						.events
						.as_ref()
						.filter(|p| p.guild == guild)
						.map(|p| p.events.as_slice())
						.unwrap_or(&[]);
					if events.is_empty() && !state.community.pending() {
						ui.label(
							if state.community.error.is_some() || state.community.events.is_none() {
								"Event information is unavailable. Use Refresh to try again."
							} else {
								"No scheduled events. Use Refresh to check for new events."
							},
						);
					}
					for event in events.iter().filter(|e| state.event_visible(e)) {
						egui::Frame::group(ui.style()).show(ui, |ui| {
							ui.label(crate::design::semibold(ui, &event.draft.name, 15.0));
							ui.label(time_label(event.draft.start));
							ui.small(event_status(event.status));
							if crate::design::text_action(ui, "server-events-view").clicked() {
								self.selected = Some(event.id);
							}
						});
					}
				});
				body.footer(|ui| {
					use crate::dialog::{self, Action as Tone};
					if self.replacement.is_some() {
						if dialog::action(ui, "event-draft-keep", Tone::Neutral).clicked() {
							self.replacement = None;
						}
						if ui.add_enabled_ui(!state.community.pending(), |ui| dialog::action(ui, "event-draft-replace", Tone::Danger)).inner.clicked()
							&& let Some(replacement) = self.replacement.take()
						{
							match replacement {
								DraftReplacement::Event(next) => {
									self.selected = next.before.as_ref().map(|event| event.id);
									self.draft = Some(*next);
									self.browsing_draft = false;
								}
								DraftReplacement::Occurrence(next) => {
									self.occurrence_draft = Some(*next);
									self.browsing_occurrence = false;
								}
							}
						}
						false
					} else if let Some(action) = confirm {
						if ui.add_enabled(!state.community.pending() && allowed, egui::Button::new(crate::i18n::translate("event-action-confirm"))).clicked() {
							let saved = occurrence && matches!(&action, Action::EditOccurrence { before, original, .. }
								if self.occurrence_draft.as_ref().is_some_and(|draft| draft.before.id == before.id && draft.original == *original));
							let request = submit(state, guild, action, commands);
							self.submitted = request.map(|r| (r, if saved { SavedDraft::Occurrence } else { SavedDraft::Confirmation }));
						}
						if !state.community.pending() && dialog::action(ui, "event-action-back", Tone::Neutral).clicked() {
							self.confirm = None;
						}
						false
					} else if occurrence {
						let action = self.occurrence_draft.as_ref().and_then(|editor| editor.action().ok())
							.filter(|action| state.community_action_allowed(guild, action));
						if ui.add_enabled(!state.community.pending() && !state.community.stale && !state.community.needs_refresh && action.is_some(), egui::Button::new(crate::i18n::translate("event-occurrence-review"))).clicked() {
							self.confirm = action;
						}
						if dialog::action(ui, "event-action-back", Tone::Neutral).clicked() {
							self.browsing_occurrence = true;
						}
						false
					} else {
						dialog::action(ui, "server-events-done", Tone::Neutral).clicked()
					}
				})
			});
		if response.close && self.replacement.is_some() {
			self.replacement = None;
		} else if response.close && self.confirm.is_some() {
			if !state.community.pending() {
				self.confirm = None;
			}
		} else if response.close && occurrence {
			self.browsing_occurrence = true;
		} else if response.close || response.inner {
			self.guild = None;
			state.close_events();
		}
	}

	fn detail(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		event: ScheduledEvent,
		commands: &mut Vec<Command>,
	) {
		let guild = event.guild;
		ui.heading(&event.draft.name);
		ui.label(event_status(event.status));
		ui.label(time_label(event.draft.start));
		if let Some(end) = event.draft.end {
			ui.label(format!("Ends {}", time_label(end)));
		}
		if let Some(r) = &event.draft.recurrence {
			ui.label(format!(
				"Repeats {} · every {} · UTC schedule",
				match r.frequency {
					0 => "yearly",
					1 => "monthly",
					2 => "weekly",
					_ => "daily",
				},
				r.interval
			));
		}
		if let Some(channel) = event.draft.channel {
			ui.label(
				state
					.channel(channel)
					.map(|c| format!("In {}", c.name))
					.unwrap_or_else(|| "Channel unavailable".into()),
			);
		} else {
			ui.label(&event.draft.location);
		}
		if !event.draft.description.is_empty() {
			ui.label(&event.draft.description);
		}
		if let Some(count) = event.user_count {
			ui.small(format!("{count} interested"));
		}
		let subscribed = state.community.events.as_ref().and_then(|p| {
			p.subscriptions
				.as_ref()
				.and_then(|s| {
					s.iter()
						.find(|s| s.event == event.id && s.occurrence.is_none())
						.map(|s| s.interested)
				})
				.or(p.subscriptions_complete.then_some(false))
		});
		let ready =
			!state.community.pending() && !state.community.stale && !state.community.needs_refresh;
		ui.horizontal_wrapped(|ui| {
			if matches!(event.status, 1 | 2) {
				if ui
					.add_enabled(
						ready,
						egui::Button::new(if subscribed == Some(true) {
							"Interested ✓"
						} else {
							"Interested"
						}),
					)
					.clicked()
				{
					submit(
						state,
						guild,
						Action::Rsvp {
							event: event.id,
							interested: true,
						},
						commands,
					);
				}
				if ui
					.add_enabled(ready, egui::Button::new("Not interested"))
					.clicked()
				{
					submit(
						state,
						guild,
						Action::Rsvp {
							event: event.id,
							interested: false,
						},
						commands,
					);
				}
			}
			if ui.button("Copy event link").clicked() {
				ui.ctx().copy_text(event.link());
			}
			if let Some(channel) = event.draft.channel
				&& event.status == 2
				&& ui
					.add_enabled(
						state.can_call(channel),
						egui::Button::new(if state.is_stage(channel) {
							"Join audience"
						} else {
							"Join voice"
						}),
					)
					.clicked()
			{
				if let Some(c) = state.select(channel) {
					commands.push(c);
				}
				if let Some(c) = state.start_call(channel, false) {
					commands.push(c);
				}
			}
		});
		if subscribed.is_none() {
			ui.small(
				"Your current RSVP is unavailable. Each choice is an explicit subscription update.",
			);
		}
		self.occurrences(ui, state, &event, commands);
		if state.can_manage_event(&event) {
			ui.separator();
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(
						ready && matches!(event.status, 1 | 2),
						egui::Button::new("Edit event"),
					)
					.clicked()
				{
					if self
						.draft
						.as_ref()
						.and_then(|draft| draft.before.as_ref())
						.is_some_and(|before| before.guild == guild && before.id == event.id)
					{
						self.browsing_draft = false;
					} else if self.draft.is_some() {
						self.replacement = Some(DraftReplacement::Event(Box::new(Editor::new(
							guild,
							Some(event.clone()),
						))));
					} else {
						self.draft = Some(Editor::new(guild, Some(event.clone())));
						self.browsing_draft = false;
					}
				}
				if event.status == 1 {
					if ui
						.add_enabled(ready, egui::Button::new("Start event"))
						.clicked()
					{
						if event.draft.entity_type == 1
							&& let Some(channel) = event.draft.channel
						{
							if !self.open_stage_editor(
								state,
								channel,
								event.draft.name.clone(),
								Some(event.id),
							) {
								return;
							}
							submit(
								state,
								guild,
								Action::GetStage {
									channel,
									call_request: state
										.voice
										.active
										.as_ref()
										.filter(|c| c.channel == channel)
										.map(|c| c.request),
								},
								commands,
							);
						} else {
							self.confirm = Some(Action::Status {
								event: event.id,
								status: 2,
							});
						}
					}
					if ui
						.add_enabled(ready, egui::Button::new("Cancel event"))
						.clicked()
					{
						self.confirm = Some(Action::Status {
							event: event.id,
							status: 4,
						});
					}
				} else if event.status == 2
					&& ui
						.add_enabled(ready, egui::Button::new("End event"))
						.clicked()
				{
					self.confirm = Some(Action::Status {
						event: event.id,
						status: 3,
					});
				}
				if ui
					.add_enabled(ready, egui::Button::new("Delete event"))
					.clicked()
				{
					self.confirm = Some(Action::Delete(event.id));
				}
			});
		}
	}

	fn occurrences(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		event: &ScheduledEvent,
		commands: &mut Vec<Command>,
	) {
		let Some(rule) = &event.draft.recurrence else {
			return;
		};
		if !rule.writable() {
			ui.small("This series uses service-only recurrence fields. Its existing rule is preserved; occurrence editing is unavailable for that rule.");
			return;
		}
		let after = if event.status == 2 {
			event.draft.start
		} else {
			OffsetDateTime::now_utc().unix_timestamp()
		};
		let mut occurrences = rule.upcoming(after, 8);
		for exception in &event.exceptions {
			if let Some(original) = occurrence_time(exception.id)
				&& !occurrences.contains(&original)
			{
				occurrences.push(original);
			}
		}
		occurrences.sort_unstable();
		let ready =
			!state.community.pending() && !state.community.stale && !state.community.needs_refresh;
		egui::CollapsingHeader::new("Upcoming occurrences and exceptions").show(ui, |ui| {
			egui::ScrollArea::vertical()
				.max_height(240.0)
				.show(ui, |ui| {
					for original in occurrences {
						let Some(id) = occurrence_id(original) else {
							continue;
						};
						let exception = event.exceptions.iter().find(|e| e.id == id);
						let canceled = exception.is_some_and(|e| e.canceled);
						egui::Frame::group(ui.style()).show(ui, |ui| {
							ui.label(time_label(
								exception.and_then(|e| e.start).unwrap_or(original),
							));
							if canceled {
								ui.label("This occurrence is canceled");
							} else if exception.is_some() {
								ui.small(format!("Originally {}", time_label(original)));
							}
							let subscriptions = state
								.community
								.events
								.as_ref()
								.and_then(|p| p.subscriptions.as_ref());
							let response = subscriptions.and_then(|s| {
								s.iter()
									.find(|s| s.event == event.id && s.occurrence == Some(id))
							});
							ui.small(match response {
								Some(s) if s.interested => "You are interested in this occurrence",
								Some(_) => "You are skipping this occurrence",
								None if state
									.community
									.events
									.as_ref()
									.is_some_and(|p| p.subscriptions_complete) =>
								{
									"Using your series response"
								}
								None => "Your occurrence response is unavailable",
							});
							let has_override = response.is_some();
							ui.horizontal_wrapped(|ui| {
								if !canceled && matches!(event.status, 1 | 2) {
									for (interested, label) in [
										(true, "Interested in this occurrence"),
										(false, "Skip this occurrence"),
									] {
										if ui.add_enabled(ready, egui::Button::new(label)).clicked()
										{
											submit(
												state,
												event.guild,
												Action::RsvpOccurrence {
													event: event.id,
													occurrence: id,
													interested,
												},
												commands,
											);
										}
									}
									if has_override
										&& ui
											.add_enabled(
												ready,
												egui::Button::new("Use series response"),
											)
											.clicked()
									{
										submit(
											state,
											event.guild,
											Action::ResetOccurrenceRsvp {
												event: event.id,
												occurrence: id,
											},
											commands,
										);
									}
								}
								if state.can_manage_event(event) && matches!(event.status, 1 | 2) {
									if ui
										.add_enabled(
											ready && self.replacement.is_none(),
											egui::Button::new("Edit occurrence"),
										)
										.clicked()
									{
										if self.occurrence_draft.as_ref().is_some_and(|old| {
											old.before.guild == event.guild
												&& old.before.id == event.id && old.original == original
										}) {
											self.browsing_occurrence = false;
										} else if self.occurrence_draft.is_some() {
											self.replacement =
												Some(DraftReplacement::Occurrence(Box::new(
													OccurrenceEditor::new(event.clone(), original),
												)));
										} else {
											self.occurrence_draft = Some(OccurrenceEditor::new(
												event.clone(),
												original,
											));
											self.browsing_occurrence = false;
										}
									}
									if !canceled
										&& ui
											.add_enabled(
												ready,
												egui::Button::new("Cancel occurrence"),
											)
											.clicked()
									{
										self.confirm = Some(Action::EditOccurrence {
											before: event.clone(),
											original,
											canceled: true,
											start: exception.and_then(|e| e.start),
											end: exception.and_then(|e| e.end),
										});
									}
									if exception.is_some()
										&& ui
											.add_enabled(
												ready,
												egui::Button::new("Restore schedule"),
											)
											.clicked()
									{
										self.confirm = Some(Action::RestoreOccurrence {
											before: event.clone(),
											exception: id,
										});
									}
								}
							});
						});
					}
				});
		});
	}
	fn occurrence_editor(&mut self, ui: &mut egui::Ui, state: &State, guild: Id) {
		let Some(editor) = &mut self.occurrence_draft else {
			return;
		};
		if editor.before.guild != guild {
			ui.label("An occurrence draft for another server is preserved.");
			return;
		}
		ui.heading("Edit one occurrence");
		ui.label(format!(
			"Original schedule: {}",
			time_label(editor.original)
		));
		Self::clock_fields(
			ui,
			&mut editor.inputs,
			"start",
			"event-clock-start",
			&mut editor.start,
		);
		Self::clock_fields(
			ui,
			&mut editor.inputs,
			"end",
			"event-clock-end",
			&mut editor.end,
		);
		ui.checkbox(&mut editor.canceled, "Cancel this occurrence");
		let current = state
			.community
			.events
			.as_ref()
			.filter(|p| p.guild == guild)
			.and_then(|p| p.events.iter().find(|event| event.id == editor.before.id));
		if current != Some(&editor.before) {
			crate::design::hint(ui, "event-draft-changed-review");
			if !state.community.pending()
				&& !state.community.stale
				&& !state.community.needs_refresh
				&& let Some(current) = current
					.filter(|event| state.can_manage_event(event) && matches!(event.status, 1 | 2))
			{
				let baseline = OccurrenceEditor::new(editor.before.clone(), editor.original);
				let mut latest = OccurrenceEditor::new(current.clone(), editor.original);
				if editor.start != baseline.start {
					latest.start.clone_from(&editor.start);
				}
				if editor.end != baseline.end {
					latest.end.clone_from(&editor.end);
				}
				if editor.canceled != baseline.canceled {
					latest.canceled = editor.canceled;
				}
				if latest
					.action()
					.is_ok_and(|action| state.community_action_allowed(guild, &action))
					&& crate::design::text_action(ui, "event-draft-reapply").clicked()
				{
					*editor = latest;
				}
			}
		}
		if let Err(error) = editor.action() {
			crate::design::hint(ui, error);
		}
	}

	fn editor(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		guild: Id,
		commands: &mut Vec<Command>,
	) {
		let Some(editor) = self.draft.as_mut() else {
			return;
		};
		let event_id = editor.before.as_ref().map(|event| event.id);
		let title = crate::i18n::translate(if event_id.is_some() {
			"server-events-edit"
		} else {
			"server-events-create"
		});
		let server = state
			.guild(guild)
			.map_or_else(String::new, |guild| guild.name.clone());
		let response = crate::dialog::Dialog::new(
			("server-event-editor", state.generation, guild, event_id),
			title,
		)
		.subtitle(server)
		.width(600.0)
		.show(ctx, |body| {
			body.scroll(280.0, |ui| Self::event_fields(editor, ui, state, guild));
			let built = if state.community.stale || state.community.needs_refresh {
				Err("server-events-save-stale")
			} else if !state.demo && !state.gateway_connected {
				Err("server-events-save-offline")
			} else if editor.before.as_ref().is_some_and(|before| {
				!state
					.community
					.events
					.as_ref()
					.is_some_and(|page| page.guild == guild && page.events.contains(before))
			}) {
				Err("server-events-save-changed")
			} else {
				editor.draft().and_then(|draft| {
					let action = if let Some(before) = &editor.before {
						Action::Edit {
							before: before.clone(),
							after: draft,
						}
					} else {
						Action::Create(draft)
					};
					state
						.community_action_allowed(guild, &action)
						.then_some(action)
						.ok_or("server-events-save-denied")
				})
			};
			body.content(|ui| {
				if let Some(error) = state.community.error {
					crate::dialog::notice(ui, crate::dialog::Level::Warning, error);
				}
				if let Err(error) = &built {
					crate::design::hint(ui, error);
					if *error == "server-events-save-changed"
						&& !state.community.pending()
						&& let Some(current) = state
							.community
							.events
							.as_ref()
							.filter(|page| page.guild == guild)
							.and_then(|page| {
								page.events.iter().find(|event| Some(event.id) == event_id)
							})
							.filter(|event| {
								state.can_manage_event(event) && matches!(event.status, 1 | 2)
							}) && crate::design::text_action(ui, "event-draft-reapply").clicked()
					{
						editor.reapply_on(current.clone());
					}
				}
				if state.community.pending() {
					ui.horizontal(|ui| {
						ui.add(crate::design::Busy::new());
						ui.label(crate::i18n::translate("server-events-working"));
					});
				}
			});
			body.footer(|ui| {
				use crate::dialog::{Action as Button, action};
				let pending = state.community.pending();
				let save = ui
					.add_enabled_ui(!pending && built.is_ok(), |ui| {
						action(ui, "server-events-save", Button::Primary).clicked()
					})
					.inner;
				let discard = ui
					.add_enabled_ui(!pending, |ui| {
						action(ui, "server-events-discard-draft", Button::Neutral).clicked()
					})
					.inner;
				let refresh = ui
					.add_enabled_ui(!pending && (state.demo || state.gateway_connected), |ui| {
						action(ui, "server-events-refresh", Button::Neutral).clicked()
					})
					.inner;
				let back = action(ui, "server-events-back", Button::Neutral).clicked();
				(
					discard,
					back,
					if save {
						built.ok()
					} else if refresh {
						Some(Action::ListEvents)
					} else {
						None
					},
				)
			})
		});
		let (discard, back, action) = response.inner;
		if response.close {
			self.guild = None;
			state.close_events();
		} else if discard {
			self.draft = None;
		} else if back {
			self.browsing_draft = true;
		} else if let Some(action) = action {
			let write = action.write();
			let request = submit(state, guild, action, commands);
			if write {
				self.submitted = request.map(|request| (request, SavedDraft::Event));
			}
		}
	}
	fn event_fields(editor: &mut Editor, ui: &mut egui::Ui, state: &State, guild: Id) {
		ui.label("Name");
		let id = editor.inputs.id("name");
		owned_input(
			&mut editor.inputs,
			ui.add(
				egui::TextEdit::singleline(&mut editor.name)
					.id(id)
					.char_limit(100)
					.desired_width(f32::INFINITY),
			),
		);
		ui.label("Description");
		let id = editor.inputs.id("description");
		owned_input(
			&mut editor.inputs,
			ui.add(
				egui::TextEdit::multiline(&mut editor.description)
					.id(id)
					.char_limit(1000)
					.desired_rows(3)
					.desired_width(f32::INFINITY),
			),
		);
		ui.horizontal_wrapped(|ui| {
			for (kind, label) in [(1, "Stage"), (2, "Voice"), (3, "Somewhere else")] {
				let enabled = editor.before.is_some() || can_create_event_type(state, guild, kind);
				ui.add_enabled_ui(enabled, |ui| {
					if ui
						.selectable_value(&mut editor.entity_type, kind, label)
						.changed() && editor.before.is_none()
					{
						editor.channel = None;
					}
				});
			}
		});
		if editor.entity_type == 3 {
			ui.label("Location");
			let id = editor.inputs.id("location");
			owned_input(
				&mut editor.inputs,
				ui.add(
					egui::TextEdit::singleline(&mut editor.location)
						.id(id)
						.char_limit(100)
						.desired_width(f32::INFINITY),
				),
			);
		} else {
			egui::ComboBox::from_id_salt("event-channel")
				.selected_text(
					editor
						.channel
						.and_then(|id| state.channel(id))
						.map(|c| c.name.as_str())
						.unwrap_or("Choose channel"),
				)
				.show_ui(ui, |ui| {
					for c in state.channels.iter().filter(|c| {
						c.guild == Some(guild)
							&& c.kind == if editor.entity_type == 1 { 13 } else { 2 }
							&& state.can_view(c.id)
							&& (editor.before.is_some()
								|| state.can_create_event_at(guild, editor.entity_type, Some(c.id)))
					}) {
						ui.selectable_value(&mut editor.channel, Some(c.id), &c.name);
					}
				});
		}
		Self::clock_fields(
			ui,
			&mut editor.inputs,
			"start",
			"event-clock-start",
			&mut editor.start,
		);
		Self::clock_fields(
			ui,
			&mut editor.inputs,
			"end",
			"event-clock-end",
			&mut editor.end,
		);
		let names = [
			"Does not repeat",
			"Keep existing recurrence",
			"Daily",
			"Weekdays",
			"Weekly",
			"Every other week",
			"Monthly (same weekday)",
			"Yearly",
		];
		egui::ComboBox::from_id_salt("event-repeat")
			.selected_text(names[usize::from(editor.repeat.min(7))])
			.show_ui(ui, |ui| {
				for (i, name) in names.iter().enumerate() {
					if i != 1
						|| editor
							.before
							.as_ref()
							.is_some_and(|e| e.draft.recurrence.is_some())
					{
						ui.selectable_value(&mut editor.repeat, i as u8, *name);
					}
				}
			});
		if editor.repeat != 0 {
			crate::design::hint(ui, "event-clock-recurrence");
			if let Ok(draft) = editor.draft()
				&& let Some(repeat) = draft.recurrence
			{
				for instant in repeat.upcoming(OffsetDateTime::now_utc().unix_timestamp(), 3) {
					ui.small(time_label(instant));
				}
			}
		}
	}
	fn clock_fields(
		ui: &mut egui::Ui,
		inputs: &mut TextInputStates,
		key: &str,
		label: &str,
		value: &mut String,
	) {
		use crate::i18n::{translate, translate_args};
		ui.strong(translate(label));
		let (day, rest) = value.split_once('T').unwrap_or((value.as_str(), ""));
		let zone_at = rest.find(['+', '-', 'Z', 'z']).unwrap_or(rest.len());
		let mut date = day.to_owned();
		let mut clock = rest[..zone_at].to_owned();
		let mut zone = rest[zone_at..].to_owned();
		let root = inputs.id(key);
		let changed = ui
			.horizontal_wrapped(|ui| {
				let each = |ui: &mut egui::Ui,
				            id: &str,
				            value: &mut String,
				            caption: &str,
				            hint: &str,
				            width: f32| {
					let name = ui.label(translate(caption));
					ui.add(
						egui::TextEdit::singleline(value)
							.id(root.with(id))
							.char_limit(24)
							.desired_width(width)
							.hint_text(hint),
					)
					.labelled_by(name.id)
				};
				let day = each(
					ui,
					"date",
					&mut date,
					"event-clock-date",
					"YYYY-MM-DD",
					86.0,
				);
				let time = each(ui, "time", &mut clock, "event-clock-time", "HH:MM", 62.0);
				let offset = each(ui, "zone", &mut zone, "event-clock-zone", "+00:00", 58.0);
				let changed = day.changed() || time.changed() || offset.changed();
				for response in [day, time, offset] {
					inputs.track(&response);
				}
				changed
			})
			.inner;
		let picked = crate::search::date_picker::event_pick(
			ui,
			root,
			key,
			crate::search::date_picker::parse_date(date.trim()),
		);
		if let Some(picked) = picked {
			date = picked.to_string();
		}
		let use_local = !zone.is_empty()
			&& !date.is_empty()
			&& !clock.is_empty()
			&& crate::design::text_action(ui, "event-clock-use-local").clicked();
		if use_local {
			zone.clear();
		}
		if changed || picked.is_some() || use_local {
			if date.trim().is_empty() && clock.trim().is_empty() && zone.trim().is_empty() {
				value.clear();
			} else {
				*value = format!("{}T{}{}", date.trim(), clock.trim(), zone.trim());
			}
		}
		if zone.is_empty() && !value.is_empty() {
			ui.small(translate_args(
				"event-clock-local-zone",
				&[("zone", &crate::local_time::timezone_label())],
			));
			if let Ok(candidates) = event_wall_time(value)
				&& let [early, late] = candidates.as_slice()
			{
				crate::design::hint(ui, "event-clock-fold");
				ui.horizontal_wrapped(|ui| {
					for option in [early, late] {
						let Ok(label) = option.format(&Rfc3339) else {
							continue;
						};
						let offset = option.offset().to_string();
						let offset = offset.get(..6).unwrap_or(&offset);
						if ui
							.button(translate_args(
								"event-clock-choose-offset",
								&[("offset", offset)],
							))
							.on_hover_text(&label)
							.clicked()
						{
							*value = label;
						}
					}
				});
			}
		}
		if !value.is_empty() {
			match parse_event_time(value) {
				Ok(at) => {
					ui.small(translate_args(
						"event-clock-local-preview",
						&[("time", &time_label(at))],
					));
				}
				Err(reason) if reason != "event-clock-fold" => {
					crate::design::hint(ui, reason);
				}
				_ => {}
			}
		}
	}

	pub fn stage_controls(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		channel: Id,
		commands: &mut Vec<Command>,
	) {
		if !state.is_stage(channel) || !state.can_view(channel) {
			return;
		}
		if self.generation != state.generation {
			*self = Self::default();
			self.generation = state.generation;
		}
		let Some(guild) = state.channel(channel).and_then(|c| c.guild) else {
			return;
		};
		if self.requested_stage != Some(channel) && !state.community.pending() {
			self.requested_stage = Some(channel);
			submit(
				state,
				guild,
				Action::GetStage {
					channel,
					call_request: state
						.voice
						.active
						.as_ref()
						.filter(|c| c.channel == channel)
						.map(|c| c.request),
				},
				commands,
			);
		}
		let topic = state
			.community
			.stage
			.as_ref()
			.filter(|s| s.channel == channel)
			.map(|s| s.topic.clone());
		if let Some(topic) = topic.as_deref() {
			ui.heading(topic);
		} else {
			ui.heading(crate::i18n::translate("stage-channel"));
		}
		ui.small(crate::i18n::translate("stage-transport-privacy"));
		let ready = !state.community.pending()
			&& !state.community.stage_stale
			&& !state.community.needs_refresh;
		stage_action_row(ui, |ui| {
			if stage_button(ui, !state.community.pending(), "stage-refresh", false) {
				submit(
					state,
					guild,
					Action::GetStage {
						channel,
						call_request: state
							.voice
							.active
							.as_ref()
							.filter(|c| c.channel == channel)
							.map(|c| c.request),
					},
					commands,
				);
			}
			if state.stage_moderator(channel)
				&& stage_button(
					ui,
					ready,
					if topic.is_some() {
						"stage-edit-topic"
					} else {
						"stage-start"
					},
					topic.is_none(),
				) {
				self.open_stage_editor(
					state,
					channel,
					topic.as_deref().unwrap_or_default().to_owned(),
					None,
				);
			}
		});
		if let Some(error) = state.community.error {
			ui.colored_label(crate::design::palette(ui).danger, error);
		}
		let call = state
			.voice
			.active
			.as_ref()
			.filter(|c| c.channel == channel)
			.map(|c| (c.request, c.stage_role(), c.phase));
		if let Some((call_request, stage, phase)) = call {
			if !matches!(
				phase,
				client_core::voice::Phase::Connected | client_core::voice::Phase::Waiting
			) {
				if phase == client_core::voice::Phase::Failed {
					crate::design::notice(
						ui,
						crate::design::Level::Warning,
						"stage-connection-failed",
					);
				} else {
					ui.label(crate::i18n::translate_if_key(phase.label()));
				}
				return;
			}
			if phase == client_core::voice::Phase::Waiting {
				ui.label(crate::i18n::translate_if_key(phase.label()));
			}
			let mut action = None;
			stage_action_row(ui, |ui| {
				match stage {
					StageParticipation::Speaker => {
						ui.label(crate::i18n::translate("stage-you-are-speaker"));
						if stage_button(ui, ready, "stage-move-audience", false) {
							action = Some(StageVoiceChange::Audience);
						}
					}
					StageParticipation::Invited => {
						ui.label(crate::i18n::translate("stage-invitation-hint"));
						if stage_button(ui, ready, "stage-accept-invitation", true) {
							action = Some(StageVoiceChange::AcceptInvitation);
						}
						if stage_button(ui, ready, "stage-decline", false) {
							action = Some(StageVoiceChange::Audience);
						}
					}
					StageParticipation::Audience { requested } => {
						ui.label(crate::i18n::translate(if requested {
							"stage-request-sent"
						} else {
							"stage-audience-muted"
						}));
						if stage_button(
							ui,
							ready
								&& (requested
									|| state
										.permission(channel, model::permissions::REQUEST_TO_SPEAK)
										== Some(true)),
							if requested {
								"stage-cancel-request"
							} else {
								"stage-request-to-speak"
							},
							!requested,
						) {
							action = Some(StageVoiceChange::RequestToSpeak(!requested));
						}
					}
					StageParticipation::Unknown => {
						ui.label(crate::i18n::translate("stage-waiting-for-state"));
					}
				}
				if stage != StageParticipation::Speaker
					&& state.permission(channel, model::permissions::MUTE_MEMBERS) == Some(true)
					&& stage_button(ui, ready, "stage-moderator-become-speaker", false)
				{
					action = Some(StageVoiceChange::BecomeSpeaker);
				}
			});
			if let Some(change) = action {
				submit(
					state,
					guild,
					Action::StageVoice {
						channel,
						call_request,
						change,
					},
					commands,
				);
			}

			let moderator =
				state.permission(channel, model::permissions::MUTE_MEMBERS) == Some(true);
			let own = state.user.as_ref().map(|u| u.id);
			let total = state
				.voice
				.roster
				.iter()
				.filter(|r| r.channel == channel && Some(r.participant.user) != own)
				.count();
			let pages = total.div_ceil(64).max(1);
			self.audience_page = self.audience_page.min(pages - 1);
			let participants = state
				.voice
				.roster
				.iter()
				.filter(|r| r.channel == channel && Some(r.participant.user) != own)
				.skip(self.audience_page * 64)
				.take(64)
				.map(|r| {
					(
						r.participant,
						r.member
							.as_ref()
							.map(|m| m.nick.as_deref().unwrap_or(&m.user.name).to_owned())
							.unwrap_or_else(|| r.participant.user.to_string()),
					)
				})
				.collect::<Vec<_>>();
			egui::CollapsingHeader::new(crate::i18n::translate_args(
				"stage-members",
				&[("count", &total.to_string())],
			))
			.show(ui, |ui| {
				stage_action_row(ui, |ui| {
					if stage_button(
						ui,
						self.audience_page > 0,
						"search-page-previous-short",
						false,
					) {
						self.audience_page = self.audience_page.saturating_sub(1);
					}
					ui.label(crate::i18n::translate_args(
						"stage-page",
						&[
							("current", &(self.audience_page + 1).to_string()),
							("total", &pages.to_string()),
						],
					));
					if stage_button(
						ui,
						self.audience_page + 1 < pages,
						"search-page-next-short",
						false,
					) {
						self.audience_page += 1;
					}
				});
				egui::ScrollArea::vertical()
					.max_height(140.0)
					.show(ui, |ui| {
						for (p, name) in participants {
							stage_action_row(ui, |ui| {
								ui.label(name);
								ui.small(crate::i18n::translate(match p.stage {
									StageParticipation::Speaker => "stage-speaker",
									StageParticipation::Invited => "stage-invitation-pending",
									StageParticipation::Audience { requested: true } => {
										"stage-requested-to-speak"
									}
									_ => "stage-audience",
								}));
								let speaker = matches!(
									p.stage,
									StageParticipation::Speaker | StageParticipation::Invited
								);
								if moderator
									&& stage_button(
										ui,
										ready,
										if speaker {
											"stage-move-audience"
										} else {
											"stage-invite-to-speak"
										},
										false,
									) {
									submit(
										state,
										guild,
										Action::StageVoice {
											channel,
											call_request,
											change: if speaker {
												StageVoiceChange::MoveToAudience(p.user)
											} else {
												StageVoiceChange::Invite(p.user)
											},
										},
										commands,
									);
								}
							});
						}
					});
			});
		} else {
			ui.small(crate::i18n::translate("stage-join-audience-hint"));
		}
		ui.separator();
	}
	fn stage_dialog(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		if self
			.stage_submitted
			.is_some_and(|r| state.community.completed == r)
		{
			self.stage_submitted = None;
			self.stage_dirty = false;
		}
		if !state.community.pending() && state.community.error.is_some() {
			self.stage_submitted = None;
		}
		let Some(channel) = self.stage_channel else {
			return;
		};
		if !state.stage_moderator(channel) {
			self.stage_channel = None;
			return;
		}
		let Some(guild) = state.channel(channel).and_then(|c| c.guild) else {
			return;
		};
		let exists = state
			.community
			.stage
			.as_ref()
			.is_some_and(|s| s.channel == channel);
		let event_conflict = exists
			&& self.stage_event.is_some_and(|id| {
				state
					.community
					.stage
					.as_ref()
					.is_some_and(|s| s.event != Some(id))
			});
		let mut open = true;
		egui::Window::new(crate::i18n::translate(if exists {
			"stage-topic"
		} else {
			"stage-start"
		}))
		.id(egui::Id::unique("stage-topic-editor"))
		.open(&mut open)
		.default_width(420.0)
		.min_width((ctx.content_rect().width() - 32.0).clamp(1.0, 240.0))
		.max_width((ctx.content_rect().width() - 32.0).max(1.0))
		.show(ctx, |ui| {
			ui.label(crate::i18n::translate("channel-menu-overview-topic"));
			if self.stage_draft_conflict {
				crate::design::notice(
					ui,
					crate::design::Level::Warning,
					"stage-draft-target-conflict",
				);
			}
			if let Some(event) = self.stage_event.and_then(|id| {
				state
					.community
					.events
					.as_ref()
					.and_then(|p| p.events.iter().find(|e| e.id == id))
			}) {
				ui.label(crate::i18n::translate_args(
					"stage-scheduled-event",
					&[("name", &event.draft.name)],
				));
			}
			let inputs = self.inputs.get_or_insert_with(|| {
				TextInputStates::new(egui::Id::unique("community-inputs"), 8)
			});
			if owned_input(
				inputs,
				ui.add(
					egui::TextEdit::multiline(&mut self.stage_topic)
						.id(egui::Id::unique((
							"community-topic",
							self.generation,
							channel,
						)))
						.char_limit(120)
						.desired_rows(2)
						.desired_width(f32::INFINITY),
				),
			)
			.changed()
			{
				self.stage_dirty = true;
			}
			if let Some(error) = state.community.error {
				ui.colored_label(crate::design::palette(ui).danger, error);
			}
			if event_conflict {
				ui.label(crate::i18n::translate("stage-event-conflict"));
			}
			if self.end_stage_confirm {
				ui.label(crate::i18n::translate("stage-end-confirmation"));
				ui.horizontal_wrapped(|ui| {
					if ui
						.add_enabled_ui(
							!state.community.pending()
								&& !state.community.stage_stale
								&& !state.community.needs_refresh
								&& state
									.community_action_allowed(guild, &Action::EndStage(channel)),
							|ui| {
								crate::design::button(
									ui,
									"stage-end",
									crate::design::ButtonKind::Danger,
								)
							},
						)
						.inner
						.clicked()
					{
						self.stage_submitted =
							submit(state, guild, Action::EndStage(channel), commands);
						self.end_stage_confirm = false;
					}
					if stage_button(ui, true, "join-server-audience-back", false) {
						self.end_stage_confirm = false;
					}
				});
				return;
			}
			let action = if exists {
				Action::EditStage {
					channel,
					topic: self.stage_topic.trim().to_owned(),
				}
			} else {
				Action::CreateStage {
					channel,
					topic: self.stage_topic.trim().to_owned(),
					event: self.stage_event,
				}
			};
			let allowed = state.community_action_allowed(guild, &action);
			ui.horizontal_wrapped(|ui| {
				if stage_button(
					ui,
					!state.community.pending()
						&& !state.community.stage_stale
						&& !state.community.needs_refresh
						&& !event_conflict && allowed,
					if exists {
						"stage-save-topic"
					} else {
						"stage-start"
					},
					true,
				) {
					self.stage_dirty = true;
					self.stage_submitted = submit(state, guild, action, commands);
				}
				if exists
					&& stage_button(ui, !state.community.pending(), "stage-end-ellipsis", false)
				{
					self.end_stage_confirm = true;
				}
			});
			if self.stage_dirty
				&& !state.community.pending()
				&& stage_button(ui, true, "stage-discard-topic", false)
			{
				self.stage_topic = state
					.community
					.stage
					.as_ref()
					.filter(|s| s.channel == channel)
					.map(|s| s.topic.clone())
					.unwrap_or_default();
				self.stage_event = None;
				self.stage_dirty = false;
				self.inputs = None;
			}
			ui.small(crate::i18n::translate("stage-start-hint"));
		});
		if !open {
			self.stage_channel = None;
		}
	}
}

// An enabled scope gets only the remainder of a wrapping row. On a narrow
// stage that can be one glyph wide after the audience label.
fn stage_action_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
	if ui.available_width() < 400.0 {
		ui.vertical(add);
	} else {
		ui.horizontal_wrapped(add);
	}
}

fn stage_button(ui: &mut egui::Ui, enabled: bool, label: &str, primary: bool) -> bool {
	ui.add_enabled_ui(enabled, |ui| {
		crate::design::button(
			ui,
			label,
			if primary {
				crate::design::ButtonKind::Primary
			} else {
				crate::design::ButtonKind::Outline
			},
		)
	})
	.inner
	.clicked()
}

fn design_event_series_scope(ui: &mut egui::Ui, action: &Action) {
	crate::design::hint(
		ui,
		if matches!(action, Action::Delete(_)) || matches!(action, Action::Status { status: 4, .. })
		{
			"event-confirm-entire-series"
		} else {
			"event-confirm-scheduled-series"
		},
	);
}

fn event_status(status: u8) -> &'static str {
	match status {
		1 => "Scheduled",
		2 => "Live now",
		3 => "Completed",
		4 => "Canceled",
		_ => "Unknown",
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn stage_labels_and_dynamic_values_are_available_in_each_language() {
		for language in crate::i18n::Language::ALL {
			crate::i18n::set_current(language);
			assert_ne!(crate::i18n::translate("stage-start"), "stage-start");
			assert!(crate::i18n::translate("stage-start-hint").contains("@everyone"));
			assert!(crate::i18n::translate("stage-transport-privacy").contains("Discord"));
			let page =
				crate::i18n::translate_args("stage-page", &[("current", "4"), ("total", "17")]);
			assert!(
				page.contains('4') && page.contains("17"),
				"{language:?}: {page}"
			);
			let members = crate::i18n::translate_args("stage-members", &[("count", "123")]);
			assert!(members.contains("123"), "{language:?}: {members}");
		}
		crate::i18n::set_current(crate::i18n::Language::English);
	}

	#[test]
	fn editor_uses_explicit_offsets_and_recurs_from_utc_weekday() {
		let mut editor = Editor::new(Id(1), None);
		editor.name = "Synthetic event".into();
		editor.location = "Outside".into();
		editor.start = "2027-01-02T00:30:00+02:00".into();
		editor.end = "2027-01-02T01:30:00+02:00".into();
		editor.repeat = 4;
		let d = editor.draft().unwrap();
		assert_eq!(d.recurrence.unwrap().by_weekday, vec![4]);
		editor.start = "2027-03-28T02:30:00".into();
		assert!(editor.draft().is_err());
	}
	fn labels(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
		match shape {
			egui::Shape::Text(t) => out.push((
				t.galley.job.text.clone(),
				t.galley.rect.translate(t.pos.to_vec2()),
			)),
			egui::Shape::Vec(shapes) => {
				for shape in shapes {
					labels(shape, out)
				}
			}
			_ => {}
		}
	}
	fn frame(
		ctx: &egui::Context,
		view: &mut CommunityUi,
		state: &mut State,
		size: egui::Vec2,
		events: Vec<egui::Event>,
	) -> (Vec<Command>, Vec<(String, egui::Rect)>) {
		let mut commands = vec![];
		let mut output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
				events,
				..Default::default()
			},
			|ui| view.show(ui.ctx(), state, &mut commands),
		);
		output.textures_delta.clear();
		let mut text = vec![];
		for clipped in output.shapes {
			let mut painted = vec![];
			labels(&clipped.shape, &mut painted);
			text.extend(
				painted
					.into_iter()
					.filter(|(_, rect)| clipped.clip_rect.contains(rect.center())),
			);
		}
		(commands, text)
	}
	fn scroll_to_label(
		ctx: &egui::Context,
		view: &mut CommunityUi,
		state: &mut State,
		size: egui::Vec2,
		label: &str,
		down: bool,
	) -> Vec<(String, egui::Rect)> {
		let mut text = frame(ctx, view, state, size, vec![]).1;
		for _ in 0..4 {
			if text.iter().any(|(s, _)| s == label) {
				return text;
			}
			let pos = text.last().expect("visible scroll content").1.center();
			let (commands, _) = frame(
				ctx,
				view,
				state,
				size,
				vec![
					egui::Event::PointerMoved(pos),
					egui::Event::MouseWheel {
						unit: egui::MouseWheelUnit::Point,
						phase: egui::TouchPhase::Move,
						source: egui::MouseWheelSource::Unknown,
						delta: egui::vec2(0.0, if down { -1200.0 } else { 1200.0 }),
						modifiers: egui::Modifiers::NONE,
					},
				],
			);
			assert!(commands.is_empty());
			for _ in 0..16 {
				let (commands, labels) = frame(ctx, view, state, size, vec![]);
				assert!(commands.is_empty());
				text = labels;
			}
		}
		assert!(
			text.iter().any(|(s, _)| s == label),
			"{label} unreachable after scrolling: {text:?}"
		);
		text
	}
	#[test]
	fn stage_draft_is_not_relabelled_as_a_different_scheduled_event() {
		let mut view = CommunityUi::default();
		let mut state = State::default();
		assert!(view.open_stage_editor(&mut state, Id(2), "Our draft".into(), None));
		view.stage_dirty = true;
		assert!(!view.open_stage_editor(&mut state, Id(2), "Scheduled".into(), Some(Id(3))));
		assert_eq!(view.stage_event, None);
		assert_eq!(view.stage_topic, "Our draft");
		view.stage_dirty = false;
		assert!(view.open_stage_editor(&mut state, Id(2), "Scheduled".into(), Some(Id(3))));
		assert_eq!(view.stage_event, Some(Id(3)));
	}

	#[test]
	fn event_creation_requires_an_eligible_location_and_keeps_a_draft_on_revocation() {
		use model::permissions as p;
		let ctx = egui::Context::default();
		let mut state = test_support::community_state(false);
		let guild = state.community.opened.unwrap();
		let voice = state
			.channels
			.iter()
			.find(|c| c.guild == Some(guild) && c.kind == 2)
			.unwrap()
			.id;
		let snapshot = test_support::permission_snapshot(&state);
		state.permissions.replace(snapshot).unwrap();
		assert!(state.can_browse_events(guild));
		let mut view = CommunityUi {
			guild: Some(guild),
			generation: state.generation,
			..Default::default()
		};
		let size = egui::vec2(1000.0, 1100.0);
		let create = crate::i18n::translate("server-events-create");
		for _ in 0..3 {
			let (commands, text) = frame(&ctx, &mut view, &mut state, size, vec![]);
			assert!(commands.is_empty());
			assert!(!text.iter().any(|(label, _)| label == &create));
		}
		assert!(view.draft.is_none());

		let mut snapshot = test_support::permission_snapshot(&state);
		snapshot
			.channels
			.iter_mut()
			.find(|c| c.id == voice)
			.unwrap()
			.overwrites = Some(vec![p::Overwrite {
			id: guild,
			kind: 0,
			allow: p::CREATE_EVENTS,
			deny: 0,
		}]);
		state.permissions.replace(snapshot).unwrap();
		assert!(!state.can_create_event_at(guild, 3, None));
		assert!(state.can_create_event_at(guild, 2, Some(voice)));
		let pos = scroll_to_label(&ctx, &mut view, &mut state, size, &create, false)
			.into_iter()
			.find(|(label, _)| label == &create)
			.unwrap()
			.1
			.center();
		for pressed in [true, false] {
			let (commands, _) = frame(
				&ctx,
				&mut view,
				&mut state,
				size,
				vec![
					egui::Event::PointerMoved(pos),
					egui::Event::PointerButton {
						pos,
						button: egui::PointerButton::Primary,
						pressed,
						modifiers: egui::Modifiers::NONE,
					},
				],
			);
			assert!(commands.is_empty());
		}
		let draft = view.draft.as_mut().unwrap();
		assert_eq!(draft.entity_type, 2);
		assert_eq!(
			draft.channel, None,
			"a channel still requires an explicit choice"
		);
		assert!(draft.draft().is_err());
		draft.channel = Some(voice);
		draft.name = "Keep this draft".into();
		let snapshot = test_support::permission_snapshot(&state);
		state.permissions.replace(snapshot).unwrap();
		let denied = crate::i18n::translate("server-events-save-denied");
		scroll_to_label(&ctx, &mut view, &mut state, size, &denied, true);
		state.community.stale = true;
		let stale = crate::i18n::translate("server-events-save-stale");
		scroll_to_label(&ctx, &mut view, &mut state, size, &stale, true);
		assert_eq!(view.draft.as_ref().unwrap().name, "Keep this draft");
	}

	#[test]
	fn native_editor_dispatches_once_and_preserves_draft_after_unknown_write() {
		let ctx = egui::Context::default();
		let mut state = test_support::community_state(false);
		let before = state.community.events.as_ref().unwrap().events[0].clone();
		let guild = before.guild;
		let mut editor = Editor::new(guild, Some(before));
		editor.name = "Edited synthetic meetup".into();
		let mut view = CommunityUi {
			guild: Some(guild),
			generation: state.generation,
			draft: Some(editor),
			..Default::default()
		};
		let size = egui::vec2(1000.0, 1100.0);
		for _ in 0..3 {
			assert!(
				frame(&ctx, &mut view, &mut state, size, vec![])
					.0
					.is_empty()
			);
		}
		let text = scroll_to_label(&ctx, &mut view, &mut state, size, "Save event", true);
		let pos = text
			.iter()
			.find(|(s, _)| s == "Save event")
			.expect("native save button")
			.1
			.center();
		assert!(state.community_action_allowed(
			guild,
			&Action::Edit {
				before: view.draft.as_ref().unwrap().before.clone().unwrap(),
				after: view.draft.as_ref().unwrap().draft().unwrap()
			}
		));
		assert!(
			ctx.content_rect().contains(pos),
			"save control outside viewport: {pos:?}"
		);
		let mut writes = vec![];
		for pressed in [true, false] {
			writes.extend(
				frame(
					&ctx,
					&mut view,
					&mut state,
					size,
					vec![
						egui::Event::PointerMoved(pos),
						egui::Event::PointerButton {
							pos,
							button: egui::PointerButton::Primary,
							pressed,
							modifiers: egui::Modifiers::NONE,
						},
					],
				)
				.0,
			);
		}
		assert_eq!(
			writes.len(),
			1,
			"save at {pos:?}; status={} error={:?} pending={} stale={}",
			state.status,
			state.community.error,
			state.community.pending(),
			state.community.stale
		);
		let Command::Community {
			request, action, ..
		} = writes.pop().unwrap()
		else {
			panic!()
		};
		assert!(matches!(*action, Action::Edit { .. }));
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Community(client_core::community::Event {
				guild,
				request,
				result: Err(client_core::auth::Failure::Ambiguous),
			}),
		});
		let (commands, _) = frame(&ctx, &mut view, &mut state, size, vec![]);
		assert!(commands.is_empty());
		assert_eq!(view.draft.as_ref().unwrap().name, "Edited synthetic meetup");
		assert!(view.submitted.is_none());
		assert!(state.community.needs_refresh);
		let error = state.community.error.unwrap();
		let text = scroll_to_label(&ctx, &mut view, &mut state, size, error, false);
		assert!(text.iter().any(|(s, _)| s.contains("unknown")));
	}
	#[test]
	fn event_details_and_editor_render_long_content_in_light_dark_and_narrow_windows() {
		for light in [false, true] {
			for width in [340.0, 1000.0] {
				let ctx = egui::Context::default();
				ctx.set_visuals(if light {
					egui::Visuals::light()
				} else {
					egui::Visuals::dark()
				});
				let mut state = test_support::community_state(false);
				let event = &mut state.community.events.as_mut().unwrap().events[0];
				event.draft.name = "Community ".repeat(10);
				event.draft.description = "Long synthetic community event description. ".repeat(20);
				let event = event.clone();
				let mut view = CommunityUi {
					guild: Some(event.guild),
					generation: state.generation,
					selected: Some(event.id),
					..Default::default()
				};
				let size = egui::vec2(width, 900.0);
				let mut text = vec![];
				for _ in 0..3 {
					let (commands, labels) = frame(&ctx, &mut view, &mut state, size, vec![]);
					assert!(commands.is_empty());
					text = labels;
				}
				assert!(text.iter().any(|(s, _)| s == &event.draft.name));
				assert!(text.iter().all(|(_, r)| r.is_finite()));
				view.draft = Some(Editor::new(event.guild, Some(event)));
				let (_, text) = frame(&ctx, &mut view, &mut state, size, vec![]);
				assert!(text.iter().any(|(s, _)| s == "Edit event"));
				let text = scroll_to_label(&ctx, &mut view, &mut state, size, "Save event", true);
				assert!(text.iter().any(|(s, _)| s == "Save event"));
				assert!(view.draft.is_some());
			}
		}
	}
	#[test]
	fn compact_event_editor_has_visible_actions_and_escape_keeps_the_draft() {
		let ctx = egui::Context::default();
		let mut state = test_support::community_state(false);
		let event = state.community.events.as_ref().unwrap().events[0].clone();
		let mut view = CommunityUi {
			guild: Some(event.guild),
			generation: state.generation,
			draft: Some(Editor::new(event.guild, Some(event))),
			..Default::default()
		};
		view.draft.as_mut().unwrap().name = "Retain my change".into();
		let size = egui::vec2(560.0, 460.0);
		for _ in 0..2 {
			frame(&ctx, &mut view, &mut state, size, vec![]);
		}
		let (commands, text) = frame(&ctx, &mut view, &mut state, size, vec![]);
		for key in [
			"server-events-save",
			"server-events-discard-draft",
			"server-events-refresh",
		] {
			let label = crate::i18n::translate(key);
			assert!(
				text.iter()
					.any(|(s, rect)| s == &label && rect.top() >= 0.0 && rect.bottom() <= size.y),
				"missing visible {label}"
			);
		}
		assert!(commands.is_empty());
		let (commands, _) = frame(
			&ctx,
			&mut view,
			&mut state,
			size,
			vec![egui::Event::Key {
				key: egui::Key::Escape,
				physical_key: None,
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}],
		);
		assert!(commands.is_empty());
		assert!(view.guild.is_none());
		assert_eq!(view.draft.as_ref().unwrap().name, "Retain my change");
	}
	#[test]
	fn native_stage_request_and_cancel_controls_do_not_grant_microphone_consent() {
		let ctx = egui::Context::default();
		let mut state = test_support::community_state(true);
		let mut view = CommunityUi {
			generation: state.generation,
			requested_stage: Some(Id(90)),
			..Default::default()
		};
		let render = |view: &mut CommunityUi, state: &mut State, events| {
			let mut commands = vec![];
			let mut output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(600.0, 900.0),
					)),
					events,
					..Default::default()
				},
				|ui| view.stage_controls(ui, state, Id(90), &mut commands),
			);
			output.textures_delta.clear();
			let mut text = vec![];
			for clipped in output.shapes {
				labels(&clipped.shape, &mut text);
			}
			(commands, text)
		};
		for requested in [true, false] {
			for _ in 0..2 {
				assert!(render(&mut view, &mut state, vec![]).0.is_empty());
			}
			let (_, text) = render(&mut view, &mut state, vec![]);
			let pos = text
				.iter()
				.find(|(s, _)| {
					s == if requested {
						"Request to speak"
					} else {
						"Cancel request"
					}
				})
				.unwrap()
				.1
				.center();
			let mut writes = vec![];
			for pressed in [true, false] {
				writes.extend(
					render(
						&mut view,
						&mut state,
						vec![
							egui::Event::PointerMoved(pos),
							egui::Event::PointerButton {
								pos,
								button: egui::PointerButton::Primary,
								pressed,
								modifiers: egui::Modifiers::NONE,
							},
						],
					)
					.0,
				);
			}
			assert_eq!(writes.len(), 1);
			let Command::Community {
				guild,
				request,
				action,
				..
			} = writes.pop().unwrap()
			else {
				panic!()
			};
			assert!(
				matches!(*action,Action::StageVoice{change:StageVoiceChange::RequestToSpeak(r),..}if r==requested)
			);
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::Community(client_core::community::Event {
					guild,
					request,
					result: Ok(Outcome::StageVoice { channel: Id(90) }),
				}),
			});
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::Voice(client_core::voice::Event::State {
					guild: Some(guild),
					member: None,
					server_muted: false,
					server_deafened: false,
					stage: StageParticipation::Audience { requested },
					request: Some(1),
					channel: Some(Id(90)),
					user: Id(1),
					session: None,
					negotiation_revision: None,
					muted: true,
					deafened: false,
					video: false,
					streaming: false,
				}),
			});
			assert!(!state.can_speak(Id(90)));
			assert!(!state.voice.active.as_ref().unwrap().stage_consent);
		}
	}
	#[test]
	fn community_editor_undo_state_is_bounded_and_retires_with_draft_and_account() {
		let ctx = egui::Context::default();
		let mut state = test_support::community_state(false);
		let guild = Id(10);
		let size = egui::vec2(1000.0, 1000.0);
		let mut view = CommunityUi {
			generation: state.generation,
			guild: Some(guild),
			draft: Some(Editor::new(guild, None)),
			..Default::default()
		};
		for _ in 0..3 {
			frame(&ctx, &mut view, &mut state, size, vec![]);
		}
		let name = view.draft.as_ref().unwrap().inputs.id("name");
		assert!(egui::text_edit::TextEditState::load(&ctx, name).is_some());
		// Name, description, optional location, and three inputs per clock row.
		assert!(ctx.data(|d| d.count::<egui::text_edit::TextEditState>()) <= 9);
		drop(view.draft.take());
		assert!(egui::text_edit::TextEditState::load(&ctx, name).is_none());
		view.draft = Some(Editor::new(guild, None));
		for _ in 0..3 {
			frame(&ctx, &mut view, &mut state, size, vec![]);
		}
		assert!(egui::text_edit::TextEditState::load(&ctx, name).is_some());
		state.generation += 1;
		frame(&ctx, &mut view, &mut state, size, vec![]);
		assert!(view.draft.is_none());
		assert!(egui::text_edit::TextEditState::load(&ctx, name).is_none());
		view.generation = state.generation;
		view.stage_channel = Some(Id(90));
		view.stage_topic = "Private synthetic Stage topic".into();
		for _ in 0..3 {
			frame(&ctx, &mut view, &mut state, size, vec![]);
		}
		let topic = egui::Id::unique(("community-topic", state.generation, Id(90)));
		assert!(egui::text_edit::TextEditState::load(&ctx, topic).is_some());
		drop(view);
		assert!(egui::text_edit::TextEditState::load(&ctx, topic).is_none());
		assert_eq!(ctx.data(|d| d.count::<egui::text_edit::TextEditState>()), 0);
	}
}
