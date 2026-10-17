//! Native guide navigation for ordinary members. Chat tasks complete after an actual own message.
use crate::{avatars::Avatars, design, dialog, i18n};
use client_core::{Command, State};
use model::member_guide::Action;
pub(super) fn show(
	ctx: &egui::Context,
	state: &mut State,
	avatars: &mut Avatars,
	commands: &mut Vec<Command>,
) {
	if let Some(command) = state.continue_member_guide() {
		commands.push(command);
	}
	if !state.member_guide.open {
		return;
	}
	let Some(guild) = state.member_guide.guild else {
		return;
	};
	let name = state
		.guild(guild)
		.map_or_else(String::new, |g| g.name.clone());
	let snapshot = state.member_guide.snapshot.clone();
	let mut visit = None;
	let mut close = false;
	let mut reload = false;
	let response = dialog::Dialog::new("member-server-guide", i18n::translate("community-guide"))
		.subtitle(name)
		.width(680.0)
		.show(ctx, |d| {
			d.scroll(190.0, |ui| {
				if let Some(error) = state.member_guide.error {
					dialog::notice(ui, dialog::Level::Error, error);
				}
				if state.member_guide.pending {
					ui.spinner();
				}
				if let Some(snapshot) = &snapshot {
					if !snapshot.guide.enabled {
						ui.label(i18n::translate("member-guide-unavailable"));
						return;
					}
					let welcome = snapshot.guide.message.replace(
						"[@username]",
						state.user.as_ref().map_or("member", |u| &u.name),
					);
					if let Some(hash) = &snapshot.guide.header {
						avatars.show_guide_header(ui, guild, hash, state.demo);
					}
					ui.heading(welcome);
					design::section(ui, "community-new-member-actions", None);
					for action in &snapshot.guide.actions {
						ui.push_id(action.channel, |ui| {
							design::card(ui, |ui| {
								crate::community_admin::guide_icon(
									ui, avatars, action, false, state.demo,
								);
								let done = snapshot.completed.contains(&action.channel);
								ui.strong(format!(
									"{} {}",
									if done { "✓" } else { "○" },
									action.title
								));
								ui.label(&action.description);
								if ui
									.add_enabled(
										state.can_view(action.channel),
										egui::Button::new(i18n::translate(if action.action == 1 {
											"member-guide-chat"
										} else {
											"member-guide-view"
										})),
									)
									.clicked()
								{
									visit = Some(action.channel);
								}
							});
						});
					}
					design::section(ui, "community-resource-channels", None);
					for resource in &snapshot.guide.resources {
						ui.push_id(resource.channel, |ui| {
							design::card(ui, |ui| {
								crate::community_admin::guide_icon(
									ui, avatars, resource, true, state.demo,
								);
								ui.strong(&resource.title);
								ui.label(&resource.description);
								if ui
									.add_enabled(
										state.can_view(resource.channel),
										egui::Button::new(i18n::translate("member-guide-view")),
									)
									.clicked()
								{
									visit = Some(resource.channel);
								}
							});
						});
					}
				} else if !state.member_guide.pending {
					ui.label(i18n::translate("member-guide-unavailable"));
				}
			});
			d.footer(|ui| {
				close =
					dialog::action(ui, "onboarding-show-close", dialog::Action::Neutral).clicked();
				reload = ui
					.add_enabled(
						!state.member_guide.pending,
						egui::Button::new(i18n::translate("community-reload")),
					)
					.clicked();
			});
		});
	if reload && let Some(command) = state.request_member_guide(Action::Load) {
		commands.push(command);
	}
	if let Some(channel) = visit
		&& let Some(command) = state.visit_server_guide_channel(channel)
	{
		commands.push(command);
	}
	if close || response.close {
		state.close_server_guide();
	}
}
