//! Native rule editor; drafts survive rejected saves, and mutations need explicit actions.
use crate::{design, dialog, i18n};
use client_core::{Command, State};
use model::{
	Id,
	automod::{self as m, Action, Settings},
	server_admin,
};

struct Draft {
	id: Option<Id>,
	before: Settings,
	after: Settings,
	keywords: String,
	regex: String,
	allow: String,
	reason: String,
	role_search: String,
	channel_search: String,
}
impl Draft {
	fn new(id: Option<Id>, settings: Settings) -> Self {
		Self {
			keywords: settings.metadata.keywords.join("\n"),
			regex: settings.metadata.regex.join("\n"),
			allow: settings.metadata.allow.join("\n"),
			before: settings.clone(),
			after: settings,
			id,
			reason: String::new(),
			role_search: String::new(),
			channel_search: String::new(),
		}
	}
	fn settings(&self) -> Option<Settings> {
		let list = |value: &str, max: usize| {
			let lines: Vec<_> = value
				.lines()
				.filter(|line| !line.is_empty())
				.take(max + 1)
				.map(str::to_owned)
				.collect();
			(lines.len() <= max).then_some(lines)
		};
		let mut value = self.after.clone();
		if matches!(value.trigger, 1 | 6) {
			value.metadata.keywords = list(&self.keywords, 1000)?;
			value.metadata.regex = list(&self.regex, 10)?;
		}
		if matches!(value.trigger, 1 | 4 | 6) {
			value.metadata.allow = list(&self.allow, if value.trigger == 4 { 1000 } else { 100 })?;
		}
		value.valid().then_some(value)
	}
}
#[derive(Default)]
pub(super) struct AutoModUi {
	scope: Option<(u64, Id)>,
	attempted: bool,
	draft: Option<Draft>,
	submitted: bool,
	delete: Option<Id>,
	reason: String,
}
impl AutoModUi {
	pub fn has_changes(&self) -> bool {
		self.draft.is_some() || self.delete.is_some()
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		commands: &mut Vec<Command>,
	) {
		if self.scope != Some((state.generation, guild)) {
			*self = Self {
				scope: Some((state.generation, guild)),
				..Default::default()
			};
		}
		if !state.can_manage_guild(guild) {
			return;
		}
		let mut action = None;
		if state.server_admin.guild == Some(guild) && state.server_admin.automod.is_some() {
			self.attempted = true;
		}

		if !self.attempted && !state.server_admin.pending {
			action = Some(Action::Load);
			self.attempted = true;
		}
		if self.submitted && !state.server_admin.pending {
			self.submitted = false;
			if state.server_admin.error.is_none() {
				self.draft = None;
				self.delete = None;
				self.reason.clear();
			}
		}
		ui.heading(i18n::translate("automod-title"));
		ui.label(i18n::translate("automod-description"));
		if let Some(error) = state.server_admin.error {
			ui.colored_label(design::palette(ui).danger, error);
		}
		if state.server_admin.pending {
			ui.spinner();
		}
		let available = !state.server_admin.pending && (state.demo || state.gateway_connected);
		if state.server_admin.needs_refresh
			&& ui
				.add_enabled(
					available,
					egui::Button::new(i18n::translate("automod-refresh")),
				)
				.clicked()
		{
			action = Some(Action::Load);
		}

		if let Some(draft) = &mut self.draft {
			ui.add_enabled_ui(available, |ui| {
				ui.label(i18n::translate("automod-name"));
				ui.add(egui::TextEdit::singleline(&mut draft.after.name).char_limit(100));
				let previous = draft.after.trigger;
				ui.add_enabled_ui(draft.id.is_none(), |ui| {
					egui::ComboBox::from_id_salt("automod-trigger")
						.selected_text(trigger_name(draft.after.trigger))
						.show_ui(ui, |ui| {
							for kind in [1, 3, 4, 5, 6] {
								ui.selectable_value(
									&mut draft.after.trigger,
									kind,
									trigger_name(kind),
								);
							}
						});
				});
				if previous != draft.after.trigger {
					draft.after.event = if draft.after.trigger == 6 { 2 } else { 1 };
					draft.after.metadata = Default::default();
					draft.after.effects = vec![m::Effect {
						kind: if draft.after.trigger == 6 { 4 } else { 1 },
						..Default::default()
					}];
					draft.keywords.clear();
					draft.regex.clear();
					draft.allow.clear();
				}
				ui.checkbox(&mut draft.after.enabled, i18n::translate("automod-enabled"));
				if matches!(draft.after.trigger, 1 | 6) {
					lines(ui, "automod-keywords", &mut draft.keywords, 61000);
					lines(ui, "automod-regex", &mut draft.regex, 2610);
				}
				if draft.after.trigger == 4 {
					for (id, label) in [
						(1, "automod-profanity"),
						(2, "automod-sexual"),
						(3, "automod-slurs"),
					] {
						let mut enabled = draft.after.metadata.presets.contains(&id);
						if ui.checkbox(&mut enabled, i18n::translate(label)).changed() {
							draft.after.metadata.presets.retain(|value| *value != id);
							if enabled {
								draft.after.metadata.presets.push(id);
							}
						}
					}
				}
				if matches!(draft.after.trigger, 1 | 4 | 6) {
					lines(ui, "automod-allow", &mut draft.allow, 61000);
				}
				if draft.after.trigger == 5 {
					ui.horizontal(|ui| {
						ui.label(i18n::translate("automod-mention-limit"));
						ui.add(
							egui::DragValue::new(&mut draft.after.metadata.mentions).range(0..=50),
						);
					});
					ui.checkbox(
						&mut draft.after.metadata.raid_protection,
						i18n::translate("automod-raid"),
					);
				}
				ui.separator();
				ui.strong(i18n::translate("automod-actions"));
				if draft.after.trigger == 6 {
					effect_toggle(ui, &mut draft.after, 4, "automod-block-interaction");
				} else {
					effect_toggle(ui, &mut draft.after, 1, "automod-block-message");
				}
				effect_toggle(ui, &mut draft.after, 2, "automod-alert");
				if matches!(draft.after.trigger, 1 | 5) {
					ui.add_enabled_ui(
						state.guild_permission(guild, model::permissions::MODERATE_MEMBERS),
						|ui| {
							effect_toggle(ui, &mut draft.after, 3, "automod-timeout");
						},
					);
				}
				for effect in &mut draft.after.effects {
					match effect.kind {
						1 => {
							ui.label(i18n::translate("automod-block-explanation"));
							ui.add(egui::TextEdit::singleline(&mut effect.message).char_limit(150));
						}
						2 => {
							egui::ComboBox::from_id_salt("automod-alert-channel")
								.selected_text(
									effect.channel.and_then(|id| state.channel(id)).map_or_else(
										|| i18n::translate("automod-select-channel"),
										|row| row.name.clone(),
									),
								)
								.show_ui(ui, |ui| {
									for channel in state.channels.iter().filter(|row| {
										row.guild == Some(guild)
											&& matches!(row.kind, 0 | 5) && state.can_view(row.id)
									}) {
										ui.selectable_value(
											&mut effect.channel,
											Some(channel.id),
											&channel.name,
										);
									}
								});
						}
						3 => {
							ui.horizontal(|ui| {
								ui.label(i18n::translate("automod-timeout-seconds"));
								ui.add(
									egui::DragValue::new(&mut effect.duration).range(1..=2_419_200),
								);
							});
						}
						_ => {}
					}
				}
				ui.separator();
				ui.collapsing(i18n::translate("automod-exempt-roles"), |ui| {
					ui.add(
						egui::TextEdit::singleline(&mut draft.role_search)
							.hint_text(i18n::translate("automod-search-roles"))
							.char_limit(100),
					);
					let query = draft.role_search.trim().to_lowercase();
					if let Some(roles) = state
						.permissions
						.guilds
						.get(&guild)
						.and_then(|guild| guild.roles.as_ref())
					{
						let mut found = false;
						for role in roles
							.iter()
							.filter(|role| matches_exemption(&query, role.id, &role.name))
						{
							found = true;
							let mut exempt = draft.after.exempt_roles.contains(&role.id);
							if ui
								.add_enabled(
									exempt || draft.after.exempt_roles.len() < 20,
									egui::Checkbox::new(&mut exempt, &role.name),
								)
								.changed()
							{
								draft.after.exempt_roles.retain(|id| *id != role.id);
								if exempt {
									draft.after.exempt_roles.push(role.id);
								}
							}
						}
						if !found {
							ui.weak(i18n::translate("automod-no-matching-roles"));
						}
						if ui
							.button(i18n::translate("automod-clear-unavailable-roles"))
							.clicked()
						{
							draft
								.after
								.exempt_roles
								.retain(|id| roles.iter().any(|role| role.id == *id));
						}
					}
				});
				ui.collapsing(i18n::translate("automod-exempt-channels"), |ui| {
					ui.add(
						egui::TextEdit::singleline(&mut draft.channel_search)
							.hint_text(i18n::translate("automod-search-channels"))
							.char_limit(100),
					);
					let query = draft.channel_search.trim().to_lowercase();
					let mut found = false;
					for channel in state.channels.iter().filter(|row| {
						row.guild == Some(guild)
							&& state.can_view(row.id)
							&& matches_exemption(&query, row.id, &row.name)
					}) {
						found = true;
						let mut exempt = draft.after.exempt_channels.contains(&channel.id);
						if ui
							.add_enabled(
								exempt || draft.after.exempt_channels.len() < 50,
								egui::Checkbox::new(&mut exempt, &channel.name),
							)
							.changed()
						{
							draft.after.exempt_channels.retain(|id| *id != channel.id);
							if exempt {
								draft.after.exempt_channels.push(channel.id);
							}
						}
					}
					if !found {
						ui.weak(i18n::translate("automod-no-matching-channels"));
					}
					if ui
						.button(i18n::translate("automod-clear-unavailable-channels"))
						.clicked()
					{
						draft.after.exempt_channels.retain(|id| {
							state
								.channel(*id)
								.is_some_and(|row| row.guild == Some(guild) && state.can_view(*id))
						});
					}
				});
				reason(ui, &mut draft.reason);
			});
			if let Some(id) = draft.id
				&& let Some(latest) = state
					.server_admin
					.automod
					.as_ref()
					.and_then(|page| page.rules.iter().find(|rule| rule.id == id))
				&& latest.settings != draft.before
			{
				ui.weak(i18n::translate("automod-concurrent"));
				if let Some(changes) = draft
					.settings()
					.and_then(|after| latest.settings.reapply(&draft.before, &after))
					&& ui
						.add_enabled(
							available,
							egui::Button::new(i18n::translate("automod-reapply")),
						)
						.clicked()
				{
					let reason = std::mem::take(&mut draft.reason);
					*draft = Draft::new(Some(id), changes);
					draft.before = latest.settings.clone();
					draft.reason = reason;
				}
			}
			let settings = draft.settings();
			let request = settings.map(|settings| match draft.id {
				Some(id) => Action::Edit {
					id,
					before: Box::new(draft.before.clone()),
					after: Box::new(settings),
					reason: draft.reason.clone(),
				},
				None => Action::Create {
					settings: Box::new(settings),
					reason: draft.reason.clone(),
				},
			});
			let can_save = available
				&& !state.server_admin.needs_refresh
				&& request.as_ref().is_some_and(|action| {
					state.server_admin_action_allowed(
						guild,
						&server_admin::Action::AutoMod(action.clone()),
					)
				});
			let mut cancel = false;
			ui.horizontal(|ui| {
				if ui
					.add_enabled(can_save, egui::Button::new(i18n::translate("automod-save")))
					.clicked()
				{
					action = request;
					self.submitted = true;
				}
				cancel = ui
					.add_enabled(
						!state.server_admin.pending,
						egui::Button::new(i18n::translate("automod-cancel")),
					)
					.clicked();
			});
			if !can_save && available {
				ui.weak(i18n::translate("automod-invalid"));
			}
			if cancel {
				self.draft = None;
			}
		} else {
			ui.horizontal(|ui| {
				if ui
					.add_enabled(
						available,
						egui::Button::new(i18n::translate("automod-refresh")),
					)
					.clicked()
				{
					action = Some(Action::Load);
				}
				if ui
					.add_enabled(
						available && !state.server_admin.needs_refresh,
						egui::Button::new(i18n::translate("automod-create")),
					)
					.clicked()
				{
					self.draft = Some(Draft::new(None, Settings::default()));
				}
			});
			if let Some(page) = &state.server_admin.automod {
				if page.rules.is_empty() {
					ui.weak(i18n::translate("automod-empty"));
				}
				let mut edit = None;
				for rule in &page.rules {
					ui.push_id(rule.id.0, |ui| {
						ui.separator();
						ui.strong(&rule.settings.name);
						ui.weak(trigger_name(rule.settings.trigger));
						ui.label(i18n::translate(if rule.settings.enabled {
							"automod-enabled-state"
						} else {
							"automod-disabled-state"
						}));
						ui.horizontal(|ui| {
							if ui
								.add_enabled(
									available,
									egui::Button::new(i18n::translate("automod-edit")),
								)
								.clicked()
							{
								edit = Some(Draft::new(Some(rule.id), rule.settings.clone()));
							}
							if ui
								.add_enabled(
									available,
									egui::Button::new(i18n::translate("automod-delete")),
								)
								.clicked()
							{
								self.delete = Some(rule.id);
							}
						});
					});
				}
				if let Some(draft) = edit {
					self.draft = Some(draft);
				}
			}
		}
		if let Some(id) = self.delete {
			let mut confirm = false;
			let delete = Action::Delete {
				id,
				reason: self.reason.clone(),
			};
			let allowed = available
				&& !state.server_admin.needs_refresh
				&& state.server_admin_action_allowed(guild, &server_admin::Action::AutoMod(delete));
			let response =
				dialog::Dialog::new("delete-automod", i18n::translate("automod-delete-confirm"))
					.danger()
					.width(440.0)
					.show(ui.ctx(), |body| {
						body.content(|ui| {
							if let Some(rule) = state
								.server_admin
								.automod
								.as_ref()
								.and_then(|page| page.rules.iter().find(|rule| rule.id == id))
							{
								ui.strong(&rule.settings.name);
							}
							reason(ui, &mut self.reason);
						});
						body.footer(|ui| {
							confirm = ui
								.add_enabled(
									allowed && m::valid_reason(&self.reason),
									egui::Button::new(i18n::translate("automod-delete")),
								)
								.clicked();
						});
					});
			if response.close && !state.server_admin.pending {
				self.delete = None;
			}
			if confirm {
				action = Some(Action::Delete {
					id,
					reason: self.reason.clone(),
				});
				self.submitted = true;
			}
		}
		if let Some(action) = action {
			let write = action.write();
			if let Some(command) =
				state.request_server_admin(guild, server_admin::Action::AutoMod(action))
			{
				self.submitted = write;
				commands.push(command);
			} else {
				self.submitted = false;
				self.attempted = false;
			}
		}
	}
}
pub(super) fn reason(ui: &mut egui::Ui, value: &mut String) {
	ui.label(i18n::translate("moderation-reason"));
	ui.add(egui::TextEdit::singleline(value).char_limit(512));
}
fn trigger_name(kind: u8) -> String {
	i18n::translate(match kind {
		1 => "automod-keyword",
		3 => "automod-spam",
		4 => "automod-presets",
		5 => "automod-mentions",
		6 => "automod-profile",
		_ => "automod-title",
	})
}
fn matches_exemption(query: &str, id: Id, name: &str) -> bool {
	query.is_empty() || name.to_lowercase().contains(query) || query.parse::<Id>() == Ok(id)
}
fn lines(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) {
	ui.label(i18n::translate(label));
	ui.add(
		egui::TextEdit::multiline(value)
			.char_limit(limit)
			.desired_rows(3),
	);
}
fn effect_toggle(ui: &mut egui::Ui, settings: &mut Settings, kind: u8, label: &str) {
	let mut enabled = settings.effects.iter().any(|effect| effect.kind == kind);
	if ui.checkbox(&mut enabled, i18n::translate(label)).changed() {
		settings.effects.retain(|effect| effect.kind != kind);
		if enabled {
			settings.effects.push(m::Effect {
				kind,
				duration: if kind == 3 { 60 } else { 0 },
				..Default::default()
			});
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn exemption_search_filters_production_selectors_without_dropping_hidden_selections() {
		fn frame(
			ctx: &egui::Context,
			editor: &mut AutoModUi,
			state: &mut State,
			guild: Id,
			events: Vec<egui::Event>,
		) -> egui::FullOutput {
			let mut commands = vec![];
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(900.0, 1800.0),
					)),
					time: Some(ctx.input(|input| input.time) + 0.3),
					events,
					..Default::default()
				},
				|ui| editor.show(ui, state, guild, &mut commands),
			);
			assert!(
				commands.is_empty(),
				"search and selection do not submit a rule"
			);
			output
		}
		fn position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
			output.shapes.iter().find_map(|shape| match &shape.shape {
				egui::Shape::Text(text) if text.galley.job.text == label => {
					Some(text.pos + text.galley.size() / 2.0)
				}
				_ => None,
			})
		}
		fn click(position: egui::Pos2) -> Vec<egui::Event> {
			vec![
				egui::Event::PointerMoved(position),
				egui::Event::PointerButton {
					pos: position,
					button: egui::PointerButton::Primary,
					pressed: true,
					modifiers: egui::Modifiers::NONE,
				},
				egui::Event::PointerButton {
					pos: position,
					button: egui::PointerButton::Primary,
					pressed: false,
					modifiers: egui::Modifiers::NONE,
				},
			]
		}
		i18n::set_current(i18n::Language::English);
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let mut permissions = test_support::permission_snapshot(&state);
		let metadata = permissions
			.guilds
			.iter_mut()
			.find(|g| g.id == guild)
			.unwrap();
		metadata.owner = state.user.as_ref().map(|u| u.id);
		let roles = metadata.roles.as_mut().unwrap();
		let mut role = roles[0].clone();
		role.id = Id(19991);
		role.name = "Hidden selected role".into();
		roles.push(role.clone());
		role.id = Id(19992);
		role.name = "Target Moderator".into();
		roles.push(role);
		state.permissions.replace(permissions).unwrap();
		let channels: Vec<_> = state
			.channels
			.iter_mut()
			.filter(|c| c.guild == Some(guild) && c.kind == 0)
			.take(2)
			.enumerate()
			.map(|(i, c)| {
				c.name = if i == 0 {
					"Target Room"
				} else {
					"Hidden selected channel"
				}
				.into();
				c.id
			})
			.collect();
		assert_eq!(channels.len(), 2);
		let settings = Settings {
			name: "Synthetic filter".into(),
			trigger: 3,
			exempt_roles: vec![Id(19991)],
			exempt_channels: vec![channels[1]],
			..Default::default()
		};
		state.server_admin.guild = Some(guild);
		state.server_admin.automod = Some(m::Snapshot {
			guild,
			rules: vec![m::Rule {
				id: Id(77),
				guild,
				creator: Id(1),
				settings: settings.clone(),
			}],
		});
		let mut draft = Draft::new(Some(Id(77)), settings);
		draft.role_search = "TARGET".into();
		draft.channel_search = "target".into();
		let mut editor = AutoModUi {
			scope: Some((state.generation, guild)),
			attempted: true,
			draft: Some(draft),
			..Default::default()
		};
		let ctx = egui::Context::default();
		design::apply(&ctx);
		for label in ["Exempt roles", "Exempt channels"] {
			let output = frame(&ctx, &mut editor, &mut state, guild, vec![]);
			let point = position(&output, label).expect("selector heading");
			output.drop_without_applying_deltas();
			frame(&ctx, &mut editor, &mut state, guild, click(point))
				.drop_without_applying_deltas();
		}
		for _ in 0..4 {
			frame(&ctx, &mut editor, &mut state, guild, vec![]).drop_without_applying_deltas();
		}
		for label in [
			"Target Moderator",
			"Target Room",
			"Remove unavailable roles",
			"Remove unavailable channels",
		] {
			let output = frame(&ctx, &mut editor, &mut state, guild, vec![]);
			assert!(position(&output, "Hidden selected role").is_none());
			assert!(position(&output, "Hidden selected channel").is_none());
			let point = position(&output, label).expect("filtered control");
			output.drop_without_applying_deltas();
			frame(&ctx, &mut editor, &mut state, guild, click(point))
				.drop_without_applying_deltas();
		}
		let draft = editor.draft.as_mut().unwrap();
		assert!(
			draft.after.exempt_roles.contains(&Id(19991))
				&& draft.after.exempt_roles.contains(&Id(19992))
		);
		assert!(
			draft.after.exempt_channels.contains(&channels[0])
				&& draft.after.exempt_channels.contains(&channels[1])
		);
		draft.role_search = "19992".into();
		draft.channel_search = channels[0].to_string();
		let output = frame(&ctx, &mut editor, &mut state, guild, vec![]);
		assert!(
			position(&output, "Target Moderator").is_some()
				&& position(&output, "Target Room").is_some()
		);
		output.drop_without_applying_deltas();
		let draft = editor.draft.as_mut().unwrap();
		draft.role_search = "no matching role".into();
		draft.channel_search = "no matching channel".into();
		let output = frame(&ctx, &mut editor, &mut state, guild, vec![]);
		assert!(
			position(&output, "No matching roles.").is_some()
				&& position(&output, "No matching channels.").is_some()
		);
		output.drop_without_applying_deltas();
		let draft = editor.draft.as_ref().unwrap();
		assert_eq!(draft.after.exempt_roles.len(), 2);
		assert_eq!(draft.after.exempt_channels.len(), 2);
	}
	#[test]
	fn unrelated_rule_edits_preserve_keyword_and_regex_whitespace() {
		let settings = Settings {
			name: "Original".into(),
			metadata: m::Metadata {
				keywords: vec!["  literal phrase  ".into()],
				regex: vec![" ^spam$ ".into()],
				allow: vec![" allowed ".into()],
				..Default::default()
			},
			..Default::default()
		};
		assert!(settings.valid());
		let mut draft = Draft::new(Some(Id(77)), settings.clone());
		draft.after.name = "Renamed rule".into();
		assert!(draft.settings().unwrap().metadata == settings.metadata);
		draft.regex = "  ^edited$  ".into();
		assert_eq!(
			draft.settings().unwrap().metadata.regex,
			vec!["  ^edited$  "]
		);
	}
	#[test]
	fn offline_rule_confirmation_can_be_cancelled_without_a_write() {
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		state.demo = false;
		state.auth = client_core::auth::AuthState::Authenticated;
		state.gateway_connected = false;
		state.permissions.guilds.insert(
			guild,
			model::permissions::Guild {
				id: guild,
				owner: state.user.as_ref().map(|user| user.id),
				roles: Some(Vec::new()),
				member: None,
			},
		);
		state.permissions.clear_cache();
		state.server_admin.guild = Some(guild);
		let mut editor = AutoModUi {
			scope: Some((state.generation, guild)),
			attempted: true,
			delete: Some(Id(77)),
			..Default::default()
		};
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut commands = Vec::new();
		for pass in 0..4 {
			ctx.run_ui(
				egui::RawInput {
					focused: true,
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(700.0, 700.0),
					)),
					events: if pass == 3 {
						vec![egui::Event::Key {
							key: egui::Key::Escape,
							physical_key: None,
							pressed: true,
							repeat: false,
							modifiers: egui::Modifiers::NONE,
						}]
					} else {
						vec![]
					},
					..Default::default()
				},
				|ui| editor.show(ui, &mut state, guild, &mut commands),
			)
			.drop_without_applying_deltas();
		}
		assert!(editor.delete.is_none() && commands.is_empty());
	}
	#[test]
	fn rejected_rule_saves_preserve_drafts_and_account_changes_retire_them() {
		for (width, dark) in [(390.0, false), (1120.0, true)] {
			let mut state = test_support::demo_state();
			let guild = state.guilds[0].id;
			state.permissions.guilds.insert(
				guild,
				model::permissions::Guild {
					id: guild,
					owner: state.user.as_ref().map(|user| user.id),
					roles: Some(Vec::new()),
					member: Some(model::permissions::Member {
						roles: Vec::new(),
						timeout_until: None,
					}),
				},
			);
			state.permissions.clear_cache();
			state.server_admin.guild = Some(guild);
			let settings = Settings {
				name: "Original".into(),
				metadata: m::Metadata {
					keywords: vec!["synthetic".into()],
					..Default::default()
				},
				..Default::default()
			};
			let mut draft = Draft::new(Some(Id(77)), settings.clone());
			draft.after.name = "Keep my edits".into();
			draft.reason = "Keep my reason".into();
			state.server_admin.automod = Some(m::Snapshot {
				guild,
				rules: vec![m::Rule {
					id: Id(77),
					guild,
					creator: Id(1),
					settings,
				}],
			});
			state.server_admin.error = Some("Permission denied");
			let mut editor = AutoModUi {
				scope: Some((state.generation, guild)),
				attempted: true,
				draft: Some(draft),
				submitted: true,
				..Default::default()
			};
			let ctx = egui::Context::default();
			crate::design::apply(&ctx);
			ctx.set_visuals(if dark {
				egui::Visuals::dark()
			} else {
				egui::Visuals::light()
			});
			let mut commands = Vec::new();
			let frame = |editor: &mut AutoModUi, state: &mut State, commands: &mut Vec<Command>| {
				ctx.run_ui(
					egui::RawInput {
						screen_rect: Some(egui::Rect::from_min_size(
							egui::Pos2::ZERO,
							egui::vec2(width, 900.0),
						)),
						..Default::default()
					},
					|ui| editor.show(ui, state, guild, commands),
				)
				.drop_without_applying_deltas();
			};
			frame(&mut editor, &mut state, &mut commands);
			assert!(commands.is_empty());
			let draft = editor.draft.as_ref().unwrap();
			assert_eq!(draft.after.name, "Keep my edits");
			assert_eq!(draft.reason, "Keep my reason");
			assert_eq!(draft.keywords, "synthetic");
			state.generation += 1;
			frame(&mut editor, &mut state, &mut commands);
			assert!(editor.draft.is_none());
		}
	}
}
