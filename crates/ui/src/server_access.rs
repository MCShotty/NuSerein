//! Native application review and explicit server security/access controls.
use crate::{design, dialog, local_time};
use client_core::{Command, State};
use model::{
	Id,
	server_access::{Action, Configuration, Incident, Status},
	server_admin,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
	Applications,
	Safety,
	Access,
}
#[derive(Clone)]
enum Confirmation {
	Decision { request: Id, status: Status },
	Incident { before: Incident, after: Incident },
	Mfa { before: u8, after: u8 },
	Transfer { owner: Id, recipient: Id },
}
#[derive(Default)]
pub(super) struct AccessUi {
	scope: Option<(u64, Id)>,
	status: Status,
	selected: Option<Id>,
	baseline: Option<Configuration>,
	draft: Option<Configuration>,
	revision: u64,
	reason: String,
	recipient: String,
	transfer_name: String,
	confirmation: Option<Confirmation>,
	submitted: bool,
	open_interview: bool,
	pub opened_channel: Option<Id>,
	invites: bool,
	dms: bool,
	duration: u32,
	error: Option<&'static str>,
}
impl AccessUi {
	pub fn allowed(state: &State, guild: Id, page: Page) -> bool {
		match page {
			Page::Applications => state.can_review_guild_applications(guild),
			Page::Safety | Page::Access => state.can_manage_guild(guild),
		}
	}
	pub fn has_changes(&self) -> bool {
		self.confirmation.is_some() || self.submitted || self.draft != self.baseline
	}
	fn scope(&mut self, state: &State, guild: Id) {
		if self.scope != Some((state.generation, guild)) {
			*self = Self {
				scope: Some((state.generation, guild)),
				duration: 3600,
				..Self::default()
			};
		}
	}
	pub fn load(&mut self, state: &mut State, guild: Id, page: Page) -> Option<Command> {
		self.scope(state, guild);
		if !Self::allowed(state, guild, page)
			|| state.server_admin.pending
			|| state.server_admin.error.is_some()
		{
			return None;
		}
		let needed = state.server_admin.guild != Some(guild)
			|| match page {
				Page::Applications => state.server_admin.access.applications.is_none(),
				Page::Safety | Page::Access => state.server_admin.access.settings.is_none(),
			};
		if !needed {
			return None;
		}
		state.request_server_admin(
			guild,
			server_admin::Action::Access(match page {
				Page::Applications => Action::Applications {
					status: self.status,
					before: None,
				},
				_ => Action::Settings,
			}),
		)
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		page: Page,
		commands: &mut Vec<Command>,
	) {
		self.scope(state, guild);
		if !Self::allowed(state, guild, page) {
			self.confirmation = None;
			self.reason.clear();
			self.transfer_name.clear();
			self.recipient.clear();
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"Your current server permissions do not allow this page.",
			);
			return;
		}
		if self.submitted && !state.server_admin.pending {
			self.submitted = false;
			if state.server_admin.error.is_none() {
				self.confirmation = None;
				self.reason.clear();
				self.baseline = None;
				self.draft = None;
				self.transfer_name.clear();
			}
		}
		if self.open_interview && !state.server_admin.pending {
			self.open_interview = false;
			if state.server_admin.error.is_none() {
				self.opened_channel = state.server_admin.access.interview;
			}
		}
		if let Some(channel) = self.opened_channel.take()
			&& state.channel(channel).is_some()
			&& let Some(command) = state.select(channel)
		{
			commands.push(command);
		}
		if matches!(self.confirmation,Some(Confirmation::Decision{request,..}) if state.server_admin.access.removed_application==Some(request))
			|| matches!(self.confirmation,Some(Confirmation::Transfer{..}) if !state.owns_guild(guild))
		{
			self.confirmation = None;
			self.reason.clear();
			self.transfer_name.clear();
		}
		let available = !state.server_admin.pending && (state.demo || state.gateway_connected);
		let mut action = None;
		ui.horizontal_wrapped(|ui| {
			ui.heading(match page {
				Page::Applications => "Member applications",
				Page::Safety => "Security actions",
				Page::Access => "Server access",
			});
			if ui
				.add_enabled(available, egui::Button::new("Reload"))
				.clicked()
			{
				action = Some(if page == Page::Applications {
					Action::Applications {
						status: self.status,
						before: None,
					}
				} else {
					Action::Settings
				});
			}
		});
		if state.server_admin.pending {
			ui.spinner();
		}
		if let Some(error) = self.error.or(state.server_admin.error) {
			dialog::notice(ui, dialog::Level::Error, error);
		}
		match page {
			Page::Applications => self.applications(ui, state, guild, available, &mut action),
			Page::Safety => self.safety(ui, state, guild, available, &mut action),
			Page::Access => self.access(ui, state, guild, available, &mut action),
		}
		self.confirm(ui, state, guild, available, &mut action);
		if let Some(action) = action {
			let write = action.write();
			let interview = matches!(action, Action::Interview { .. });
			if let Some(command) =
				state.request_server_admin(guild, server_admin::Action::Access(action))
			{
				commands.push(command);
				self.submitted = write;
				self.open_interview = interview;
				self.error = None;
			} else {
				self.error = Some(
					"This action is unavailable. Reload and check your current permissions before trying again.",
				);
			}
		}
	}
	fn applications(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		available: bool,
		action: &mut Option<Action>,
	) {
		if state.server_admin.access.applications_changed {
			dialog::notice(
				ui,
				dialog::Level::Info,
				"Applications changed in Discord. Reload this view before reviewing another application.",
			);
		}
		if self.selected == state.server_admin.access.removed_application && self.selected.is_some()
		{
			ui.weak("This application was withdrawn or removed.");
			self.selected = None;
		}
		let before = self.status;
		ui.add_enabled_ui(available, |ui| {
			ui.horizontal_wrapped(|ui| {
				for (status, label) in [
					(Status::Pending, "Pending"),
					(Status::Approved, "Approved"),
					(Status::Rejected, "Rejected"),
				] {
					ui.selectable_value(&mut self.status, status, label);
				}
			});
		});
		if before != self.status {
			self.selected = None;
			self.reason.clear();
			*action = Some(Action::Applications {
				status: self.status,
				before: None,
			});
		}
		let Some(page) = state
			.server_admin
			.access
			.applications
			.as_ref()
			.filter(|v| v.guild == guild && v.status == self.status)
		else {
			ui.weak("Applications are available only when this server supports manual approval. Reload to request the current list.");
			return;
		};
		if let Some(total) = page.total {
			ui.weak(format!(
				"{total} {} applications",
				status_label(page.status).to_lowercase()
			));
		}
		if page.items.is_empty() {
			ui.label("No applications in this view.");
		}
		egui::ScrollArea::vertical()
			.id_salt("application-rows")
			.max_height(300.0)
			.show_rows(ui, 48.0, page.items.len(), |ui, range| {
				for row in &page.items[range] {
					let name = row
						.user
						.as_ref()
						.map(|u| u.name.clone())
						.unwrap_or_else(|| format!("User {}", row.user_id));
					let response = ui.add_enabled(
						available,
						egui::Button::new(format!("{name}\n{}", date(Some(row.created_at))))
							.selected(self.selected == Some(row.id))
							.min_size(egui::vec2(ui.available_width(), 44.0)),
					);
					if response.clicked() {
						self.selected = Some(row.id);
						self.reason.clear();
						*action = Some(Action::Application(row.id));
					}
				}
			});
		if let Some(before) = page.next
			&& ui
				.add_enabled(available, egui::Button::new("Older applications"))
				.clicked()
		{
			*action = Some(Action::Applications {
				status: self.status,
				before: Some(before),
			});
			self.selected = None;
		}
		let Some(row) = self.selected.and_then(|id| {
			state
				.server_admin
				.access
				.detail
				.as_ref()
				.filter(|v| v.id == id && v.guild == guild)
		}) else {
			return;
		};
		ui.separator();
		if let Some(user) = &row.user {
			ui.strong(&user.name);
			if let Some(username) = &user.username {
				ui.weak(format!("@{username}"));
			}
		}
		ui.label(format!(
			"{} · submitted {}",
			status_label(row.status),
			date(Some(row.created_at))
		));
		ui.weak(format!(
			"Discord account created {}",
			snowflake_date(row.user_id)
		));
		if row.answers.is_empty() {
			ui.weak("No answers were supplied.");
		}
		for answer in &row.answers {
			ui.strong(&answer.label);
			ui.label(&answer.answer);
			ui.add_space(8.0);
		}
		if let Some(reason) = &row.reason {
			ui.label(format!("Rejection reason: {reason}"));
		}
		if let Some(moderator) = &row.moderator {
			ui.weak(format!(
				"Reviewed by {} · {}",
				moderator.name,
				date(row.reviewed_at)
			));
		}
		ui.add_enabled_ui(
			available
				&& !state.server_admin.needs_refresh
				&& !state.server_admin.access.applications_changed,
			|ui| {
				ui.horizontal_wrapped(|ui| {
					if row.status == Status::Pending {
						if ui.button("Approve").clicked() {
							self.confirmation = Some(Confirmation::Decision {
								request: row.id,
								status: Status::Approved,
							});
						}
						if ui.button("Reject").clicked() {
							self.confirmation = Some(Confirmation::Decision {
								request: row.id,
								status: Status::Rejected,
							});
						}
						if ui
							.button(if row.interview.is_some() {
								"Open interview"
							} else {
								"Create interview"
							})
							.clicked()
						{
							*action = Some(Action::Interview { request: row.id });
						}
					}
					if ui.button("Reload application").clicked() {
						*action = Some(Action::Application(row.id));
					}
				});
			},
		);
		ui.weak("Opening an interview does not send a message.");
	}
	fn safety(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		available: bool,
		action: &mut Option<Action>,
	) {
		let Some(settings) = state
			.server_admin
			.access
			.settings
			.as_ref()
			.filter(|v| v.guild == guild)
		else {
			return;
		};
		let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
		if self.revision != state.server_admin.access.revision {
			self.revision = state.server_admin.access.revision;
			self.invites = settings
				.incident
				.is_some_and(|v| v.invites_until.is_some_and(|until| until > now));
			self.dms = settings
				.incident
				.is_some_and(|v| v.dms_until.is_some_and(|until| until > now));
		}
		if settings
			.features
			.iter()
			.any(|v| v == model::server_invites::PAUSED_FEATURE)
		{
			dialog::notice(
				ui,
				dialog::Level::Info,
				"Invitation links are also paused in Invites. Timed security actions do not remove that separate pause.",
			);
		}
		let Some(before) = settings.incident else {
			ui.weak(
				"Discord did not provide security-action state. Reload or manage these actions in Discord.",
			);
			return;
		};
		for (label, until) in [
			("Invitations", before.invites_until),
			("DMs between non-friends", before.dms_until),
		] {
			ui.label(format!(
				"{label}: {}",
				until.filter(|v| *v > now).map_or_else(
					|| "not paused".into(),
					|v| format!("paused until {}", date(Some(v)))
				)
			));
		}
		ui.add_enabled_ui(available && !state.server_admin.needs_refresh, |ui| {
			ui.checkbox(&mut self.invites, "Pause invitations");
			ui.checkbox(&mut self.dms, "Pause DMs between non-friend server members");
			egui::ComboBox::from_id_salt("incident-duration")
				.selected_text(duration(self.duration))
				.show_ui(ui, |ui| {
					for seconds in [900, 3600, 14400, 86400] {
						ui.selectable_value(&mut self.duration, seconds, duration(seconds));
					}
				});
			if ui.button("Apply security actions").clicked() {
				let until = now + i128::from(self.duration) * 1_000_000_000;
				let after = Incident {
					invites_until: self.invites.then_some(until),
					dms_until: self.dms.then_some(until),
				};
				if after != before {
					self.confirmation = Some(Confirmation::Incident { before, after });
				}
			}
			ui.horizontal_wrapped(|ui| {
				if before.invites_until.is_some_and(|v| v > now)
					&& ui.button("Resume invitations now").clicked()
				{
					self.confirmation = Some(Confirmation::Incident {
						before,
						after: resume_incident(before, true, false, now),
					});
				}
				if before.dms_until.is_some_and(|v| v > now)
					&& ui.button("Resume DMs now").clicked()
				{
					self.confirmation = Some(Confirmation::Incident {
						before,
						after: resume_incident(before, false, true, now),
					});
				}
				if before.active(now) && ui.button("Resume both now").clicked() {
					self.confirmation = Some(Confirmation::Incident {
						before,
						after: Incident::default(),
					});
				}
			});
		});
		ui.separator();
		ui.strong("Moderator verification");
		ui.label(if settings.mfa_level == 1 {
			"Moderators must use two-factor authentication for protected actions."
		} else {
			"This server does not require moderator two-factor authentication."
		});
		if state.owns_guild(guild) {
			if ui
				.add_enabled(
					available && !state.server_admin.needs_refresh,
					egui::Button::new(if settings.mfa_level == 1 {
						"Remove moderator MFA requirement"
					} else {
						"Require moderator MFA"
					}),
				)
				.clicked()
			{
				self.confirmation = Some(Confirmation::Mfa {
					before: settings.mfa_level,
					after: 1 - settings.mfa_level,
				});
			}
		} else {
			ui.weak("Only the server owner can change this requirement.");
		}
		let _ = action;
	}
	fn access(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		available: bool,
		action: &mut Option<Action>,
	) {
		let Some(settings) = state
			.server_admin
			.access
			.settings
			.as_ref()
			.filter(|v| v.guild == guild)
		else {
			return;
		};
		if self.baseline.is_none() || self.draft == self.baseline {
			self.baseline = Some(settings.configuration);
			self.draft = self.baseline;
		}
		let before = self.baseline.unwrap_or_default();
		let mut after = self.draft.unwrap_or(before);
		let conflicted = settings.configuration != before;
		if conflicted {
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"Server access changed elsewhere. Review the current settings before reapplying or discarding your changes.",
			);
			ui.add_enabled_ui(available && !state.server_admin.needs_refresh, |ui| {
				ui.horizontal_wrapped(|ui| {
					if ui.button("Reapply my changes").clicked() {
						self.baseline = Some(settings.configuration);
						self.draft =
							Some(reapply_configuration(before, after, settings.configuration));
					}
					if ui.button("Use current settings").clicked() {
						self.baseline = Some(settings.configuration);
						self.draft = self.baseline;
					}
				});
			});
		}
		ui.add_enabled_ui(available && !state.server_admin.needs_refresh, |ui| {
			ui.checkbox(&mut after.community, "Enable Community");
			channel_picker(ui, state, guild, "Rules channel", &mut after.rules);
			channel_picker(
				ui,
				state,
				guild,
				"Community updates channel",
				&mut after.updates,
			);
			channel_picker(ui, state, guild, "Safety alerts channel", &mut after.alerts);
		});
		// Conflict choices above replace the draft; do not overwrite that choice
		// with this frame's old values.
		if self.baseline == Some(before) {
			self.draft = Some(after);
		}
		if after.community && (settings.verification_level < 1 || settings.content_filter != 2) {
			ui.colored_label(design::palette(ui).danger,"Community requires verification level Low or higher and the content filter for all members. Set these in Safety Setup first.");
		}
		if before != after {
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(
						available
							&& !state.server_admin.needs_refresh
							&& !conflicted && after.valid(),
						egui::Button::new("Save changes"),
					)
					.clicked()
				{
					*action = Some(Action::Configure { before, after });
				}
				if ui
					.add_enabled(available, egui::Button::new("Reset"))
					.clicked()
				{
					self.draft = Some(before);
				}
			});
		}
		if !state.owns_guild(guild) {
			return;
		}
		ui.separator();
		ui.heading("Transfer ownership");
		ui.weak(
			"The chosen member becomes the server owner. This cannot be undone by your account.",
		);
		ui.add_enabled_ui(available && !state.server_admin.needs_refresh, |ui| {
			ui.horizontal_wrapped(|ui| {
				ui.label("New owner's user ID");
				ui.add(
					egui::TextEdit::singleline(&mut self.recipient)
						.char_limit(20)
						.desired_width(190.0),
				);
				if ui
					.add_enabled(
						self.recipient.parse::<Id>().is_ok(),
						egui::Button::new("Verify member"),
					)
					.clicked()
				{
					*action = self.recipient.parse().ok().map(Action::Recipient);
				}
			});
			if let Some(members) = &state.server_admin.members {
				egui::ComboBox::from_id_salt("owner-member")
					.selected_text("Choose a loaded member")
					.show_ui(ui, |ui| {
						for member in &members.items {
							if member.user.id != settings.owner
								&& member.user.kind == model::AccountKind::Human
								&& ui.button(&member.user.name).clicked()
							{
								self.recipient = member.user.id.to_string();
								*action = Some(Action::Recipient(member.user.id));
							}
						}
					});
			}
			if let Some(member) = state
				.server_admin
				.access
				.recipient
				.as_ref()
				.filter(|v| self.recipient.parse::<Id>() == Ok(v.user.id))
			{
				ui.strong(format!("Verified server member: {}", member.user.name));
				if let Some(username) = &member.user.username {
					ui.weak(format!("@{username}"));
				}
				if ui.button("Transfer ownership…").clicked() {
					self.transfer_name.clear();
					self.confirmation = Some(Confirmation::Transfer {
						owner: settings.owner,
						recipient: member.user.id,
					});
				}
			}
		});
	}
	fn confirm(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		available: bool,
		action: &mut Option<Action>,
	) {
		let Some(confirmation) = self.confirmation.clone() else {
			return;
		};
		let mut open = true;
		let mut dismiss = false;
		egui::Window::new("Confirm server action")
			.id(egui::Id::unique("access-confirmation"))
			.open(&mut open)
			.collapsible(false)
			.resizable(false)
			.default_width(420.0)
			.show(ui.ctx(), |ui| {
				match &confirmation {
					Confirmation::Decision { status, .. } => {
						ui.label(if *status == Status::Approved {
							"Approve this member application?"
						} else {
							"Reject this member application?"
						});
						if *status == Status::Rejected {
							ui.label("Reason (optional)");
							ui.add(
								egui::TextEdit::multiline(&mut self.reason)
									.char_limit(160)
									.desired_rows(3),
							);
						}
					}
					Confirmation::Incident { after, .. } => {
						ui.label("Apply these server security actions?");
						ui.label(format!(
							"Invitations: {}",
							after.invites_until.map_or_else(
								|| "resume".into(),
								|at| format!("pause until {}", date(Some(at)))
							)
						));
						ui.label(format!(
							"Non-friend DMs: {}",
							after.dms_until.map_or_else(
								|| "resume".into(),
								|at| format!("pause until {}", date(Some(at)))
							)
						));
					}
					Confirmation::Mfa { after, .. } => {
						ui.label(if *after == 1 {
							"Require moderator two-factor authentication?"
						} else {
							"Remove the moderator two-factor requirement?"
						});
					}
					Confirmation::Transfer { recipient, .. } => {
						ui.label(format!(
							"Transfer this server to the verified member {recipient}?"
						));
						ui.label("Type the server name to confirm.");
						ui.add(egui::TextEdit::singleline(&mut self.transfer_name).char_limit(100));
					}
				}
				let ready = available
					&& !state.server_admin.needs_refresh
					&& !self.submitted
					&& (!matches!(confirmation, Confirmation::Decision { .. })
						|| !state.server_admin.access.applications_changed);
				let name_matches = !matches!(confirmation, Confirmation::Transfer { .. })
					|| state
						.guild(guild)
						.is_some_and(|g| g.name == self.transfer_name);
				ui.horizontal_wrapped(|ui| {
					if ui
						.add_enabled(ready && name_matches, egui::Button::new("Confirm"))
						.clicked()
					{
						*action = Some(match confirmation.clone() {
							Confirmation::Decision { request, status } => Action::Decide {
								request,
								status,
								reason: if status == Status::Rejected {
									self.reason.trim().to_owned()
								} else {
									String::new()
								},
							},
							Confirmation::Incident { before, after } => {
								Action::Incident { before, after }
							}
							Confirmation::Mfa { before, after } => {
								Action::MfaLevel { before, after }
							}
							Confirmation::Transfer { owner, recipient } => Action::Transfer {
								before_owner: owner,
								recipient,
							},
						});
					}
					if ui
						.add_enabled(!self.submitted, egui::Button::new("Cancel"))
						.clicked()
					{
						dismiss = true;
					}
				});
			});
		if !open || dismiss {
			self.confirmation = None;
			self.reason.clear();
			self.transfer_name.clear();
		}
	}
	pub fn banner(ui: &mut egui::Ui, state: &State, guild: Id) -> bool {
		let Some(incident) = state.guild_incident(guild) else {
			return false;
		};
		let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
		if !incident.active(now) {
			return false;
		}
		let mut review = false;
		egui::Frame::new()
			.fill(design::palette(ui).raised)
			.inner_margin(10)
			.corner_radius(6)
			.show(ui, |ui| {
				ui.horizontal_wrapped(|ui| {
					for (label, until) in [
						("Invitations", incident.invites_until),
						("Non-friend DMs", incident.dms_until),
					] {
						if let Some(until) = until.filter(|until| *until > now) {
							ui.label(format!("{label} paused until {}", date(Some(until))));
						}
					}
					if state.can_manage_guild(guild)
						&& ui.button("Review security actions").clicked()
					{
						review = true;
					}
				});
			});
		if let Some(next) = [incident.invites_until, incident.dms_until]
			.into_iter()
			.flatten()
			.filter(|at| *at > now)
			.min()
		{
			ui.ctx()
				.request_repaint_after(std::time::Duration::from_nanos(
					(next - now).min(u64::MAX as i128) as u64,
				));
		}
		review
	}
}
fn resume_incident(before: Incident, invitations: bool, dms: bool, now: i128) -> Incident {
	Incident {
		invites_until: before
			.invites_until
			.filter(|until| !invitations && *until > now),
		dms_until: before.dms_until.filter(|until| !dms && *until > now),
	}
}
fn reapply_configuration(
	before: Configuration,
	draft: Configuration,
	current: Configuration,
) -> Configuration {
	Configuration {
		community: if draft.community != before.community {
			draft.community
		} else {
			current.community
		},
		rules: if draft.rules != before.rules {
			draft.rules
		} else {
			current.rules
		},
		updates: if draft.updates != before.updates {
			draft.updates
		} else {
			current.updates
		},
		alerts: if draft.alerts != before.alerts {
			draft.alerts
		} else {
			current.alerts
		},
	}
}
fn channel_picker(
	ui: &mut egui::Ui,
	state: &State,
	guild: Id,
	label: &str,
	selected: &mut Option<Id>,
) {
	let name = selected
		.and_then(|id| state.channel(id))
		.map(|c| format!("#{}", c.name))
		.unwrap_or_else(|| {
			if selected.is_some() {
				"Unavailable channel".into()
			} else {
				"None".into()
			}
		});
	ui.label(label);
	egui::ComboBox::from_id_salt(("access-channel", label))
		.selected_text(name)
		.show_ui(ui, |ui| {
			ui.selectable_value(selected, None, "None");
			for channel in state.channels.iter().filter(|c| {
				c.guild == Some(guild) && matches!(c.kind, 0 | 5) && state.can_view(c.id)
			}) {
				ui.selectable_value(selected, Some(channel.id), format!("#{}", channel.name));
			}
		});
}
fn date(at: Option<i128>) -> String {
	at.and_then(|at| i64::try_from(at / 1_000_000_000).ok())
		.and_then(|at| local_time::discord_timestamp(at, b'f'))
		.unwrap_or_else(|| "Unknown".into())
}
fn snowflake_date(id: Id) -> String {
	date(Some(
		i128::from((id.0 >> 22) + 1_420_070_400_000) * 1_000_000,
	))
}
fn status_label(status: Status) -> &'static str {
	match status {
		Status::Pending => "Pending",
		Status::Approved => "Approved",
		Status::Rejected => "Rejected",
	}
}
fn duration(seconds: u32) -> &'static str {
	match seconds {
		900 => "15 minutes",
		14400 => "4 hours",
		86400 => "24 hours",
		_ => "1 hour",
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn incident_resume_drops_expired_companion_without_renewing_it() {
		let before = Incident {
			invites_until: Some(200),
			dms_until: Some(50),
		};
		let after = resume_incident(before, true, false, 100);
		assert_eq!(after, Incident::default());
		assert!(after.writable(100));
		let before = Incident {
			invites_until: Some(50),
			dms_until: Some(200),
		};
		assert_eq!(
			resume_incident(before, false, true, 100),
			Incident::default()
		);
		let both_active = Incident {
			invites_until: Some(200),
			dms_until: Some(300),
		};
		assert_eq!(
			resume_incident(both_active, true, false, 100).dms_until,
			Some(300)
		);
	}
	#[test]
	fn explicit_draft_reapplication_keeps_unedited_remote_fields() {
		let before = Configuration {
			community: true,
			rules: Some(Id(1)),
			updates: Some(Id(2)),
			alerts: None,
		};
		let draft = Configuration {
			rules: Some(Id(3)),
			..before
		};
		let current = Configuration {
			updates: Some(Id(4)),
			alerts: Some(Id(5)),
			..before
		};
		assert_eq!(
			reapply_configuration(before, draft, current),
			Configuration {
				rules: Some(Id(3)),
				..current
			}
		);
	}
	#[test]
	fn application_selection_and_confirmation_do_not_survive_an_account_generation_change() {
		let state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let mut editor = AccessUi::default();
		editor.scope(&state, guild);
		editor.selected = Some(Id(10));
		editor.reason = "Private draft".into();
		editor.confirmation = Some(Confirmation::Decision {
			request: Id(10),
			status: Status::Rejected,
		});
		let mut next = state;
		next.generation = next.generation.wrapping_add(1);
		editor.scope(&next, guild);
		assert!(
			editor.selected.is_none() && editor.confirmation.is_none() && editor.reason.is_empty()
		);
	}
}
