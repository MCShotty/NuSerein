//! Deliberate thread member changes; reads and writes share existing session correlation.
use client_core::{Command, State, channel_actions::Action};
use model::Id;

#[derive(Default)]
pub(super) struct Dialog {
	target: Option<(u64, Id)>,
	requested: bool,
	submitted: Option<u64>,
	filter: String,
	user_id: String,
	remove: Option<Id>,
}
impl Dialog {
	pub fn open(&mut self, state: &State, thread: Id) {
		*self = Self {
			target: Some((state.generation, thread)),
			..Self::default()
		};
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		let Some((generation, thread)) = self.target else {
			return;
		};
		if state.generation != generation
			|| state.selected != Some(thread)
			|| !state.is_thread_channel(thread)
			|| !state.can_view(thread)
		{
			*self = Self::default();
			return;
		}
		let connected = state.demo
			|| (state.auth == client_core::auth::AuthState::Authenticated
				&& state.gateway_connected);
		if connected && !self.requested {
			if let Some(command) = state.request_members() {
				commands.push(command);
				self.requested = true;
			}
			if state.post_details(thread).is_none()
				&& let Some(command) = state.request_channel_action(thread, Action::PostLoad)
			{
				commands.push(command);
			}
		}
		if let Some(request) = self.submitted
			&& let Some(success) = state.channel_action_completed(thread, request)
		{
			self.submitted = None;
			if success && let Some(command) = state.request_members() {
				commands.push(command);
			}
		}
		let busy = state.channel_action_pending() || self.submitted.is_some();
		let mut reload = false;
		let mut add = None;
		let mut remove = None;
		let mut close = false;
		let response = crate::dialog::Dialog::new(
			"thread-members",
			crate::i18n::translate("thread-members-title"),
		)
		.width(460.0)
		.show(ctx, |dialog| {
			dialog.scroll(170.0, |ui| {
				if let Some(user) = self.remove {
					ui.label(crate::i18n::translate_args(
						"thread-members-remove-confirm",
						&[("user", &user.to_string())],
					));
					return;
				}
				if let Some(status) = state.channel_action_status(thread) {
					ui.weak(crate::i18n::translate_if_key(status));
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
				ui.add(
					egui::TextEdit::singleline(&mut self.filter)
						.char_limit(64)
						.hint_text(crate::i18n::translate("thread-members-search")),
				);
				let query = self.filter.trim().to_lowercase();
				let mut found = false;
				egui::ScrollArea::vertical()
					.max_height(180.0)
					.show(ui, |ui| {
						if let Some(members) = state
							.members
							.as_ref()
							.filter(|members| members.channel == thread)
						{
							if members.freshness == model::Freshness::Loading {
								ui.spinner();
							}
							for member in members
								.slots
								.iter()
								.flatten()
								.filter_map(|slot| {
									if let model::MemberSlot::Person(member) = slot {
										Some(member)
									} else {
										None
									}
								})
								.filter(|member| {
									query.is_empty()
										|| state
											.user_display_name(&member.user)
											.to_lowercase()
											.contains(&query)
								}) {
								found = true;
								ui.push_id(("thread-member", member.user.id), |ui| {
									ui.horizontal_wrapped(|ui| {
										ui.label(state.user_display_name(&member.user));
										if state.can_remove_thread_member(thread, member.user.id)
											&& ui
												.add_enabled(
													!busy && connected,
													egui::Button::new(crate::i18n::translate(
														"group-members-remove",
													)),
												)
												.clicked()
										{
											self.remove = Some(member.user.id);
										}
									})
								});
							}
						}
					});
				if !found {
					ui.weak(crate::i18n::translate("thread-members-empty"));
				}
				ui.separator();
				ui.weak(crate::i18n::translate("thread-members-invite-help"));
				ui.add(
					egui::TextEdit::singleline(&mut self.user_id)
						.char_limit(20)
						.hint_text(crate::i18n::translate("thread-members-user-id")),
				);
				let user = self
					.user_id
					.parse::<u64>()
					.ok()
					.filter(|id| *id != 0)
					.map(Id);
				if ui
					.add_enabled(
						!busy
							&& connected && user
							.is_some_and(|user| state.can_add_thread_member(thread, user)),
						egui::Button::new(crate::i18n::translate("thread-members-invite")),
					)
					.clicked()
				{
					add = user;
				}
				for friend in state
					.friends()
					.filter(|friend| {
						state.can_add_thread_member(thread, friend.id)
							&& (query.is_empty()
								|| state
									.user_display_name(friend)
									.to_lowercase()
									.contains(&query))
					})
					.take(10)
				{
					ui.push_id(("thread-invite", friend.id), |ui| {
						ui.horizontal_wrapped(|ui| {
							ui.label(state.user_display_name(friend));
							if ui
								.add_enabled(
									!busy && connected,
									egui::Button::new(crate::i18n::translate(
										"thread-members-invite",
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
			dialog.footer(|ui| {
				if let Some(user) = self.remove {
					if ui
						.add_enabled(
							!busy && connected && state.can_remove_thread_member(thread, user),
							egui::Button::new(crate::i18n::translate("group-members-remove")),
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
		if reload {
			if let Some(command) = state.request_members() {
				commands.push(command);
			}
			if let Some(command) = state.request_channel_action(thread, Action::PostLoad) {
				commands.push(command);
			}
		}
		let action = add
			.map(Action::PostAddMember)
			.or_else(|| remove.map(Action::PostRemoveMember));
		if let Some(action) = action
			&& let Some(command) = state.request_channel_action(thread, action)
		{
			if let Command::ChannelAction { request, .. } = &command {
				self.submitted = Some(*request);
			}
			commands.push(command);
			self.remove = None;
		}
		if close || response.close {
			*self = Self::default();
		}
	}
}
