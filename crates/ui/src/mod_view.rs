//! Member review is scoped separately from the visible conversation and read acknowledgements.
use client_core::{Command, State};
use model::{Id, moderation::Action, server_admin};
pub(super) enum Intent {
	Original(String),
	Messages { guild: Id, user: Id },
	Audit { guild: Id, user: Id },
}
#[derive(Default)]
pub(super) struct ModViewUi {
	scope: Option<(u64, Id, Id, Option<Id>)>,
	actions: crate::server_moderation::ModerationUi,
	nickname: String,
	nickname_initialized: bool,
	role_search: String,
}
impl ModViewUi {
	pub fn close(&mut self, state: &mut State) {
		*self = Self::default();
		state.close_member_review();
	}
	pub fn open(&mut self, state: &mut State, guild: Id, user: Id) -> Option<Command> {
		if !state.can_open_mod_view(guild) {
			return None;
		}
		if self.scope.is_some() {
			state.close_member_review();
		}
		if state.server_admin.pending {
			state.status = "Wait for the current administrative action to finish.";
			return None;
		}

		let channel = state
			.selected
			.filter(|id| {
				state
					.channel(*id)
					.is_some_and(|channel| channel.guild == Some(guild))
					&& state.can_view(*id)
			})
			.or_else(|| {
				state
					.channels
					.iter()
					.find(|channel| channel.guild == Some(guild) && state.can_view(channel.id))
					.map(|channel| channel.id)
			})?;
		let action = Action::Review {
			user,
			channel,
			audit: state.guild_permission(guild, model::permissions::VIEW_AUDIT_LOG),
		};
		let command =
			state.request_server_admin(guild, server_admin::Action::Moderation(action))?;
		self.scope = Some((state.generation, guild, user, state.selected));
		self.actions.select(state.generation, guild, user);
		self.nickname_initialized = false;
		self.role_search.clear();
		Some(command)
	}
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		commands: &mut Vec<Command>,
	) -> Option<Intent> {
		let (generation, guild, user, origin) = self.scope?;
		if generation != state.generation
			|| state.selected != origin
			|| !state.can_open_mod_view(guild)
			|| state.server_admin.guild != Some(guild)
		{
			*self = Self::default();
			state.close_member_review();
			return None;
		}
		let mut open = true;
		let mut jump = None;
		let mut member_action = None;
		egui::Window::new("Mod View").id(egui::Id::unique("member-mod-view")).open(&mut open).collapsible(false).resizable(false).vscroll(true).default_width(420.0).max_height((ctx.content_rect().height()-100.0).max(180.0)).anchor(egui::Align2::RIGHT_TOP,egui::vec2(-16.0,64.0)).show(ctx,|ui| {
			ui.horizontal(|ui| {
				ui.weak("Member review");
				if ui.add_enabled(!state.server_admin.pending,egui::Button::new("Refresh")).clicked()
					&& let Some(command)=self.open(state,guild,user){commands.push(command);}
			});
			if state.server_admin.pending {ui.spinner();}
			if let Some(error)=state.server_admin.error {ui.colored_label(crate::design::palette(ui).danger,error);}
			let Some(review)=state.server_admin.member_review.as_ref().filter(|review|review.guild==guild&&review.member.user.id==user) else {
				ui.weak(if state.server_admin.pending {"Loading member information…"}else{"Member details are unavailable. Refresh to try again."});return;
			};
			let member=&review.member;
			ui.heading(member.nick.as_deref().unwrap_or(&member.user.name));
			if let Some(username)=member.user.username.as_deref(){ui.weak(format!("@{username} · {user}"));}
			else{ui.weak(format!("Account ID: {user}"));}
			if let Some(joined)=member.joined_at.and_then(|at|crate::local_time::discord_timestamp((at/1000)as i64,b'f')) {ui.label(format!("Joined server: {joined}"));}
			if let Some(created)=crate::local_time::discord_timestamp((((user.0>>22)+1_420_070_400_000)/1000)as i64,b'f') {ui.label(format!("Account created: {created}"));}
			ui.label(format!("Join method: {}",member.join_source.map_or("Unknown",|source|match source {1=>"Bot",2=>"Integration",3=>"Discovery",4=>"Hub",5=>"Invite",6=>"Custom invite link",7=>"Application",8=>"Linked channel",_=>"Unknown"})));
			ui.horizontal_wrapped(|ui| {
				ui.label("Roles:");
				for role in state.guild_roles(guild).unwrap_or_default().iter().filter(|role|member.roles.contains(&role.id)) {ui.label(&role.name);}
				if member.roles.is_empty(){ui.weak("No additional roles");}
			});
			let now=time::OffsetDateTime::now_utc().unix_timestamp_nanos()/1_000_000;
			if member.unusual_dm_until.is_some_and(|until|until>now){ui.label("Discord signal: unusual DM activity");}
			if member.flags.is_some_and(|flags|flags&(1<<7)!=0){ui.label("Discord signal: quarantined name");}
			if member.flags.is_none()&&member.unusual_dm_until.is_none(){ui.weak("No safety information supplied.");}
			ui.separator();
			egui::CollapsingHeader::new("Recent server messages").default_open(true).show(ui,|ui| {
				if ui.add_enabled(search_channel(state,guild).is_some(),egui::Button::new("Search messages from member"))
					.on_disabled_hover_text("Open a text channel in this server to search its messages.").clicked(){jump=Some(Intent::Messages {guild,user});}
				if state.user_content_restriction(user).is_some(){ui.weak("Messages are hidden for this blocked or ignored member.");return;}
				if let Some(error)=review.messages_error {ui.weak(error);}
				if review.messages.is_empty()&&review.messages_error.is_none(){ui.weak("No recent messages returned.");}
				for row in &review.messages {
					if !state.can_view(row.channel){continue;}
					ui.separator();
					ui.weak(state.channel(row.channel).map_or("Channel",|channel|channel.name.as_str()));
					ui.label(visible_excerpt(&row.excerpt));
					if ui.small_button("Open original").clicked(){jump=Some(Intent::Original(format!("https://discord.com/channels/{guild}/{}/{}",row.channel,row.id)));}
				}
			});
			if state.guild_permission(guild,model::permissions::VIEW_AUDIT_LOG){
				egui::CollapsingHeader::new("Recent audit activity").show(ui,|ui| {
					if ui.add_enabled(!state.server_admin.pending,egui::Button::new("Open member audit log")).clicked(){jump=Some(Intent::Audit {guild,user});}
					ui.weak("Matching entries from the latest 50 audit events; this is not a complete disciplinary history.");
					if let Some((oldest,newest))=review.audit_window {
						let date=|id:Id|crate::local_time::discord_timestamp((((id.0>>22)+1_420_070_400_000)/1000)as i64,b'f').unwrap_or_else(||id.to_string());
						ui.weak(format!("Loaded window: {} – {}",date(oldest),date(newest)));
					}
					if let Some(error)=review.audit_error{ui.weak(error);}
					if review.audit.is_empty()&&review.audit_error.is_none(){ui.weak("No matching entries in this window.");}
					for entry in &review.audit {
						ui.separator();
						ui.label(crate::i18n::translate_if_key(crate::server_audit_log::action_text(entry.action)));
						if let Some(actor)=entry.actor {ui.weak(format!("Moderator: {actor}"));}
						if let Some(at)=crate::local_time::discord_timestamp((((entry.id.0>>22)+1_420_070_400_000)/1000)as i64,b'f'){ui.weak(at);}
						if let Some(reason)=&entry.reason{ui.label(reason);}
					}
				});
			}
			ui.separator();
			let available=!state.server_admin.pending&&!state.server_admin.needs_refresh;
			if state.can_edit_guild_nickname(guild,user) {
				if !self.nickname_initialized {self.nickname=member.nick.clone().unwrap_or_default();self.nickname_initialized=true;}
				ui.horizontal(|ui| {
					ui.add(egui::TextEdit::singleline(&mut self.nickname).char_limit(32).hint_text("Server nickname"));
					if ui.add_enabled(available,egui::Button::new("Save nickname")).clicked(){member_action=Some(server_admin::Action::SetNickname {user,nick:self.nickname.clone()});}
				});
			}
			if review.roles.iter().any(|role|state.can_edit_member_role(guild,user,role.role.id)) {
				ui.add_enabled_ui(available,|ui|ui.menu_button("Edit roles",|ui| {
					ui.add(egui::TextEdit::singleline(&mut self.role_search).char_limit(100).hint_text("Search roles"));
					let query=self.role_search.to_lowercase();
					egui::ScrollArea::vertical().max_height(180.0).show(ui,|ui|for role in review.roles.iter().filter(|role|state.can_edit_member_role(guild,user,role.role.id)&&role.role.name.to_lowercase().contains(&query)) {
						let mut assigned=member.roles.contains(&role.role.id);
						if ui.checkbox(&mut assigned,&role.role.name).changed(){member_action=Some(server_admin::Action::SetRole {user,role:role.role.id,assigned});ui.close();}
					});
				}));
			}
			self.actions.show_actions(ui,state,guild,user,commands);
		});
		if let Some(action) = member_action
			&& let Some(command) = state.request_server_admin(guild, action)
		{
			commands.push(command);
		}
		if !open {
			*self = Self::default();
			state.close_member_review();
		}
		jump
	}
}
fn search_channel(state: &State, guild: Id) -> Option<Id> {
	state.selected.filter(|channel| {
		state.can_search()
			&& state
				.channel(*channel)
				.is_some_and(|channel| channel.guild == Some(guild))
	})
}
impl crate::MessagingUi {
	pub(super) fn apply_mod_view_intent(
		&mut self,
		intent: Intent,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		match intent {
			Intent::Original(url) => self.timeline.opening = Some(url),
			Intent::Messages { guild, user } => {
				if user.0 != 0
					&& state.can_open_mod_view(guild)
					&& let Some(channel) = search_channel(state, guild)
				{
					let query = format!("from:{user}");
					if let Some(command) = state.request_search(query.clone(), None) {
						self.mod_view.close(state);
						self.search.open_extension(channel, false, Some(query));
						commands.push(command);
					}
				}
			}
			Intent::Audit { guild, user } => {
				if let Some(command) = self.open_member_audit(state, guild, user) {
					commands.push(command);
				}
			}
		}
	}
}
/// Preview text never reveals spoilers; opening the original uses the ordinary renderer.
fn visible_excerpt(text: &str) -> String {
	let mut result = String::new();
	let mut parts = text.split("||");
	result.push_str(parts.next().unwrap_or_default());
	for (index, part) in parts.enumerate() {
		if index % 2 == 0 {
			result.push_str("[Spoiler]");
		} else {
			result.push_str(part);
		}
	}
	result
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn contextual_navigation_uses_member_ids_without_replacing_or_acknowledging_the_conversation() {
		let mut state = test_support::demo_state();
		state
			.permissions
			.replace(test_support::permission_snapshot(&state))
			.unwrap();
		let channel = state.selected.unwrap();
		let guild = state.channel(channel).unwrap().guild.unwrap();
		let actor = state.user.as_ref().unwrap().id;
		state.permissions.guilds.get_mut(&guild).unwrap().owner = Some(actor);
		state.permissions.clear_cache();
		state.auth = client_core::auth::AuthState::Authenticated;
		state.gateway_connected = true;
		state.freshness = model::Freshness::Fresh;
		let timeline: Vec<_> = state.timeline.iter().map(|message| message.id).collect();
		let request = state.request;
		let mut view = crate::MessagingUi::default();
		let mut commands = vec![];
		view.apply_mod_view_intent(
			Intent::Messages { guild, user: Id(7) },
			&mut state,
			&mut commands,
		);
		assert!(
			matches!(commands.as_slice(),[Command::Search {channel:origin,guild:Some(server),query,..}] if *origin==channel && *server==guild && query=="from:7")
		);
		assert!(view.search.open);
		assert_eq!(state.selected, Some(channel));
		assert_eq!(state.request, request);
		assert_eq!(
			state
				.timeline
				.iter()
				.map(|message| message.id)
				.collect::<Vec<_>>(),
			timeline
		);
		commands.clear();
		view.apply_mod_view_intent(
			Intent::Audit { guild, user: Id(7) },
			&mut state,
			&mut commands,
		);
		assert!(
			matches!(commands.as_slice(),[Command::ServerAdmin {guild:server,action,..}] if *server==guild && matches!(action.as_ref(),server_admin::Action::AuditLog(query) if query.user.is_none()))
		);
		assert_eq!(state.selected, Some(channel));
		assert_eq!(state.request, request);
	}
	#[test]
	fn review_excerpts_conceal_spoilers_including_unclosed_segments() {
		assert_eq!(
			super::visible_excerpt("Before ||hidden|| after ||unfinished"),
			"Before [Spoiler] after [Spoiler]"
		);
	}
}
