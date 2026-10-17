use crate::{design, dialog, i18n};
use client_core::{Command, State};
use model::{
	Id,
	moderation::{Action, Change},
	server_admin,
};

#[derive(Clone, Copy)]
enum Confirmation {
	Change(Id, Change),
	Unban(Id),
}
#[derive(Default)]
pub(super) struct ModerationUi {
	scope: Option<(u64, Id)>,
	target: String,
	reason: String,
	confirmation: Option<Confirmation>,
	submitted: bool,
	ban_seconds: u32,
	timeout_seconds: u32,
}
impl ModerationUi {
	pub fn has_changes(&self) -> bool {
		self.confirmation.is_some()
	}
	pub fn select(&mut self, generation: u64, guild: Id, user: Id) {
		*self = Self {
			scope: Some((generation, guild)),
			target: user.to_string(),
			timeout_seconds: 600,
			..Default::default()
		};
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
				timeout_seconds: 600,
				..Default::default()
			};
		}
		if !state.can_open_moderation(guild) {
			return;
		}
		if self.submitted && !state.server_admin.pending {
			self.submitted = false;
			if state.server_admin.error.is_none() {
				self.confirmation = None;
				self.reason.clear();
			}
		}
		let available = !state.server_admin.pending && (state.demo || state.gateway_connected);
		let mut action = None;
		ui.heading(i18n::translate("moderation-title"));
		if let Some(error) = state.server_admin.error {
			ui.colored_label(design::palette(ui).danger, error);
		}
		if state.server_admin.pending {
			ui.spinner();
		}
		ui.horizontal_wrapped(|ui| {
			ui.label(i18n::translate("moderation-member"));
			ui.add(
				egui::TextEdit::singleline(&mut self.target)
					.char_limit(20)
					.desired_width(200.0),
			);
			if ui
				.add_enabled(
					available && self.target.parse::<Id>().is_ok_and(|user| user.0 != 0),
					egui::Button::new(i18n::translate("moderation-lookup")),
				)
				.clicked()
			{
				action = self.target.parse().ok().map(Action::Member);
			}
		});
		if let Some(channel) = state.selected.filter(|id| {
			state
				.channel(*id)
				.is_some_and(|row| row.guild == Some(guild))
		}) {
			ui.menu_button(i18n::translate("moderation-choose-member"), |ui| {
				egui::ScrollArea::vertical()
					.max_height(220.0)
					.show(ui, |ui| {
						for user in crate::mentions::known_users(state, channel)
							.into_iter()
							.filter(|user| {
								state.user.as_ref().is_none_or(|actor| actor.id != user.id)
							}) {
							if ui
								.add_enabled(available, egui::Button::new(&user.name))
								.clicked()
							{
								self.target = user.id.to_string();
								action = Some(Action::Member(user.id));
								ui.close();
							}
						}
					});
			});
		}
		ui.weak(i18n::translate("moderation-lookup-help"));
		if let Some(member) = state
			.server_admin
			.moderation_target
			.as_ref()
			.filter(|member| self.target.parse::<Id>() == Ok(member.user.id))
		{
			ui.separator();
			ui.strong(&member.user.name);
			if let Some(until) = member
				.timeout_until
				.and_then(|at| crate::local_time::discord_timestamp((at / 1000) as i64, b'f'))
			{
				ui.label(format!(
					"{} {until}",
					i18n::translate("moderation-timeout-until")
				));
			}
			ui.add_enabled_ui(available && !state.server_admin.needs_refresh, |ui| {
				ui.horizontal_wrapped(|ui| {
					egui::ComboBox::from_id_salt("moderation-timeout")
						.selected_text(duration(self.timeout_seconds))
						.show_ui(ui, |ui| {
							for seconds in [60, 300, 600, 3600, 86400, 604800, 2_419_200] {
								ui.selectable_value(
									&mut self.timeout_seconds,
									seconds,
									duration(seconds),
								);
							}
						});
					if ui
						.add_enabled(
							state.can_timeout_guild_member(guild, member.user.id),
							egui::Button::new(i18n::translate("moderation-timeout")),
						)
						.clicked()
					{
						let seconds = time::OffsetDateTime::now_utc().unix_timestamp() as i128
							+ i128::from(self.timeout_seconds);
						self.confirmation = Some(Confirmation::Change(
							member.user.id,
							Change::Timeout {
								until: Some(seconds * 1_000_000_000),
							},
						));
					}
					if member.timeout_until.is_some()
						&& ui
							.add_enabled(
								state.can_timeout_guild_member(guild, member.user.id),
								egui::Button::new(i18n::translate("moderation-remove-timeout")),
							)
							.clicked()
					{
						self.confirmation = Some(Confirmation::Change(
							member.user.id,
							Change::Timeout { until: None },
						));
					}
				});
				ui.horizontal_wrapped(|ui| {
					egui::ComboBox::from_id_salt("moderation-ban-delete")
						.selected_text(if self.ban_seconds == 0 {
							i18n::translate("moderation-keep-messages")
						} else {
							duration(self.ban_seconds)
						})
						.show_ui(ui, |ui| {
							ui.selectable_value(
								&mut self.ban_seconds,
								0,
								i18n::translate("moderation-keep-messages"),
							);
							for seconds in [3600, 86400, 604800] {
								ui.selectable_value(
									&mut self.ban_seconds,
									seconds,
									duration(seconds),
								);
							}
						});
					if ui
						.add_enabled(
							state.can_ban_guild_member(guild, member.user.id),
							egui::Button::new(i18n::translate("moderation-ban")),
						)
						.clicked()
					{
						self.confirmation = Some(Confirmation::Change(
							member.user.id,
							Change::Ban {
								delete_seconds: self.ban_seconds,
							},
						));
					}
					if ui
						.add_enabled(
							state.can_kick_guild_member(guild, member.user.id),
							egui::Button::new(i18n::translate("moderation-kick")),
						)
						.clicked()
					{
						self.confirmation =
							Some(Confirmation::Change(member.user.id, Change::Kick));
					}
				});
			});
		}
		if state.guild_permission(guild, model::permissions::BAN_MEMBERS) {
			ui.separator();
			ui.heading(i18n::translate("moderation-bans"));
			if ui
				.add_enabled(
					available,
					egui::Button::new(i18n::translate("moderation-refresh-bans")),
				)
				.clicked()
			{
				action = Some(Action::Bans { after: None });
			}
			if let Some(page) = &state.server_admin.bans {
				if page.items.is_empty() {
					ui.weak(i18n::translate("moderation-no-bans"));
				}
				for ban in &page.items {
					ui.push_id(ban.user.id.0, |ui| {
						ui.separator();
						ui.strong(&ban.user.name);
						if let Some(reason) = &ban.reason {
							ui.label(reason);
						}
						if ui
							.add_enabled(
								available && !state.server_admin.needs_refresh,
								egui::Button::new(i18n::translate("moderation-unban")),
							)
							.clicked()
						{
							self.confirmation = Some(Confirmation::Unban(ban.user.id));
						}
					});
				}
				if let Some(after) = page.next
					&& ui
						.add_enabled(
							available,
							egui::Button::new(i18n::translate("moderation-next-bans")),
						)
						.clicked()
				{
					action = Some(Action::Bans { after: Some(after) });
				}
			}
		}
		if let Some(confirmation) = self.confirmation {
			let user = match confirmation {
				Confirmation::Change(user, _) | Confirmation::Unban(user) => user,
			};
			let name = state
				.server_admin
				.moderation_target
				.as_ref()
				.filter(|member| member.user.id == user)
				.map(|member| member.user.name.clone())
				.or_else(|| {
					state
						.server_admin
						.bans
						.as_ref()
						.and_then(|page| page.items.iter().find(|row| row.user.id == user))
						.map(|ban| ban.user.name.clone())
				})
				.unwrap_or_else(|| user.to_string());
			let title = match confirmation {
				Confirmation::Unban(_) => "moderation-confirm-unban",
				Confirmation::Change(_, Change::Kick) => "moderation-confirm-kick",
				Confirmation::Change(_, Change::Ban { .. }) => "moderation-confirm-ban",
				Confirmation::Change(_, Change::Timeout { .. }) => "moderation-confirm-timeout",
			};
			let request = match confirmation {
				Confirmation::Unban(user) => Action::Unban {
					user,
					reason: self.reason.clone(),
				},
				Confirmation::Change(user, change) => Action::Change {
					user,
					change,
					reason: self.reason.clone(),
				},
			};
			let allowed = available
				&& !state.server_admin.needs_refresh
				&& state.server_admin_action_allowed(
					guild,
					&server_admin::Action::Moderation(request.clone()),
				);
			let mut confirm = false;
			let response = dialog::Dialog::new("moderation-confirm", i18n::translate(title))
				.danger()
				.width(440.0)
				.show(ui.ctx(), |body| {
					body.content(|ui| {
						ui.strong(&name);
						ui.label(i18n::translate("moderation-confirm-description"));
						crate::server_automod::reason(ui, &mut self.reason);
					});
					body.footer(|ui| {
						confirm = ui
							.add_enabled(
								allowed,
								egui::Button::new(i18n::translate("moderation-confirm")),
							)
							.clicked();
					});
				});
			if response.close && !state.server_admin.pending {
				self.confirmation = None;
			}
			if confirm {
				action = Some(match confirmation {
					Confirmation::Unban(user) => Action::Unban {
						user,
						reason: self.reason.clone(),
					},
					Confirmation::Change(user, change) => Action::Change {
						user,
						change,
						reason: self.reason.clone(),
					},
				});
			}
		}
		if let Some(action) = action {
			let write = action.write();
			if let Some(command) =
				state.request_server_admin(guild, server_admin::Action::Moderation(action))
			{
				self.submitted = write;
				commands.push(command);
			}
		}
	}
}
fn duration(seconds: u32) -> String {
	let (count, key) = if seconds >= 86400 {
		(seconds / 86400, "moderation-days")
	} else if seconds >= 3600 {
		(seconds / 3600, "moderation-hours")
	} else {
		(seconds / 60, "moderation-minutes")
	};
	format!("{count} {}", i18n::translate(key))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn offline_moderation_confirmation_can_be_cancelled_without_a_write() {
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
		assert!(state.can_open_moderation(guild));
		let mut editor = ModerationUi {
			scope: Some((state.generation, guild)),
			confirmation: Some(Confirmation::Change(Id(77), Change::Kick)),
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
		assert!(editor.confirmation.is_none() && commands.is_empty());
	}
}
