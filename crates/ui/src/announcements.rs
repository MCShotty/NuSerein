//! Follow an announcement channel into one explicitly selected, manageable text channel.
use client_core::{Command, State, channel_actions::Action};
use model::Id;

#[derive(Default)]
pub(super) struct FollowDialog {
	source: Option<(u64, Id)>,
	destination: Option<Id>,
	query: String,
	submitted: Option<u64>,
}
impl FollowDialog {
	pub fn open(&mut self, state: &State, source: Id) {
		*self = Self {
			source: Some((state.generation, source)),
			..Self::default()
		};
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &mut State, commands: &mut Vec<Command>) {
		let Some((generation, source)) = self.source else {
			return;
		};
		if state.generation != generation
			|| state.selected != Some(source)
			|| !state.can_view(source)
			|| !state
				.channel(source)
				.is_some_and(|channel| channel.kind == 5)
		{
			*self = Self::default();
			return;
		}
		if let Some(request) = self.submitted
			&& let Some(success) = state.channel_action_completed(source, request)
		{
			self.submitted = None;
			if success {
				*self = Self::default();
				return;
			}
		}
		let mut close = false;
		let busy = state.channel_action_pending() || self.submitted.is_some();
		let connected = state.demo
			|| (state.gateway_connected
				&& state.auth == client_core::auth::AuthState::Authenticated);
		let response = crate::dialog::Dialog::new(
			"follow-announcement",
			crate::i18n::translate("announcement-follow-title"),
		)
		.subtitle(crate::i18n::translate("announcement-follow-detail"))
		.width(460.0)
		.show(ctx, |dialog| {
			dialog.scroll(190.0, |ui| {
				ui.add(
					egui::TextEdit::singleline(&mut self.query)
						.char_limit(64)
						.hint_text(crate::i18n::translate("announcement-follow-search")),
				);
				let query = self.query.trim().to_lowercase();
				let mut found = false;
				egui::ScrollArea::vertical()
					.max_height(240.0)
					.show(ui, |ui| {
						for channel in state
							.channels
							.iter()
							.filter(|channel| state.can_follow_announcement(source, channel.id))
							.filter(|channel| {
								query.is_empty()
									|| channel.name.to_lowercase().contains(&query)
									|| channel.guild.and_then(|id| state.guild(id)).is_some_and(
										|guild| guild.name.to_lowercase().contains(&query),
									)
							})
							.take(100)
						{
							found = true;
							let guild = channel
								.guild
								.and_then(|id| state.guild(id))
								.map_or("", |guild| guild.name.as_str());
							ui.add_enabled_ui(!busy, |ui| {
								ui.radio_value(
									&mut self.destination,
									Some(channel.id),
									format!("{guild} · #{}", channel.name),
								);
							});
						}
					});
				if !found {
					ui.weak(crate::i18n::translate("announcement-follow-no-targets"));
				}
				if let Some(status) = state.channel_action_status(source) {
					ui.weak(crate::i18n::translate_if_key(status));
				}
			});
			dialog.footer(|ui| {
				if ui
					.add_enabled(
						!busy
							&& connected && self
							.destination
							.is_some_and(|target| state.can_follow_announcement(source, target)),
						egui::Button::new(crate::i18n::translate("announcement-follow")),
					)
					.clicked() && let Some(target) = self.destination
					&& let Some(command) =
						state.request_channel_action(source, Action::FollowAnnouncement(target))
				{
					if let Command::ChannelAction { request, .. } = &command {
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
