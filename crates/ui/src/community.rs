//! Native event browsing/editing and explicit Stage controls. All drafts stay session-only.
use client_core::{Command, State};
use model::{Id, community::*};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Default)]
pub(super) struct CommunityUi {
	guild: Option<Id>,
	generation: u64,
	selected: Option<Id>,
	draft: Option<Editor>,
	occurrence_draft: Option<OccurrenceEditor>,
	confirm: Option<Action>,
	submitted: Option<u64>,
	link: String,
	link_error: Option<&'static str>,
	pub stage_channel: Option<Id>,
	stage_topic: String,
	stage_event: Option<Id>,
	stage_draft_channel: Option<Id>,
	stage_dirty: bool,
	stage_submitted: Option<u64>,
	requested_stage: Option<Id>,
	audience_page: usize,
	end_stage_confirm: bool,
}
struct Editor {
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
	error: Option<&'static str>,
}

struct OccurrenceEditor {
	before: ScheduledEvent,
	original: i64,
	start: String,
	end: String,
	canceled: bool,
	error: Option<&'static str>,
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
			original,
			start: format_time(exception.and_then(|e| e.start).unwrap_or(original)),
			end: exception
				.and_then(|e| e.end)
				.or(default_end)
				.map(format_time)
				.unwrap_or_default(),
			canceled: exception.is_some_and(|e| e.canceled),
			before,
			error: None,
		}
	}
	fn action(&self) -> Result<Action, &'static str> {
		let start = parse_time(&self.start)?;
		let end = if self.end.trim().is_empty() {
			None
		} else {
			Some(parse_time(&self.end)?)
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
			error: None,
		}
	}
	fn draft(&self) -> Result<EventDraft, &'static str> {
		let start = parse_time(&self.start)?;
		let end = if self.end.trim().is_empty() {
			None
		} else {
			Some(parse_time(&self.end)?)
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
	OffsetDateTime::parse(value.trim(), &Rfc3339)
		.map(|t| t.unix_timestamp())
		.map_err(|_| "Use YYYY-MM-DDTHH:MM:SS±HH:MM, including the UTC offset")
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
		if self.stage_dirty && self.stage_draft_channel.is_some_and(|c| c != channel) {
			self.stage_channel = self.stage_draft_channel;
			state.status =
				"A topic draft for another Stage is preserved; discard it there to start another";
			return false;
		}
		self.stage_channel = Some(channel);
		if !self.stage_dirty {
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
		if self
			.submitted
			.is_some_and(|r| state.community.completed == r)
		{
			self.submitted = None;
			self.draft = None;
			self.occurrence_draft = None;
			self.confirm = None;
		}
		if !state.community.pending() && state.community.error.is_some() {
			self.submitted = None;
		}
		let mut open = true;
		egui::Window::new("Server events")
			.id(egui::Id::unique("community-events"))
			.open(&mut open)
			.default_width(620.0)
			.min_width(240.0)
			.max_width((ctx.content_rect().width() - 32.0).max(240.0))
			.max_height((ctx.content_rect().height() - 64.0).max(180.0))
			.resizable(true)
			.show(ctx, |ui| {
				ui.set_max_width(ui.available_width());
				egui::ScrollArea::vertical().show(ui, |ui| {
					if let Some(name) = state.guild(guild).map(|g| g.name.as_str()) {
						ui.label(crate::design::semibold(ui, name, 16.0));
					}
					ui.horizontal_wrapped(|ui| {
						if ui
							.add_enabled(!state.community.pending(), egui::Button::new("Refresh"))
							.clicked()
						{
							submit(state, guild, Action::ListEvents, commands);
						}
						if ui
							.add_enabled(
								!state.community.pending(),
								egui::Button::new(if self.draft.is_some() {
									"Continue draft"
								} else {
									"Create event"
								}),
							)
							.clicked()
						{
							if self.draft.is_none() {
								self.draft = Some(Editor::new(guild, None));
							}
							self.selected = None;
						}
						if state.community.pending() {
							ui.spinner();
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
					if let Some(action) = self.confirm.clone() {
						let allowed=state.community_action_allowed(guild,&action) && !state.community.needs_refresh && if action.stage_channel().is_some(){!state.community.stage_stale}else{!state.community.stale};
						ui.separator();
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
						ui.horizontal_wrapped(|ui| {
							if ui
								.add_enabled(
										!state.community.pending() && allowed,
									egui::Button::new("Confirm"),
								)
								.clicked()
							{
								self.submitted = submit(state, guild, action, commands);
							}
							if ui.button("Back").clicked() {
								self.confirm = None;
							}
						});
						if !allowed {ui.small("The event or its permissions changed. Refresh, go back and review the current event before confirming.");}
						return;
					}
					if self.occurrence_draft.is_some() {
						self.occurrence_editor(ui, state, guild);
						return;
					}
					if self.draft.is_some() {
						self.editor(ui, state, guild, commands);
						return;
					}
					ui.separator();
					ui.horizontal_wrapped(|ui| {
						ui.label("Event link");
						ui.add(
							egui::TextEdit::singleline(&mut self.link)
								.char_limit(128)
								.hint_text("https://discord.com/events/server/event"),
						);
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
							if ui.button("View event").clicked() {
								self.selected = Some(event.id);
							}
						});
					}
				});
			});
		if !open {
			self.guild = None;
			state.close_events();
			self.confirm = None;
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
					self.draft = Some(Editor::new(guild, Some(event.clone())));
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
										.add_enabled(ready, egui::Button::new("Edit occurrence"))
										.clicked()
									{
										self.occurrence_draft =
											Some(OccurrenceEditor::new(event.clone(), original));
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
		ui.label("Start · RFC3339 with UTC offset");
		ui.add(
			egui::TextEdit::singleline(&mut editor.start)
				.char_limit(64)
				.desired_width(f32::INFINITY),
		);
		ui.label("End · RFC3339 with UTC offset");
		ui.add(
			egui::TextEdit::singleline(&mut editor.end)
				.char_limit(64)
				.desired_width(f32::INFINITY),
		);
		ui.checkbox(&mut editor.canceled, "Cancel this occurrence");
		if let Some(error) = editor.error {
			ui.colored_label(crate::design::palette(ui).danger, error);
		}
		let action = editor.action();
		let allowed = action
			.as_ref()
			.is_ok_and(|a| state.community_action_allowed(guild, a));
		let mut discard = false;
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					!state.community.pending() && !state.community.stale && allowed,
					egui::Button::new("Review occurrence change"),
				)
				.clicked()
			{
				match action {
					Ok(action) => self.confirm = Some(action),
					Err(error) => editor.error = Some(error),
				}
			}
			if ui.button("Discard occurrence draft").clicked() {
				discard = true;
			}
		});
		if discard {
			self.occurrence_draft = None;
		}
	}
	fn editor(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		commands: &mut Vec<Command>,
	) {
		let Some(editor) = &mut self.draft else {
			return;
		};
		if editor.guild != guild {
			ui.label(
				"An event draft for another server is preserved. Reopen that server to continue it.",
			);
			if ui.button("Discard saved draft").clicked() {
				self.draft = None;
			}
			return;
		}
		ui.separator();
		ui.heading(if editor.before.is_some() {
			"Edit event"
		} else {
			"Create event"
		});
		ui.label("Name");
		ui.add(
			egui::TextEdit::singleline(&mut editor.name)
				.char_limit(100)
				.desired_width(f32::INFINITY),
		);
		ui.label("Description");
		ui.add(
			egui::TextEdit::multiline(&mut editor.description)
				.char_limit(1000)
				.desired_rows(3)
				.desired_width(f32::INFINITY),
		);
		ui.horizontal_wrapped(|ui| {
			for (kind, label) in [(1, "Stage"), (2, "Voice"), (3, "Somewhere else")] {
				ui.selectable_value(&mut editor.entity_type, kind, label);
			}
		});
		if editor.entity_type == 3 {
			ui.label("Location");
			ui.add(
				egui::TextEdit::singleline(&mut editor.location)
					.char_limit(100)
					.desired_width(f32::INFINITY),
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
					}) {
						ui.selectable_value(&mut editor.channel, Some(c.id), &c.name);
					}
				});
		}
		ui.label("Start · include UTC offset (YYYY-MM-DDTHH:MM:SS+02:00)");
		ui.add(
			egui::TextEdit::singleline(&mut editor.start)
				.char_limit(64)
				.desired_width(f32::INFINITY),
		);
		if let Ok(start) = parse_time(&editor.start) {
			ui.small(format!("Your time: {}", time_label(start)));
		}
		ui.label("End · required for external events");
		ui.add(
			egui::TextEdit::singleline(&mut editor.end)
				.char_limit(64)
				.desired_width(f32::INFINITY),
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
			ui.small("Repetition uses the UTC weekday/time of the first event. Local clock times can change at daylight saving transitions.");
		}
		if let Some(error) = editor.error {
			ui.colored_label(crate::design::palette(ui).danger, error);
		}
		let built = editor.draft();
		let allowed = built.as_ref().is_ok_and(|d| {
			if let Some(before) = &editor.before {
				state.can_manage_event(before)
					&& state.community_action_allowed(
						guild,
						&Action::Edit {
							before: before.clone(),
							after: d.clone(),
						},
					)
			} else {
				state.can_create_event(guild, d)
			}
		});
		let mut discard = false;
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					!state.community.pending() && !state.community.stale && allowed,
					egui::Button::new("Save event"),
				)
				.clicked()
			{
				match built {
					Ok(draft) => {
						let action = if let Some(before) = &editor.before {
							Action::Edit {
								before: before.clone(),
								after: draft,
							}
						} else {
							Action::Create(draft)
						};
						self.submitted = submit(state, guild, action, commands);
					}
					Err(error) => editor.error = Some(error),
				}
			}
			if ui.button("Discard draft").clicked() {
				discard = true;
			}
		});
		if !allowed {
			ui.small("Complete the fields and check your current event permissions before saving.");
		}
		if discard {
			self.draft = None;
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
		ui.heading(topic.as_deref().unwrap_or("Stage channel"));
		ui.small("Stage audio uses transport encryption. Discord can process this audio; there is no end-to-end privacy code.");
		let ready = !state.community.pending()
			&& !state.community.stage_stale
			&& !state.community.needs_refresh;
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					!state.community.pending(),
					egui::Button::new("Refresh Stage"),
				)
				.clicked()
			{
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
				&& ui
					.add_enabled(
						ready,
						egui::Button::new(if topic.is_some() {
							"Edit topic"
						} else {
							"Start Stage"
						}),
					)
					.clicked()
			{
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
				ui.label("Connecting to the audience…");
				return;
			}
			let mut action = None;
			ui.horizontal_wrapped(|ui| {
				match stage {
					StageParticipation::Speaker => {
						ui.label("You are a speaker");
						if ui
							.add_enabled(ready, egui::Button::new("Move to audience"))
							.clicked()
						{
							action = Some(StageVoiceChange::Audience);
						}
					}
					StageParticipation::Invited => {
						ui.label("You can speak after accepting. Your microphone is off.");
						if ui
							.add_enabled(ready, egui::Button::new("Accept invitation"))
							.clicked()
						{
							action = Some(StageVoiceChange::AcceptInvitation);
						}
						if ui
							.add_enabled(ready, egui::Button::new("Decline"))
							.clicked()
						{
							action = Some(StageVoiceChange::Audience);
						}
					}
					StageParticipation::Audience { requested } => {
						ui.label(if requested {
							"In audience · request to speak sent"
						} else {
							"In audience · microphone off"
						});
						if ui
							.add_enabled(
								ready
									&& (requested
										|| state.permission(
											channel,
											model::permissions::REQUEST_TO_SPEAK,
										) == Some(true)),
								egui::Button::new(if requested {
									"Cancel request"
								} else {
									"Request to speak"
								}),
							)
							.clicked()
						{
							action = Some(StageVoiceChange::RequestToSpeak(!requested));
						}
					}
					StageParticipation::Unknown => {
						ui.label(
							"Audience mode · waiting for current Stage state. Microphone off.",
						);
					}
				}
				if stage != StageParticipation::Speaker
					&& state.permission(channel, model::permissions::MUTE_MEMBERS) == Some(true)
					&& ui
						.add_enabled(ready, egui::Button::new("Become speaker"))
						.clicked()
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
			egui::CollapsingHeader::new(format!("Stage members ({total})")).show(ui, |ui| {
				ui.horizontal_wrapped(|ui| {
					if ui
						.add_enabled(self.audience_page > 0, egui::Button::new("Previous"))
						.clicked()
					{
						self.audience_page = self.audience_page.saturating_sub(1);
					}
					ui.label(format!("Page {} of {pages}", self.audience_page + 1));
					if ui
						.add_enabled(self.audience_page + 1 < pages, egui::Button::new("Next"))
						.clicked()
					{
						self.audience_page += 1;
					}
				});
				egui::ScrollArea::vertical()
					.max_height(140.0)
					.show(ui, |ui| {
						for (p, name) in participants {
							ui.horizontal_wrapped(|ui| {
								ui.label(name);
								ui.small(match p.stage {
									StageParticipation::Speaker => "Speaker",
									StageParticipation::Invited => "Invitation pending",
									StageParticipation::Audience { requested: true } => {
										"Requested to speak"
									}
									_ => "Audience",
								});
								let speaker = matches!(
									p.stage,
									StageParticipation::Speaker | StageParticipation::Invited
								);
								if moderator
									&& ui
										.add_enabled(
											ready,
											egui::Button::new(if speaker {
												"Move to audience"
											} else {
												"Invite to speak"
											}),
										)
										.clicked()
								{
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
			ui.small("Join this Stage as an audience member to listen or request to speak.");
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
		egui::Window::new(if exists { "Stage topic" } else { "Start Stage" })
			.id(egui::Id::unique("stage-topic-editor"))
			.open(&mut open)
			.default_width(420.0)
			.min_width(240.0)
			.max_width((ctx.content_rect().width() - 32.0).max(240.0))
			.show(ctx, |ui| {
				ui.label("Topic");
				if let Some(event) = self.stage_event.and_then(|id| {
					state
						.community
						.events
						.as_ref()
						.and_then(|p| p.events.iter().find(|e| e.id == id))
				}) {
					ui.label(format!("Scheduled event: {}", event.draft.name));
				}
				if ui
					.add(
						egui::TextEdit::multiline(&mut self.stage_topic)
							.char_limit(120)
							.desired_rows(2)
							.desired_width(f32::INFINITY),
					)
					.changed()
				{
					self.stage_dirty = true;
				}
				if let Some(error) = state.community.error {
					ui.colored_label(crate::design::palette(ui).danger, error);
				}
				if event_conflict {
					ui.label(
						"A Stage is already active. End it before starting this scheduled event.",
					);
				}
				if self.end_stage_confirm {
					ui.label("End this Stage for everyone? This closes the Stage instance.");
					ui.horizontal_wrapped(|ui| {
						if ui
							.add_enabled(
								!state.community.pending()
									&& !state.community.stage_stale
									&& !state.community.needs_refresh
									&& state.community_action_allowed(
										guild,
										&Action::EndStage(channel),
									),
								egui::Button::new("End Stage"),
							)
							.clicked()
						{
							self.stage_submitted =
								submit(state, guild, Action::EndStage(channel), commands);
							self.end_stage_confirm = false;
						}
						if ui.button("Back").clicked() {
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
					if ui
						.add_enabled(
							!state.community.pending()
								&& !state.community.stage_stale
								&& !state.community.needs_refresh
								&& !event_conflict && allowed,
							egui::Button::new(if exists { "Save topic" } else { "Start Stage" }),
						)
						.clicked()
					{
						self.stage_dirty = true;
						self.stage_submitted = submit(state, guild, action, commands);
					}
					if exists
						&& ui
							.add_enabled(
								!state.community.pending(),
								egui::Button::new("End Stage…"),
							)
							.clicked()
					{
						self.end_stage_confirm = true;
					}
				});
				if self.stage_dirty
					&& !state.community.pending()
					&& ui.button("Discard topic draft").clicked()
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
				}
				ui.small("Starting a Stage does not enable your microphone or notify @everyone.");
			});
		if !open {
			self.stage_channel = None;
		}
	}
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
}
