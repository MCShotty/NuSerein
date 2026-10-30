//! Native guide navigation for ordinary members. Chat tasks complete after an actual own message.
use crate::{avatars::Avatars, design, dialog, i18n};
use client_core::{Command, State};
use model::{Id, MemberSlot, User, member_guide::Action};

/// Resolve only resident users. Guild nicknames must belong to this Guide's guild.
fn known_author(state: &State, guild: Id, author: Id) -> Option<(&User, &str)> {
	if let Some(member) = state
		.members
		.as_ref()
		.filter(|list| list.guild == Some(guild))
		.and_then(|list| {
			list.slots.iter().flatten().find_map(|slot| match slot {
				MemberSlot::Person(member) if member.user.id == author => Some(member),
				_ => None,
			})
		}) {
		return Some((
			&member.user,
			member
				.nick
				.as_deref()
				.filter(|nick| !nick.is_empty())
				.unwrap_or_else(|| state.user_display_name(&member.user)),
		));
	}
	if state.server_admin.guild == Some(guild)
		&& let Some(member) = state
			.server_admin
			.members
			.as_ref()
			.and_then(|members| members.items.iter().find(|member| member.user.id == author))
	{
		return Some((
			&member.user,
			member
				.nick
				.as_deref()
				.filter(|nick| !nick.is_empty())
				.unwrap_or_else(|| state.user_display_name(&member.user)),
		));
	}
	let user = state
		.user
		.as_ref()
		.filter(|user| user.id == author)
		.or_else(|| state.known_profile(author).map(|profile| &profile.user))
		.or_else(|| state.friend(author))?;
	Some((user, state.user_display_name(user)))
}

fn welcome_authors(
	ui: &mut egui::Ui,
	state: &State,
	guild: Id,
	authors: &[Id],
	avatars: &mut Avatars,
) {
	if authors.is_empty() {
		return;
	}
	ui.label(i18n::translate("member-guide-welcomed-by"));
	ui.horizontal_wrapped(|ui| {
		for &author in authors.iter().take(10) {
			ui.push_id(author, |ui| {
				ui.horizontal(|ui| {
					if let Some((user, name)) = known_author(state, guild, author) {
						avatars.show_quiet(ui, user, 32.0, state.demo);
						ui.label(name);
					} else {
						let label = i18n::translate("member-guide-unknown-author");
						avatars.show_icon(ui, None, 32.0, state.demo, &label);
						ui.label(label).on_hover_text(author.to_string());
					}
				});
			});
		}
	});
}

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
					welcome_authors(ui, state, guild, &snapshot.guide.authors, avatars);
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

#[cfg(test)]
mod tests {
	use super::*;

	fn member_list(user: User, guild: Id, nick: Option<&str>) -> model::MemberList {
		model::MemberList {
			missing_identity: vec![],
			guild: Some(guild),
			channel: Id(20),
			request: 0,
			start: 0,
			slots: vec![Some(MemberSlot::Person(model::Member {
				user,
				nick: nick.map(str::to_owned),
				roles: vec![],
				status: None,
				custom_status: None,
				activities: vec![],
				clients: Default::default(),
			}))],
			total: 1,
			lazy: false,
			freshness: model::Freshness::Fresh,
			groups: vec![],
			ranges: vec![],
		}
	}

	#[test]
	fn welcome_authors_use_resident_guild_names_without_cross_guild_nicknames() {
		let mut state = test_support::demo_state();
		let own = state.user.as_ref().unwrap().clone();
		assert_eq!(known_author(&state, Id(10), own.id).unwrap().1, own.name);
		let mut author = own.clone();
		author.id = Id(999999);
		author.name = "Known welcome author".into();
		author.avatar = Some("a".repeat(32));
		state.members = Some(member_list(author.clone(), Id(10), Some("Guild host")));
		let (user, name) = known_author(&state, Id(10), author.id).unwrap();
		assert_eq!(name, "Guild host");
		assert_eq!(user.avatar, author.avatar);
		assert!(known_author(&state, Id(11), author.id).is_none());
		state.members.as_mut().unwrap().slots[0] = Some(MemberSlot::Person(model::Member {
			user: author.clone(),
			nick: Some(String::new()),
			roles: vec![],
			status: None,
			custom_status: None,
			activities: vec![],
			clients: Default::default(),
		}));
		assert_eq!(
			known_author(&state, Id(10), author.id).unwrap().1,
			author.name
		);
		state.members = None;
		state.server_admin.guild = Some(Id(10));
		state.server_admin.members = Some(model::server_admin::Members {
			items: vec![model::server_admin::Member {
				user: author.clone(),
				nick: Some("Administration host".into()),
				roles: vec![],
				joined_at: None,
				join_source: None,
				invite_code: None,
				flags: None,
				unusual_dm_until: None,
				timeout_until: None,
			}],
			..Default::default()
		});
		assert_eq!(
			known_author(&state, Id(10), author.id).unwrap().1,
			"Administration host"
		);
		assert!(known_author(&state, Id(11), author.id).is_none());
	}

	#[test]
	fn guide_unknown_authors_render_without_user_fetches_or_mutations() {
		let ctx = egui::Context::default();
		design::apply(&ctx);
		let mut state = test_support::demo_state();
		let own = state.user.as_ref().unwrap().id;
		state.member_guide.guild = Some(Id(10));
		state.member_guide.open = true;
		state.member_guide.snapshot = Some(model::member_guide::Snapshot {
			guild: Id(10),
			user: own,
			guide: model::community_admin::Guide {
				enabled: true,
				message: "Welcome, [@username]".into(),
				authors: vec![own, Id(999999)],
				..Default::default()
			},
			completed: vec![],
			header_known: false,
		});
		let mut avatars = Avatars::default();
		let mut commands = vec![];
		for _ in 0..2 {
			ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(1000.0, 900.0),
					)),
					..Default::default()
				},
				|ui| show(ui.ctx(), &mut state, &mut avatars, &mut commands),
			)
			.drop_without_applying_deltas();
		}
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1000.0, 900.0),
				)),
				..Default::default()
			},
			|ui| show(ui.ctx(), &mut state, &mut avatars, &mut commands),
		);
		fn text(shape: &egui::Shape, labels: &mut Vec<String>) {
			match shape {
				egui::Shape::Text(text) => labels.push(text.galley.job.text.clone()),
				egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| text(shape, labels)),
				_ => {}
			}
		}
		let mut labels = vec![];
		for shape in &output.shapes {
			text(&shape.shape, &mut labels);
		}
		assert!(
			labels
				.iter()
				.any(|label| label == &state.user.as_ref().unwrap().name)
		);
		assert!(
			labels
				.iter()
				.any(|label| label == &i18n::translate("member-guide-unknown-author"))
		);
		assert!(commands.is_empty());
		assert!(avatars.take_requests().is_empty());
		assert!(known_author(&state, Id(10), Id(999999)).is_none());
		output.drop_without_applying_deltas();
	}
}
