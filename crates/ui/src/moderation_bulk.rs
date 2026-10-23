//! Explicit bounded selection; a submitted request never means every target was banned.
use client_core::State;
use model::{
	Id,
	moderation::{self as m, BanStatus},
	server_admin::{Action, Query},
};
use std::collections::BTreeSet;
#[derive(Default)]
pub(super) struct BulkUi {
	selected: BTreeSet<Id>,
	query: Option<Query>,
	confirmation: Option<Vec<Id>>,
	reason: String,
	delete_seconds: u32,
}
impl BulkUi {
	pub fn select(&mut self, ui: &mut egui::Ui, state: &State, guild: Id, user: Id) {
		let mut selected = self.selected.contains(&user);
		if ui
			.add_enabled(
				state.can_ban_guild_member(guild, user)
					&& !state.server_admin.pending
					&& (selected || self.selected.len() < m::MAX_BULK),
				egui::Checkbox::without_text(&mut selected),
			)
			.on_hover_text("Select member")
			.changed()
		{
			if selected {
				self.selected.insert(user);
			} else {
				self.selected.remove(&user);
			}
		}
	}
	pub fn header(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		query: &Query,
		action: &mut Option<Action>,
	) {
		if self.query.as_ref() != Some(query) {
			self.selected.clear();
			self.confirmation = None;
			self.query = Some(query.clone());
		}
		let available = !state.server_admin.pending && !state.server_admin.needs_refresh;
		let previous_finished = state
			.server_admin
			.bulk_report
			.as_ref()
			.is_none_or(|report| {
				report
					.users
					.iter()
					.all(|(_, status)| matches!(status, BanStatus::Banned | BanStatus::Failed))
			});

		if state.can_manage_guild(guild)
			&& state.guild_permission(guild, model::permissions::BAN_MEMBERS)
		{
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(available, egui::Button::new("Select this page"))
					.clicked()
				{
					self.selected = state
						.server_admin
						.members
						.iter()
						.flat_map(|page| page.items.iter())
						.filter(|member| state.can_ban_guild_member(guild, member.user.id))
						.map(|member| member.user.id)
						.take(m::MAX_BULK)
						.collect();
				}
				if ui.button("Clear selection").clicked() {
					self.selected.clear();
				}
				ui.label(format!("{} selected", self.selected.len()));
				let allowed = available
					&& previous_finished
					&& !self.selected.is_empty()
					&& self
						.selected
						.iter()
						.all(|id| state.can_ban_guild_member(guild, *id));
				if ui
					.add_enabled(allowed, egui::Button::new("Ban selected members…"))
					.clicked()
				{
					self.confirmation = Some(self.selected.iter().copied().collect());
				}
			});
		}
		let mut dismiss = false;
		if let Some(report) = &state.server_admin.bulk_report {
			ui.separator();
			ui.strong(if report.accepted {
				"Bulk moderation results"
			} else {
				"Bulk moderation request pending"
			});
			ui.weak("Accepted requests may still be processing. Refresh checks current bans without submitting another ban.");
			egui::ScrollArea::vertical()
				.id_salt("bulk-results")
				.max_height(180.0)
				.show(ui, |ui| {
					for (user, status) in &report.users {
						let name = state
							.server_admin
							.members
							.as_ref()
							.and_then(|page| page.items.iter().find(|row| row.user.id == *user))
							.map(|row| row.nick.as_deref().unwrap_or(&row.user.name).to_owned())
							.unwrap_or_else(|| user.to_string());
						ui.label(format!(
							"{name} · {}",
							match status {
								BanStatus::Pending => "Awaiting completion",
								BanStatus::Banned => "Banned",
								BanStatus::Failed => "Failed",
								BanStatus::NotBanned => "Not currently banned",
								BanStatus::Unknown => "Could not verify",
							}
						));
					}
				});
			if ui
				.add_enabled(
					!state.server_admin.pending && state.can_view_guild_bans(guild),
					egui::Button::new("Refresh results"),
				)
				.clicked()
			{
				*action = Some(Action::Moderation(m::Action::BulkRefresh {
					users: report.users.iter().map(|(id, _)| *id).collect(),
				}));
			}
			if !state.server_admin.pending
				&& report
					.users
					.iter()
					.all(|(_, status)| *status != BanStatus::Pending)
			{
				dismiss = ui.button("Done").clicked();
			}
		}
		if dismiss {
			state.dismiss_bulk_moderation();
		}
		if let Some(users) = self.confirmation.clone() {
			let mut confirm = false;
			let allowed = available
				&& previous_finished
				&& users
					.iter()
					.all(|user| state.can_ban_guild_member(guild, *user));
			let response = crate::dialog::Dialog::new("bulk-ban-confirm", "Ban selected members?")
				.danger()
				.width(470.0)
				.show(ui.ctx(), |body| {
					body.content(|ui| {
						ui.label(format!(
							"Review these {} members. They will be removed and cannot rejoin until unbanned.",
							users.len()
						));
						egui::ScrollArea::vertical()
							.max_height(180.0)
							.show(ui, |ui| {
								for user in &users {
									let member =
										state.server_admin.members.as_ref().and_then(|page| {
											page.items.iter().find(|row| row.user.id == *user)
										});
									ui.label(
										member
											.map(|member| {
												format!(
													"{} · {user}",
													member
														.nick
														.as_deref()
														.unwrap_or(&member.user.name)
												)
											})
											.unwrap_or_else(|| user.to_string()),
									);
								}
							});
						egui::ComboBox::from_id_salt("bulk-delete-window")
							.selected_text(if self.delete_seconds == 0 {
								"Keep message history".to_owned()
							} else {
								format!("Delete previous {} hours", self.delete_seconds / 3600)
							})
							.show_ui(ui, |ui| {
								for (seconds, label) in [
									(0, "Keep message history"),
									(3600, "Previous hour"),
									(86400, "Previous day"),
									(604800, "Previous week"),
								] {
									ui.selectable_value(&mut self.delete_seconds, seconds, label);
								}
							});
						crate::server_automod::reason(ui, &mut self.reason);
						if !allowed {
							ui.weak(
								"Selection or permissions changed. Close this confirmation and review the members again.",
							);
						}
					});
					body.footer(|ui| {
						confirm = ui
							.add_enabled(allowed, egui::Button::new("Ban members"))
							.clicked();
					});
				});
			if response.close {
				self.confirmation = None;
			}
			if confirm {
				*action = Some(Action::Moderation(m::Action::BulkBan {
					users,
					delete_seconds: self.delete_seconds,
					reason: self.reason.clone(),
				}));
				self.confirmation = None;
				self.selected.clear();
			}
		}
	}
}
