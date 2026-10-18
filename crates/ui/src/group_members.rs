//! Explicit group invitations and owner-only removals, with a fresh bounded member snapshot.
use client_core::{Command, State};
use model::Id;

#[derive(Default)]
pub(super) struct Dialog {
	target: Option<(u64, Id)>,
	filter: String,
	requested: bool,
	remove: Option<Id>,
	transfer: bool,
}
impl Dialog {
	pub fn open(&mut self, state: &State, channel: Id) {
		*self = Self {
			target: Some((state.generation, channel)),
			..Self::default()
		};
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		let Some((generation, channel)) = self.target else {
			return;
		};
		if state.generation != generation || !state.is_group_dm(channel) {
			*self = Self::default();
			return;
		}
		if !self.requested
			&& let Some(command) = state.request_group_members(channel)
		{
			commands.push(command);
			self.requested = true;
		}
		let mut close = false;
		let mut add = None;
		let mut remove = None;
		let mut reload = false;
		let busy = state.group_action_pending();
		let connected = state.demo
			|| (state.gateway_connected
				&& state.auth == client_core::auth::AuthState::Authenticated);
		let response = crate::dialog::Dialog::new(
			"group-members",
			crate::i18n::translate("group-members-title"),
		)
		.width(440.0)
		.show(ctx, |dialog| {
			dialog.scroll(160.0, |ui| {
				if let Some(user) = self.remove {
					let name = state
						.channel(channel)
						.and_then(|group| group.recipients.iter().find(|member| member.id == user))
						.map_or_else(
							|| user.to_string(),
							|member| state.user_display_name(member).to_owned(),
						);
					ui.label(crate::i18n::translate_args(
						if self.transfer {
							"group-members-transfer-confirm"
						} else {
							"group-members-remove-confirm"
						},
						&[("name", &name)],
					));
					return;
				}
				if let Some(status) = state.group_action_status(channel) {
					ui.weak(crate::i18n::translate_if_key(status));
				}
				if busy {
					ui.spinner();
				}
				if state.group_members(channel).is_none() {
					ui.weak(crate::i18n::translate("group-members-refresh-help"));
				}
				if ui
					.add_enabled(
						!busy && connected,
						egui::Button::new(crate::i18n::translate("group-members-refresh")),
					)
					.clicked()
				{
					reload = true;
				}
				ui.separator();
				if let Some(group) = state.channel(channel) {
					ui.label(crate::i18n::translate_args(
						"group-members-count",
						&[("count", &(group.recipients.len() + 1).to_string())],
					));
					if let Some(user) = &state.user {
						ui.horizontal_wrapped(|ui| {
							ui.label(state.user_display_name(user));
							if state
								.group_members(channel)
								.is_some_and(|members| members.owner == user.id)
							{
								ui.weak(crate::i18n::translate("group-members-owner"));
							}
						});
					}
					for member in &group.recipients {
						ui.push_id(("group-member", member.id), |ui| {
							ui.horizontal_wrapped(|ui| {
								ui.label(state.user_display_name(member));
								if state
									.group_members(channel)
									.is_some_and(|members| members.owner == member.id)
								{
									ui.weak(crate::i18n::translate("group-members-owner"));
								}
								if state.can_remove_group_member(channel, member.id)
									&& ui
										.add_enabled(
											!busy && connected,
											egui::Button::new(crate::i18n::translate(
												"group-members-remove",
											)),
										)
										.clicked()
								{
									self.remove = Some(member.id);
									self.transfer = false;
								}
								if state.can_remove_group_member(channel, member.id)
									&& ui
										.add_enabled(
											!busy && connected,
											egui::Button::new(crate::i18n::translate(
												"group-members-transfer",
											)),
										)
										.clicked()
								{
									self.remove = Some(member.id);
									self.transfer = true;
								}
							})
						});
					}
				}
				ui.separator();
				ui.label(crate::i18n::translate("group-members-add-heading"));
				ui.add(
					egui::TextEdit::singleline(&mut self.filter)
						.char_limit(64)
						.hint_text(crate::i18n::translate("group-members-search")),
				);
				let query = self.filter.trim().to_lowercase();
				let mut found = false;
				egui::ScrollArea::vertical()
					.max_height(180.0)
					.show(ui, |ui| {
						for friend in state
							.friends()
							.filter(|friend| {
								state.can_add_group_member(channel, friend.id)
									&& (query.is_empty()
										|| state
											.user_display_name(friend)
											.to_lowercase()
											.contains(&query))
							})
							.take(20)
						{
							found = true;
							ui.push_id(("group-invite", friend.id), |ui| {
								ui.horizontal_wrapped(|ui| {
									ui.label(state.user_display_name(friend));
									if ui
										.add_enabled(
											!busy && connected,
											egui::Button::new(crate::i18n::translate(
												"group-members-add",
											)),
										)
										.clicked()
									{
										add = Some(friend.id);
									}
								})
							});
						}
					});
				if !found {
					ui.weak(crate::i18n::translate("group-members-no-friends"));
				}
			});
			dialog.footer(|ui| {
				if let Some(user) = self.remove {
					if ui
						.add_enabled(
							!busy && connected && state.can_remove_group_member(channel, user),
							egui::Button::new(crate::i18n::translate(if self.transfer {
								"group-members-transfer"
							} else {
								"group-members-remove"
							})),
						)
						.clicked()
					{
						remove = Some(user);
					}
					if ui
						.button(crate::i18n::translate("group-members-cancel"))
						.clicked()
					{
						self.remove = None;
					}
				} else if ui
					.button(crate::i18n::translate("group-members-done"))
					.clicked()
				{
					close = true;
				}
			});
		});
		if reload && let Some(command) = state.request_group_members(channel) {
			commands.push(command);
		}
		if let Some(user) = add
			&& let Some(command) = state.add_group_member(channel, user)
		{
			commands.push(command);
		}
		if let Some(user) = remove
			&& let Some(command) = if self.transfer {
				state.transfer_group_owner(channel, user)
			} else {
				state.remove_group_member(channel, user)
			} {
			commands.push(command);
			self.remove = None;
		}
		if close || response.close {
			*self = Self::default();
		}
	}
}

/// One friend-selection draft, with no side effects until the explicit Create action.
#[derive(Default)]
pub(super) struct NewGroup {
	context: Option<(u64, Option<Id>)>,
	filter: String,
	users: Vec<Id>,
	submitted: Option<u64>,
}
impl NewGroup {
	pub fn open(&mut self, state: &mut State) {
		*self = Self {
			context: Some((state.generation, state.selected)),
			..Self::default()
		};
		state.clear_group_action_result(Id(0));
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		let Some((generation, view)) = self.context else {
			return;
		};
		if generation != state.generation || view != state.selected {
			*self = Self::default();
			return;
		}
		if let Some(request) = self.submitted
			&& let Some(success) = state.group_action_completed(Id(0), request)
		{
			self.submitted = None;
			if success {
				let created = state.created_group(request);
				*self = Self::default();
				if let Some(channel) = created
					&& let Some(command) = state.select(channel)
				{
					commands.push(command);
				}
				return;
			}
		}
		let busy = state.group_action_pending() || self.submitted.is_some();
		let connected = state.demo
			|| (state.auth == client_core::auth::AuthState::Authenticated
				&& state.gateway_connected);
		let mut close = false;
		let response = crate::dialog::Dialog::new(
			"create-group",
			crate::i18n::translate("group-create-title"),
		)
		.subtitle(crate::i18n::translate("group-create-detail"))
		.width(440.0)
		.show(ctx, |dialog| {
			dialog.scroll(160.0, |ui| {
				ui.add(
					egui::TextEdit::singleline(&mut self.filter)
						.char_limit(64)
						.hint_text(crate::i18n::translate("group-members-search")),
				);
				let query = self.filter.trim().to_lowercase();
				ui.weak(crate::i18n::translate_args(
					"group-create-count",
					&[("count", &self.users.len().to_string())],
				));
				// Keep selected friends visible regardless of the search and show a bounded
				// slice of other matches. Only IDs and one 64-character filter are retained.
				for user in self.users.clone() {
					let name = state
						.friends()
						.find(|friend| friend.id == user)
						.map(|friend| state.user_display_name(friend).to_owned())
						.unwrap_or_else(|| user.to_string());
					let mut selected = true;
					if ui
						.push_id(("selected-group-friend", user), |ui| {
							ui.add_enabled(!busy, egui::Checkbox::new(&mut selected, name))
						})
						.inner
						.changed()
					{
						self.users.retain(|id| *id != user);
					}
				}
				ui.separator();
				for friend in state
					.friends()
					.filter(|friend| {
						!self.users.contains(&friend.id)
							&& state.user_blocked(friend.id) == Some(false)
							&& (query.is_empty()
								|| state
									.user_display_name(friend)
									.to_lowercase()
									.contains(&query))
					})
					.take(40)
					.collect::<Vec<_>>()
				{
					let mut selected = false;
					if ui
						.push_id(("new-group-friend", friend.id), |ui| {
							ui.add_enabled(
								!busy && self.users.len() < 9,
								egui::Checkbox::new(&mut selected, state.user_display_name(friend)),
							)
						})
						.inner
						.changed() && selected
					{
						self.users.push(friend.id);
					}
				}
				if let Some(status) = state.group_action_status(Id(0)) {
					ui.weak(crate::i18n::translate_if_key(status));
				}
			});
			dialog.footer(|ui| {
				if ui
					.add_enabled(
						!busy && connected && state.can_create_group(&self.users),
						egui::Button::new(crate::i18n::translate("group-create-title")),
					)
					.clicked() && let Some(command) = state.create_group(self.users.clone())
				{
					if let Command::GroupAction { request, .. } = &command {
						self.submitted = Some(*request);
					}
					commands.push(command);
				}
				close = ui
					.button(crate::i18n::translate("group-members-cancel"))
					.clicked();
			});
		});
		if close || response.close {
			*self = Self::default();
		}
	}
}
