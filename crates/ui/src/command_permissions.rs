//! Server-installed app permissions. All writes go through the shipping administration gate.
use client_core::State;
use model::{Id, command_permissions as m};
#[derive(Default)]
pub(super) struct PermissionsUi {
	scope: Option<(u64, Id, Id)>,
	selected: Option<Id>,
	baseline: Option<m::Permissions>,
	draft: Option<m::Permissions>,
	loaded: bool,
	submitted: bool,
	reset_defaults: bool,
	error: Option<&'static str>,
	kind: u8,
	target: String,
	search: String,
}
impl PermissionsUi {
	pub fn rejected(&mut self) {
		self.submitted = false;
		self.error =
			Some("Could not save permissions; your draft is retained. Check access and reload.");
	}
	pub fn has_changes(&self) -> bool {
		self.baseline != self.draft || self.submitted
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		app: Id,
	) -> Option<m::Action> {
		if self.scope != Some((state.generation, guild, app)) {
			*self = Self {
				scope: Some((state.generation, guild, app)),
				selected: Some(app),
				kind: 1,
				..Default::default()
			};
		}
		if !state.can_manage_command_permissions(guild) {
			ui.weak("Manage Server and Manage Roles are required to edit command permissions.");
			return None;
		}
		let mut action = None;
		let snapshot = state
			.server_admin
			.command_permissions
			.as_ref()
			.filter(|page| page.guild == guild && page.application == app);
		if self.submitted && !state.server_admin.pending {
			self.submitted = false;
			if state.server_admin.error.is_none() {
				self.baseline = snapshot.and_then(|page| page.get(self.selected.unwrap_or(app)));
				self.draft = self.baseline.clone();
			}
		}
		ui.separator();
		ui.heading("Command permissions");
		ui.weak(
			"Set who can use this app and where. Individual commands can override its defaults.",
		);
		if let Some(error) = self.error.or(state.server_admin.error) {
			ui.colored_label(crate::design::palette(ui).danger, error);
		}
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					!state.server_admin.pending,
					egui::Button::new("Reload command permissions"),
				)
				.clicked()
			{
				action = Some(m::Action::Load { application: app });
				self.loaded = true;
				self.error = None;
			}
			if state.server_admin.pending {
				ui.spinner();
			}
		});
		if !self.loaded && snapshot.is_none() && !state.server_admin.pending {
			self.loaded = true;
			return Some(m::Action::Load { application: app });
		}
		let Some(snapshot) = snapshot else {
			ui.weak("Permissions have not loaded. Reload to try again.");
			return action;
		};
		let mut selected = self.selected.unwrap_or(app);
		egui::ComboBox::from_id_salt(("admin-command", app))
			.selected_text(if selected == app {
				"All commands · App defaults"
			} else {
				snapshot
					.commands
					.iter()
					.find(|row| row.id == selected)
					.map_or("Unavailable command", |row| row.name.as_str())
			})
			.show_ui(ui, |ui| {
				ui.add(
					egui::TextEdit::singleline(&mut self.search)
						.hint_text("Search commands")
						.char_limit(100),
				);
				ui.add_enabled_ui(!self.has_changes(), |ui| {
					ui.selectable_value(&mut selected, app, "All commands · App defaults");
					let query = self.search.to_lowercase();
					for command in snapshot
						.commands
						.iter()
						.filter(|row| row.name.to_lowercase().contains(&query))
					{
						ui.selectable_value(&mut selected, command.id, &command.name);
					}
				});
			});
		if self.selected != Some(selected) {
			self.selected = Some(selected);
			self.baseline = None;
			self.draft = None;
		}
		let actual = snapshot.get(selected);
		if self.baseline.is_none() {
			self.baseline = actual.clone();
			self.draft = actual.clone();
		}
		let Some(baseline) = &self.baseline else {
			ui.weak("This command no longer exists. Reload the application.");
			return action;
		};
		let Some(draft) = &mut self.draft else {
			return action;
		};
		let stale = actual
			.as_ref()
			.is_none_or(|actual| !baseline.equivalent(actual));
		let mut reapply = false;
		if stale {
			ui.colored_label(
				crate::design::palette(ui).warning,
				"Permissions changed in another client. Your draft is retained; Reset loads the latest rules.",
			);
			if ui
				.add_enabled(
					!state.server_admin.pending,
					egui::Button::new("Reapply my edits to the latest rules"),
				)
				.clicked()
			{
				reapply = true;
			}
		}
		let mut remove = None;
		for (index, row) in draft.overrides.iter_mut().enumerate() {
			ui.push_id((row.kind, row.id), |ui| {
				ui.horizontal_wrapped(|ui| {
					let label = match row.kind {
						1 => state
							.permissions
							.guilds
							.get(&guild)
							.and_then(|g| g.roles.as_ref())
							.and_then(|roles| roles.iter().find(|r| r.id == row.id))
							.map_or_else(|| format!("Role {}", row.id), |r| r.name.clone()),
						2 => state
							.user
							.as_ref()
							.filter(|u| u.id == row.id)
							.map_or_else(|| format!("Member {}", row.id), |u| u.name.clone()),
						3 => {
							if row.id == Id(guild.0.saturating_sub(1)) {
								"All channels".into()
							} else {
								state.channel(row.id).map_or_else(
									|| format!("Channel {}", row.id),
									|c| c.name.clone(),
								)
							}
						}
						_ => format!("Unsupported target type {} · {}", row.kind, row.id),
					};
					ui.label(label);
					ui.add_enabled_ui(
						(1..=3).contains(&row.kind) && !state.server_admin.pending,
						|ui| {
							ui.checkbox(&mut row.permission, "Allow");
							if ui.button("Remove override").clicked() {
								remove = Some(index);
							}
						},
					);
				})
			});
		}
		if let Some(index) = remove {
			draft.overrides.remove(index);
		}
		if draft.overrides.is_empty() {
			ui.weak("No overrides · uses the app's defaults and the member's permissions.");
		}
		ui.add_enabled_ui(
			!state.server_admin.pending && draft.overrides.len() < m::MAX_OVERRIDES,
			|ui| {
				ui.horizontal_wrapped(|ui| {
					egui::ComboBox::from_id_salt("command-target-kind")
						.selected_text(match self.kind {
							2 => "Member",
							3 => "Channel",
							_ => "Role",
						})
						.show_ui(ui, |ui| {
							ui.selectable_value(&mut self.kind, 1, "Role");
							ui.selectable_value(&mut self.kind, 2, "Member");
							ui.selectable_value(&mut self.kind, 3, "Channel");
						});
					ui.add(
						egui::TextEdit::singleline(&mut self.target)
							.hint_text("Target ID")
							.char_limit(20)
							.desired_width(170.0),
					);
					let id = self.target.parse::<Id>().ok().filter(|id| id.0 != 0);
					let valid = id.is_some_and(|id| match self.kind {
						1 => state
							.permissions
							.guilds
							.get(&guild)
							.and_then(|g| g.roles.as_ref())
							.is_some_and(|roles| roles.iter().any(|role| role.id == id)),
						2 => true,
						3 => {
							id == Id(guild.0.saturating_sub(1))
								|| state.channel(id).is_some_and(|c| c.guild == Some(guild))
						}
						_ => false,
					});
					if ui
						.add_enabled(valid, egui::Button::new("Add override"))
						.clicked()
					{
						if let Some(id) = id {
							if !draft
								.overrides
								.iter()
								.any(|r| r.id == id && r.kind == self.kind)
							{
								draft.overrides.push(m::Override {
									id,
									kind: self.kind,
									permission: true,
								});
								self.target.clear();
							}
						}
					}
				});
				if self.kind == 1 {
					ui.menu_button("Choose a role", |ui| {
						if let Some(roles) = state
							.permissions
							.guilds
							.get(&guild)
							.and_then(|g| g.roles.as_ref())
						{
							for role in roles {
								if ui.button(&role.name).clicked() {
									self.target = role.id.to_string();
									ui.close();
								}
							}
						}
					});
				}
				if self.kind == 3 {
					ui.menu_button("Choose a channel", |ui| {
						for channel in state
							.channels
							.iter()
							.filter(|c| c.guild == Some(guild) && state.can_view(c.id))
						{
							if ui.button(&channel.name).clicked() {
								self.target = channel.id.to_string();
								ui.close();
							}
						}
					});
				}
			},
		);
		if self.reset_defaults {
			ui.group(|ui| {
				ui.label(
					"Remove all custom overrides from this selection? Save changes will apply the reset.",
				);
				if ui.button("Clear overrides in draft").clicked() {
					draft.overrides.clear();
					self.reset_defaults = false;
				}
				if ui.button("Cancel reset").clicked() {
					self.reset_defaults = false;
				}
			});
		}
		let mut reset = false;
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					!stale
						&& !state.server_admin.needs_refresh
						&& (state.demo || state.gateway_connected)
						&& !state.server_admin.pending
						&& draft.valid() && !baseline.equivalent(draft),
					egui::Button::new("Save changes"),
				)
				.clicked()
			{
				action = Some(m::Action::Save {
					application: app,
					before: baseline.clone(),
					after: draft.clone(),
				});
				self.submitted = true;
			}
			if ui
				.add_enabled(
					!state.server_admin.pending,
					egui::Button::new("Reset draft"),
				)
				.clicked()
			{
				reset = true;
			}
			if ui
				.add_enabled(
					!state.server_admin.pending,
					egui::Button::new("Reset to defaults…"),
				)
				.clicked()
			{
				self.reset_defaults = true;
			}
		});
		if reapply
			&& let Some(actual) = actual.as_ref()
			&& let Some(before) = self.baseline.as_ref()
			&& let Some(draft) = self.draft.as_ref()
			&& let Some(rebased) = m::Permissions::reapply(before, draft, actual)
		{
			self.baseline = Some(actual.clone());
			self.draft = Some(rebased);
			self.error = None;
		}
		if reset {
			self.baseline = actual.clone();
			self.draft = actual;
			self.error = None;
		}
		action
	}
}
