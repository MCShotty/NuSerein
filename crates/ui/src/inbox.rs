use crate::{MessagingUi, design, dialog, i18n};
use client_core::{Command, State, inbox::Action};
use model::Id;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Tab {
	#[default]
	Unread,
	Mentions,
	Saved,
}
#[derive(Default)]
pub(super) struct Controls {
	pub tab: Tab,
	filter: String,
	include_muted: bool,
	guild_only: bool,
	generation: Option<u64>,
	custom_reminder: String,
}
impl Controls {
	fn allows(&self, state: &State, channel: &model::Channel, guild: Option<Id>) -> bool {
		state.can_read_history(channel.id)
			&& (!self.guild_only || channel.guild == guild)
			&& (self.include_muted
				|| !if channel.guild.is_some() {
					state.muted_for_guild_rail(channel)
				} else {
					state.dm_muted(channel.id) == Some(true)
				}) && (self.filter.is_empty()
			|| channel
				.name
				.to_lowercase()
				.contains(&self.filter.to_lowercase())
			|| channel
				.guild
				.and_then(|id| state.guild(id))
				.is_some_and(|row| {
					row.name
						.to_lowercase()
						.contains(&self.filter.to_lowercase())
				}))
	}
}
fn custom_reminder(value: &str, now: i128) -> Option<i128> {
	if value.len() != 16 {
		return None;
	}
	let format =
		time::format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]")
			.ok()?;
	let at = time::PrimitiveDateTime::parse(value, &format)
		.ok()?
		.assume_utc()
		.unix_timestamp_nanos();
	(at > now && at - now <= 366 * 86400 * 1_000_000_000_i128).then_some(at)
}

fn preview(ui: &mut egui::Ui, message: &model::Message) {
	ui.weak(&message.author.name);
	if crate::embeds::has_spoilers(message) {
		ui.label(i18n::translate("inbox-spoiler"));
	} else {
		let mut text: String = message.content.chars().take(240).collect();
		if text.len() < message.content.len() {
			text.push('…');
		}
		ui.label(text);
	}
}
impl MessagingUi {
	pub(super) fn mentions_inbox(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		if !self.inbox_open {
			return;
		}
		if self.inbox_controls.generation != Some(state.generation) {
			self.inbox_controls = Controls {
				generation: Some(state.generation),
				tab: self.inbox_controls.tab,
				..Default::default()
			};
		}
		// A closed panel can outlive its accepted write. Load the requested tab once
		// that write settles; failed reads wait for an explicit retry.
		if !state.inbox.loading && state.inbox.error.is_none() {
			let command = match self.inbox_controls.tab {
				Tab::Mentions if !state.inbox.loaded => state.request_mentions(None),
				Tab::Saved if !state.inbox.saved_loaded => {
					state.request_inbox_action(Action::Saved)
				}
				_ => None,
			};
			commands.extend(command);
		}
		let mut load = None;
		let mut action = None;
		let mut open = None;
		let mut mark_read = None;
		let mut changed_tab = false;
		let response = dialog::Dialog::new("mentions-inbox", i18n::translate("inbox-title"))
			.subtitle(i18n::translate("inbox-description"))
			.width(620.0)
			.show(ctx, |body| {
				body.content(|ui| {
					ui.horizontal(|ui| {
						for (tab, label) in [
							(Tab::Unread, "inbox-unread"),
							(Tab::Mentions, "inbox-mentions"),
							(Tab::Saved, "inbox-saved"),
						] {
							changed_tab |= ui
								.selectable_value(
									&mut self.inbox_controls.tab,
									tab,
									i18n::translate(label),
								)
								.changed();
						}
					});
					ui.add(
						egui::TextEdit::singleline(&mut self.inbox_controls.filter)
							.char_limit(128)
							.hint_text(i18n::translate("inbox-filter")),
					);
					ui.horizontal_wrapped(|ui| {
						ui.checkbox(
							&mut self.inbox_controls.include_muted,
							i18n::translate("inbox-include-muted"),
						);
						ui.checkbox(
							&mut self.inbox_controls.guild_only,
							i18n::translate("inbox-current-place"),
						);
					});
				});
				body.scroll(270.0, |ui| {
					let colors = design::palette(ui);
					if state.inbox.loading {
						ui.spinner();
						ui.label(i18n::translate("inbox-loading"));
					}
					if let Some(error) = state.inbox.error {
						ui.colored_label(colors.danger, error);
					}
					let mut shown = 0;
					match self.inbox_controls.tab {
						Tab::Unread => {
							let mut rows: Vec<_> = state
								.channels
								.iter()
								.filter(|row| {
									self.inbox_controls.allows(state, row, self.guild)
										&& state.channel_unread(row) == Some(true)
								})
								.collect();
							rows.sort_unstable_by_key(|row| {
								std::cmp::Reverse((row.last_message, row.id))
							});
							let total = rows.len();
							for channel in rows.into_iter().take(100) {
								shown += 1;
								place(ui, state, channel);
								ui.horizontal_wrapped(|ui| {
									ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
									if ui
										.button(i18n::translate("inbox-open-conversation"))
										.clicked()
									{
										open = Some((channel.guild, channel.id, None));
									}
									if ui
										.add_enabled(
											state.can_mark_channel_read(channel.id),
											egui::Button::new(i18n::translate("inbox-mark-read")),
										)
										.clicked()
									{
										mark_read = Some(channel.id);
									}
								});
								ui.separator();
							}
							if total > shown {
								ui.weak(i18n::translate("inbox-more-unread"));
							}
						}
						Tab::Mentions => {
							for message in &state.inbox.messages {
								let Some(channel) = state.channel(message.channel).filter(|row| {
									self.inbox_controls.allows(state, row, self.guild)
								}) else {
									continue;
								};
								shown += 1;
								place(ui, state, channel);
								preview(ui, message);
								ui.horizontal_wrapped(|ui| {
									ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
									if ui.button(i18n::translate("inbox-open-message")).clicked() {
										open = Some((channel.guild, channel.id, Some(message.id)));
									}
									if ui
										.add_enabled(
											!state.inbox.loading,
											egui::Button::new(i18n::translate("inbox-dismiss")),
										)
										.clicked()
									{
										action = Some(Action::Dismiss {
											channel: channel.id,
											message: message.id,
										});
									}
									if ui
										.add_enabled(
											!state.inbox.loading,
											egui::Button::new(i18n::translate("inbox-bookmark")),
										)
										.clicked()
									{
										action = Some(Action::Save {
											channel: channel.id,
											message: message.id,
											due_at: None,
										});
									}
								});
								ui.separator();
							}
						}
						Tab::Saved => {
							for saved in &state.inbox.saved {
								// A stale saved body must never reveal a now-inaccessible channel.
								if let Some(channel) = state.channel(saved.channel).filter(|row| {
									self.inbox_controls.allows(state, row, self.guild)
								}) {
									shown += 1;
									place(ui, state, channel);
									if let Some(message) = &saved.body {
										preview(ui, message);
									} else {
										ui.weak(i18n::translate("inbox-message-unavailable"));
									}
									if let Some(due) = saved.due_at.and_then(|at| {
										crate::local_time::discord_timestamp(
											(at / 1_000_000_000) as i64,
											b'f',
										)
									}) {
										ui.weak(due);
									}
									ui.horizontal_wrapped(|ui| {
										ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
										if ui
											.button(i18n::translate("inbox-open-message"))
											.clicked()
										{
											open = Some((
												channel.guild,
												channel.id,
												Some(saved.message),
											));
										}
										if ui
											.add_enabled(
												!state.inbox.loading,
												egui::Button::new(i18n::translate("inbox-unsave")),
											)
											.clicked()
										{
											action = Some(Action::Unsave {
												channel: saved.channel,
												message: saved.message,
											});
										}
									});
									ui.add_enabled_ui(!state.inbox.loading, |ui| {
										ui.menu_button(i18n::translate("inbox-remind"), |ui| {
											for (seconds, label) in [
												(3600, "inbox-in-hour"),
												(86400, "inbox-in-day"),
												(604800, "inbox-in-week"),
											] {
												if ui.button(i18n::translate(label)).clicked() {
													let due_at = Some(
														(time::OffsetDateTime::now_utc()
															.unix_timestamp() as i128 + seconds)
															* 1_000_000_000,
													);
													action = Some(Action::Save {
														channel: saved.channel,
														message: saved.message,
														due_at,
													});
													ui.close();
												}
											}
											ui.separator();
											ui.label(i18n::translate("inbox-custom-reminder"));
											ui.add(
												egui::TextEdit::singleline(
													&mut self.inbox_controls.custom_reminder,
												)
												.char_limit(16)
												.hint_text("YYYY-MM-DD HH:MM"),
											);
											let at = custom_reminder(
												&self.inbox_controls.custom_reminder,
												time::OffsetDateTime::now_utc()
													.unix_timestamp_nanos(),
											);
											if ui
												.add_enabled(
													at.is_some(),
													egui::Button::new(i18n::translate(
														"inbox-set-reminder",
													)),
												)
												.clicked()
											{
												action = Some(Action::Save {
													channel: saved.channel,
													message: saved.message,
													due_at: at,
												});
												self.inbox_controls.custom_reminder.clear();
												ui.close();
											}
											if saved.due_at.is_some()
												&& ui
													.button(i18n::translate("inbox-clear-reminder"))
													.clicked()
											{
												action = Some(Action::Save {
													channel: saved.channel,
													message: saved.message,
													due_at: None,
												});
												ui.close();
											}
										});
									});
									ui.separator();
								} else if self.inbox_controls.filter.is_empty()
									&& !self.inbox_controls.guild_only
								{
									shown += 1;
									ui.weak(i18n::translate("inbox-message-unavailable"));
									if ui
										.add_enabled(
											!state.inbox.loading,
											egui::Button::new(i18n::translate("inbox-unsave")),
										)
										.clicked()
									{
										action = Some(Action::Unsave {
											channel: saved.channel,
											message: saved.message,
										});
									}
								}
							}
						}
					}
					if shown == 0 && !state.inbox.loading && state.inbox.error.is_none() {
						ui.label(i18n::translate(
							if self.inbox_controls.tab == Tab::Unread
								|| state.inbox.loaded || state.inbox.saved_loaded
							{
								"inbox-empty"
							} else {
								"inbox-refresh-needed"
							},
						));
					}
				});
				if self.inbox_controls.tab != Tab::Unread || state.inbox.stale {
					body.footer(|ui| {
						if state.inbox.stale {
							ui.weak(i18n::translate("inbox-refresh-needed"));
						}
						let enabled = state.can_load_mentions() && !state.inbox.loading;
						if self.inbox_controls.tab != Tab::Unread
							&& ui
								.add_enabled(
									enabled,
									egui::Button::new(i18n::translate("inbox-refresh")),
								)
								.clicked()
						{
							if self.inbox_controls.tab == Tab::Saved {
								action = Some(Action::Saved);
							} else {
								load = Some(None);
							}
						}
						if self.inbox_controls.tab == Tab::Mentions
							&& ui
								.add_enabled(
									enabled && state.inbox.next.is_some(),
									egui::Button::new(i18n::translate("inbox-older")),
								)
								.clicked()
						{
							load = Some(state.inbox.next);
						}
						if self.inbox_controls.tab == Tab::Saved {
							ui.weak(i18n::translate("inbox-service-availability"));
						}
					});
				}
			});
		if changed_tab {
			commands.push(state.clear_mentions());
			match self.inbox_controls.tab {
				Tab::Unread => {}
				Tab::Mentions => load = Some(None),
				Tab::Saved => action = Some(Action::Saved),
			}
		}
		if let Some((guild, channel, message)) = open {
			let unread = self.inbox_controls.tab == Tab::Unread;
			let result = if unread {
				state.open_channel_unread(guild, channel)
			} else {
				state.open_chat_link(guild, channel, message)
			};
			match result {
				Ok(command) => {
					if unread {
						self.timeline.browse_away();
					}
					commands.extend(command);
					self.guild = guild;
					self.search.open = false;
					self.inbox_open = false;
				}
				Err(error) => state.status = error,
			}
		}
		if let Some(channel) = mark_read
			&& let Some(command) = state.prepare_mark_channel_read(channel)
		{
			commands.push(command);
		}
		if response.close || !self.inbox_open {
			self.inbox_open = false;
			commands.push(state.clear_mentions());
		} else if let Some(action) = action
			&& let Some(command) = state.request_inbox_action(action)
		{
			commands.push(command);
		} else if let Some(before) = load
			&& let Some(command) = state.request_mentions(before)
		{
			commands.push(command);
		}
	}
}
fn place(ui: &mut egui::Ui, state: &State, channel: &model::Channel) {
	let place = channel
		.guild
		.and_then(|guild| state.guild(guild))
		.map_or_else(
			|| channel.name.clone(),
			|guild| format!("{} · #{}", guild.name, channel.name),
		);
	let colors = design::palette(ui);
	ui.label(design::semibold(ui, &place, 14.0).color(colors.text_strong));
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::Id;

	#[test]
	fn custom_reminders_reject_invalid_dates_and_past_deadlines() {
		let now = time::OffsetDateTime::parse(
			"2026-10-17T12:00:00Z",
			&time::format_description::well_known::Rfc3339,
		)
		.unwrap()
		.unix_timestamp_nanos();
		assert_eq!(
			custom_reminder("2026-10-18 12:00", now),
			Some(now + 86400 * 1_000_000_000)
		);
		for text in [
			"2026-10-17 12:00",
			"2026-02-30 12:00",
			"2028-10-17 12:00",
			"2026-10-18 25:00",
			"2026-10-18 12:00 extra",
		] {
			assert_eq!(custom_reminder(text, now), None);
		}
	}

	#[test]
	fn unread_tab_filters_without_acknowledging_or_replacing_drafts() {
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut state = test_support::demo_state();
		let channel = state
			.channels
			.iter()
			.find(|row| state.can_read_history(row.id))
			.unwrap()
			.clone();
		state
			.drafts
			.insert(channel.id, "Keep draft while reading Inbox".into());
		let controls = Controls {
			include_muted: true,
			..Default::default()
		};
		assert!(controls.allows(&state, &channel, channel.guild));
		let controls = Controls {
			filter: "nonexistent synthetic conversation".into(),
			..controls
		};
		assert!(!controls.allows(&state, &channel, channel.guild));
		let mut view = MessagingUi {
			inbox_open: true,
			inbox_controls: Controls {
				tab: Tab::Unread,
				..Default::default()
			},
			..Default::default()
		};
		let selected = state.selected;
		let before = state.read_marker(channel.id);
		let mut commands = Vec::new();
		ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(760.0, 520.0),
				)),
				..Default::default()
			},
			|_| view.mentions_inbox(&ctx, &mut state, &mut commands),
		)
		.drop_without_applying_deltas();
		assert!(
			commands.is_empty(),
			"opening Inbox must not acknowledge or load history"
		);
		assert_eq!(state.read_marker(channel.id), before);
		assert_eq!(state.selected, selected);
		assert_eq!(state.drafts[&channel.id], "Keep draft while reading Inbox");
	}

	#[test]
	fn inbox_over_live_chat_never_automatically_acknowledges() {
		for (width, dark) in [(760.0, false), (1120.0, true)] {
			let ctx = egui::Context::default();
			crate::design::apply(&ctx);
			ctx.set_visuals(if dark {
				egui::Visuals::dark()
			} else {
				egui::Visuals::light()
			});
			let mut state = test_support::demo_state();
			let channel = state.selected.unwrap();
			let latest = state.timeline.iter().last().unwrap().id;
			state
				.channels
				.iter_mut()
				.find(|row| row.id == channel)
				.unwrap()
				.last_message = Some(latest);
			state
				.apply_read_state(client_core::read_state::Event::Snapshot {
					entries: Some(vec![(channel, Some(Id(1)), 1)]),
					version: Some(1),
					partial: false,
				})
				.unwrap();
			state.drafts.insert(channel, "Draft behind Inbox".into());
			let marker = state.read_marker(channel);
			let drafts = state.drafts.clone();
			let mut view = MessagingUi {
				inbox_open: true,
				..Default::default()
			};
			for _ in 0..4 {
				let mut commands = Vec::new();
				ctx.run_ui(
					egui::RawInput {
						focused: true,
						screen_rect: Some(egui::Rect::from_min_size(
							egui::Pos2::ZERO,
							egui::vec2(width, 720.0),
						)),
						..Default::default()
					},
					|ui| commands = view.show(ui, &mut state),
				)
				.drop_without_applying_deltas();
				assert!(
					!commands.iter().any(|command| matches!(
						command,
						Command::MarkRead { .. }
							| Command::MarkGuildRead { .. }
							| Command::MarkAllRead { .. }
							| Command::Send { .. }
							| Command::Edit { .. }
					)),
					"Inbox must isolate the live chat below it"
				);
			}
			assert_eq!(state.read_marker(channel), marker);
			assert_eq!(state.drafts, drafts);
		}
	}

	#[test]
	fn saved_entries_retire_inaccessible_bodies_but_can_be_removed() {
		use client_core::inbox::{Action, Outcome};
		let mut state = test_support::demo_state();
		let Command::InboxAction { request, .. } =
			state.request_inbox_action(Action::Saved).unwrap()
		else {
			panic!()
		};
		let row = model::inbox::SavedMessage {
			channel: Id(9999),
			message: Id(80),
			guild: None,
			due_at: None,
			body: Some(test_support::message(80, Id(9999))),
		};
		state.apply_inbox_action(request, Ok(Outcome::Saved(vec![row])));
		assert_eq!(state.inbox.saved.len(), 1);
		assert!(state.inbox.saved[0].body.is_none());
		assert!(state.inbox_action_allowed(&Action::Unsave {
			channel: Id(9999),
			message: Id(80)
		}));
		assert!(!state.inbox_action_allowed(&Action::Save {
			channel: Id(9999),
			message: Id(80),
			due_at: None
		}));
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::InboxInvalidated,
		});
		assert_eq!(state.inbox.saved.len(), 1);
		assert!(state.inbox.saved[0].body.is_none());
		assert!(state.inbox.stale && state.inbox.error.is_none());
	}

	#[test]
	fn saved_writes_are_explicit_correlated_and_preserve_drafts_on_failure() {
		use client_core::inbox::{Action, Outcome};
		let mut state = test_support::demo_state();
		let message = state.timeline.iter().next().unwrap().clone();
		state
			.drafts
			.insert(message.channel, "Keep this draft".into());
		let action = Action::Save {
			channel: message.channel,
			message: message.id,
			due_at: None,
		};
		let Command::InboxAction { request, .. } =
			state.request_inbox_action(action.clone()).unwrap()
		else {
			panic!()
		};
		assert!(state.request_inbox_action(action.clone()).is_none());
		state.apply_inbox_action(request + 1, Err(client_core::auth::Failure::Network));
		assert!(state.inbox.loading);
		state.apply_inbox_action(request, Err(client_core::auth::Failure::Ambiguous));
		assert!(state.inbox.error.is_some());
		assert_eq!(state.drafts[&message.channel], "Keep this draft");
		let Command::InboxAction { request, .. } =
			state.request_inbox_action(action.clone()).unwrap()
		else {
			panic!()
		};
		let saved = model::inbox::SavedMessage {
			channel: message.channel,
			message: message.id,
			guild: state
				.channel(message.channel)
				.and_then(|channel| channel.guild),
			body: Some(message.clone()),
			due_at: None,
		};
		state.apply_inbox_action(request, Ok(Outcome::SavedOne(Box::new(saved))));
		assert_eq!(state.inbox.saved.len(), 1);
		let Command::InboxAction { request, .. } = state
			.request_inbox_action(Action::Unsave {
				channel: message.channel,
				message: message.id,
			})
			.unwrap()
		else {
			panic!()
		};
		state.clear_mentions();
		state.apply_inbox_action(request, Ok(Outcome::Done));
		assert!(state.inbox.saved.is_empty() && !state.inbox.loading);
		assert!(!state.inbox_action_allowed(&Action::Save {
			channel: Id(99999),
			message: message.id,
			due_at: None
		}));
	}
	#[test]
	fn service_echoes_and_closing_the_inbox_do_not_retire_accepted_writes() {
		use client_core::inbox::{Action, Outcome};
		let mut state = test_support::demo_state();
		let message = state.timeline.iter().next().unwrap().clone();
		let saved = model::inbox::SavedMessage {
			channel: message.channel,
			message: message.id,
			guild: state
				.channel(message.channel)
				.and_then(|channel| channel.guild),
			body: Some(message.clone()),
			due_at: None,
		};
		let echo = |state: &mut State| {
			state.apply(client_core::Envelope {
				generation: state.generation,
				event: client_core::Event::InboxInvalidated,
			})
		};
		let action = Action::Save {
			channel: message.channel,
			message: message.id,
			due_at: None,
		};
		let Some(Command::InboxAction { request, .. }) = state.request_inbox_action(action.clone())
		else {
			panic!()
		};
		echo(&mut state);
		assert!(state.inbox.loading && state.inbox.stale);
		assert_eq!(state.inbox.request, request);
		state.apply_inbox_action(request, Ok(Outcome::SavedOne(Box::new(saved.clone()))));
		assert_eq!(state.status, "Inbox updated");
		echo(&mut state);
		assert_eq!(state.inbox.saved.len(), 1);
		assert!(state.inbox.error.is_none());
		let Some(Command::InboxAction { request, .. }) = state.request_inbox_action(action) else {
			panic!()
		};
		state.clear_mentions();
		assert!(state.inbox.loading && state.inbox.detached_write);
		assert!(state.request_mentions(None).is_none());
		state.demo = false;
		state.auth = client_core::auth::AuthState::Authenticated;
		state.gateway_connected = false;
		state.apply_inbox_action(request, Ok(Outcome::SavedOne(Box::new(saved))));
		assert!(!state.inbox.loading);
		assert!(
			state.inbox.saved.is_empty(),
			"retired message content stays retired"
		);
		assert_eq!(state.status, "Inbox updated");
		state.demo = true;
		assert!(state.request_mentions(None).is_some());
		state.clear_mentions();
		let Some(Command::InboxAction { request, .. }) = state.request_inbox_action(Action::Save {
			channel: message.channel,
			message: message.id,
			due_at: None,
		}) else {
			panic!()
		};
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Failure(client_core::auth::Failure::Network),
		});
		assert!(!state.inbox.loading && state.inbox.operation.is_none());
		assert_eq!(
			state.inbox.error,
			Some("Inbox outcome unknown; refresh before retrying")
		);
		state.apply_inbox_action(request, Ok(Outcome::Done));
		assert!(state.inbox.saved.is_empty());
	}

	#[test]
	fn saved_bodies_retire_after_deletion_and_a_late_save_cannot_restore_removed_channels() {
		use client_core::inbox::{Action, Outcome};
		let mut state = test_support::demo_state();
		let message = state.timeline.iter().next().unwrap().clone();
		let row = model::inbox::SavedMessage {
			channel: message.channel,
			message: message.id,
			guild: None,
			due_at: None,
			body: Some(message.clone()),
		};
		state.inbox.saved = vec![row.clone()];
		state.inbox.saved_loaded = true;
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Delete {
				channel: message.channel,
				id: message.id,
			},
		});
		assert!(state.inbox.saved.is_empty() && !state.inbox.saved_loaded);
		state.inbox.saved = vec![row.clone()];
		let Some(Command::InboxAction { request, .. }) = state.request_inbox_action(Action::Save {
			channel: message.channel,
			message: message.id,
			due_at: None,
		}) else {
			panic!()
		};
		state
			.channels
			.retain(|channel| channel.id != message.channel);
		state.apply_inbox_action(request, Ok(Outcome::SavedOne(Box::new(row))));
		assert_eq!(state.inbox.saved.len(), 1);
		assert!(state.inbox.saved[0].body.is_none());
	}

	#[test]
	fn mentions_pages_bound_cursors_reject_stale_results_and_clear_on_close() {
		let mut state = test_support::demo_state();
		let Command::Mentions {
			request,
			before: None,
		} = state.request_mentions(None).unwrap()
		else {
			panic!()
		};
		let page = || {
			(76..=100)
				.rev()
				.map(|id| test_support::message(id, Id(20)))
				.collect()
		};
		state.apply_mentions(request + 1, Ok(page()));
		assert!(state.inbox.loading && state.inbox.messages.is_empty());
		state.apply_mentions(request, Ok(page()));
		assert_eq!(state.inbox.messages.len(), 25);
		assert_eq!(state.inbox.next, Some(Id(76)));
		assert!(state.request_mentions(Some(Id(999))).is_none());
		let Command::Mentions {
			request,
			before: Some(Id(76)),
		} = state.request_mentions(Some(Id(76))).unwrap()
		else {
			panic!()
		};
		state.apply_mentions(request, Ok(vec![test_support::message(100, Id(20))]));
		assert!(state.inbox.error.is_some());
		let Command::Mentions { request, .. } = state.request_mentions(Some(Id(76))).unwrap()
		else {
			panic!()
		};
		state.apply_mentions(request, Err(client_core::auth::Failure::Network));
		assert!(!state.inbox.loading && state.inbox.error.is_some());
		let Command::Mentions { request, .. } = state.request_mentions(None).unwrap() else {
			panic!()
		};
		state.apply_mentions(
			request,
			Ok((1..=26)
				.rev()
				.map(|id| test_support::message(id, Id(20)))
				.collect()),
		);
		assert!(state.inbox.error.is_some() && state.inbox.messages.is_empty());
		let Command::Mentions { request, .. } = state.request_mentions(None).unwrap() else {
			panic!()
		};
		state.clear_mentions();
		state.apply_mentions(request, Ok(page()));
		assert!(state.inbox.messages.is_empty() && !state.inbox.loading);
		let Command::Mentions { request, .. } = state.request_mentions(None).unwrap() else {
			panic!()
		};
		state.apply_mentions(request, Ok(page()));
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Delete {
				channel: Id(20),
				id: Id(999),
			},
		});
		assert_eq!(
			state.inbox.messages.len(),
			25,
			"unrelated messages must not erase the page"
		);
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Delete {
				channel: Id(20),
				id: Id(100),
			},
		});
		assert!(state.inbox.messages.is_empty() && !state.inbox.loaded);
	}

	#[test]
	fn narrow_inbox_actions_remain_inside_the_dialog_and_do_not_stack_letters() {
		for (tab, light) in [(Tab::Mentions, false), (Tab::Saved, true)] {
			let ctx = egui::Context::default();
			design::apply(&ctx);
			ctx.set_theme(if light {
				egui::Theme::Light
			} else {
				egui::Theme::Dark
			});
			let mut state = test_support::demo_state();
			let message = test_support::message(80, Id(20));
			state.inbox.messages = vec![message.clone()];
			state.inbox.loaded = true;
			state.inbox.saved = vec![model::inbox::SavedMessage {
				channel: Id(20),
				message: Id(80),
				guild: None,
				due_at: None,
				body: Some(message),
			}];
			state.inbox.saved_loaded = true;
			let mut view = MessagingUi {
				inbox_open: true,
				inbox_controls: Controls {
					tab,
					generation: Some(state.generation),
					..Default::default()
				},
				..Default::default()
			};
			for pass in 0..4 {
				let output = ctx.run_ui(
					egui::RawInput {
						screen_rect: Some(egui::Rect::from_min_size(
							egui::Pos2::ZERO,
							egui::vec2(390.0, 760.0),
						)),
						..Default::default()
					},
					|_| view.mentions_inbox(&ctx, &mut state, &mut Vec::new()),
				);
				if pass == 3 {
					let mut labels = 0;
					for shape in &output.shapes {
						if let egui::Shape::Text(text) = &shape.shape
							&& [
								"Inbox",
								"Open message",
								"Dismiss",
								"Bookmark message",
								"Remove bookmark",
								"Remind me",
							]
							.contains(&text.galley.job.text.as_str())
						{
							let rect = text.galley.rect.translate(text.pos.to_vec2());
							assert!(
								rect.left() >= 0.0 && rect.right() <= 390.0,
								"action outside viewport: {rect:?}"
							);
							assert!(
								rect.height() < 35.0,
								"action label wrapped vertically: {rect:?}"
							);
							labels += 1;
						}
					}
					assert!(labels >= 4, "native dialog actions must be rendered");
				}
				output.drop_without_applying_deltas();
			}
		}
	}
	#[test]
	fn mentions_dialog_opens_target_message_and_escape_clears_its_page() {
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut state = test_support::demo_state();
		let mut message = test_support::message(80, Id(20));
		message.content = "Visible ||concealed private spoiler||".into();
		state.inbox.messages = vec![message];
		state.inbox.loaded = true;
		let mut view = MessagingUi {
			inbox_open: true,
			inbox_controls: Controls {
				tab: Tab::Mentions,
				..Default::default()
			},
			..Default::default()
		};
		let frame = |view: &mut MessagingUi, state: &mut State, events| {
			let mut commands = vec![];
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(900.0, 700.0),
					)),
					events,
					..Default::default()
				},
				|_| view.mentions_inbox(&ctx, state, &mut commands),
			);
			assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text.contains("concealed private spoiler"))));
			let open = output.shapes.iter().find_map(|shape| match &shape.shape {
				egui::Shape::Text(text) if text.galley.job.text == "Open message" => {
					Some(text.galley.rect.translate(text.pos.to_vec2()).center())
				}
				_ => None,
			});
			output.drop_without_applying_deltas();
			(open, commands)
		};
		frame(&mut view, &mut state, vec![]);
		let point = frame(&mut view, &mut state, vec![]).0.unwrap();
		for pressed in [true, false] {
			frame(
				&mut view,
				&mut state,
				vec![
					egui::Event::PointerMoved(point),
					egui::Event::PointerButton {
						pos: point,
						button: egui::PointerButton::Primary,
						pressed,
						modifiers: egui::Modifiers::NONE,
					},
				],
			);
		}
		assert!(!view.inbox_open && state.inbox.messages.is_empty());
		assert_eq!(state.selected, Some(Id(20)));
		assert_eq!(state.search_target, Some(Id(80)));
		view.inbox_open = true;
		state.inbox.loaded = true;
		frame(&mut view, &mut state, vec![]);
		frame(
			&mut view,
			&mut state,
			vec![egui::Event::Key {
				key: egui::Key::Escape,
				physical_key: None,
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}],
		);
		assert!(!view.inbox_open && !state.inbox.loaded);
	}
}
