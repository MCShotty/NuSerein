//! Member review is scoped separately from the visible conversation and read acknowledgements.
use client_core::{
	Command, State,
	server_admin::{REVIEW_AUDIT, REVIEW_MESSAGES, REVIEW_ROLES},
};
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
	notice: Option<&'static str>,
}
impl ModViewUi {
	pub fn close(&mut self, state: &mut State) {
		*self = Self::default();
		state.close_member_review();
	}
	pub fn open(&mut self, state: &mut State, guild: Id, user: Id) -> Option<Command> {
		self.notice = None;
		if !state.can_open_mod_view(guild) {
			return None;
		}
		if state.server_admin.pending {
			state.status = "Wait for the current administrative action to finish.";
			self.notice = Some("mod-view-pending");
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
	pub fn take_notice(&mut self) -> Option<&'static str> {
		self.notice.take()
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
			let loading=state.server_admin.member_review_loading;
			let member=&review.member;
			ui.heading(member.nick.as_deref().unwrap_or(&member.user.name));
			if let Some(username)=member.user.username.as_deref(){ui.weak(format!("@{username} · {user}"));}
			else{ui.weak(format!("Account ID: {user}"));}
			if let Some(joined)=member.joined_at.and_then(|at|crate::local_time::discord_timestamp((at/1000)as i64,b'f')) {ui.label(format!("Joined server: {joined}"));}
			if let Some(created)=crate::local_time::discord_timestamp((((user.0>>22)+1_420_070_400_000)/1000)as i64,b'f') {ui.label(format!("Account created: {created}"));}
			ui.label(format!("Join method: {}",member.join_source.map_or("Unknown",|source|match source {1=>"Bot",2=>"Integration",3=>"Discovery",4=>"Hub",5=>"Invite",6=>"Custom invite link",7=>"Application",8=>"Linked channel",_=>"Unknown"})));
			ui.horizontal_wrapped(|ui| {
				ui.label("Roles:");
				if loading & REVIEW_ROLES != 0 {ui.spinner();}

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
				if loading & REVIEW_MESSAGES != 0 {ui.horizontal(|ui|{ui.spinner();ui.weak("Loading recent messages…");});}
				if let Some(error)=review.messages_error {ui.weak(error);}
				if review.messages.is_empty()&&review.messages_error.is_none()&&loading & REVIEW_MESSAGES == 0{ui.weak("No recent messages returned.");}
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
					if loading & REVIEW_AUDIT != 0 {ui.horizontal(|ui|{ui.spinner();ui.weak("Loading audit activity…");});}
					if let Some(error)=review.audit_error{ui.weak(error);}
					if review.audit.is_empty()&&review.audit_error.is_none()&&loading & REVIEW_AUDIT == 0{ui.weak("No matching entries in this window.");}
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
	fn rejected_replacement_keeps_existing_review() {
		for missing_channel in [false, true] {
			let mut state = test_support::demo_state();
			let guild = Id(10);
			let channel = state.selected.unwrap();
			let mut permissions = test_support::permission_snapshot(&state);
			for row in &mut permissions.guilds {
				row.owner = state.user.as_ref().map(|user| user.id);
			}
			state.permissions.replace(permissions).unwrap();
			assert!(state.can_open_mod_view(guild));
			let mut user = state.user.as_ref().unwrap().clone();
			user.id = Id(776);
			state.server_admin.guild = Some(guild);
			state.server_admin.member_review = Some(model::moderation::Review {
				guild,
				channel,
				member: model::server_admin::Member {
					user,
					nick: None,
					roles: vec![],
					joined_at: None,
					join_source: None,
					invite_code: None,
					flags: None,
					unusual_dm_until: None,
					timeout_until: None,
				},
				roles: vec![],
				messages: vec![],
				audit: vec![],
				audit_window: None,
				messages_error: None,
				audit_error: None,
			});
			let mut view = ModViewUi::default();
			let old_scope = (state.generation, guild, Id(776), state.selected);
			view.scope = Some(old_scope);
			state.server_admin.member_review_loading = REVIEW_ROLES;
			if missing_channel {
				state
					.channels
					.retain(|channel| channel.guild != Some(guild));
			} else {
				state.server_settings.saving = true;
			}
			assert!(view.open(&mut state, guild, Id(777)).is_none());
			assert_eq!(view.scope, Some(old_scope));
			assert_eq!(
				state
					.server_admin
					.member_review
					.as_ref()
					.unwrap()
					.member
					.user
					.id,
				Id(776)
			);
			assert_eq!(state.server_admin.member_review_loading, REVIEW_ROLES);
		}
	}

	#[test]
	fn rejected_mod_view_action_keeps_the_open_profile() {
		let mut state = test_support::demo_state();
		let guild = Id(10);
		let mut permissions = test_support::permission_snapshot(&state);
		for row in &mut permissions.guilds {
			row.owner = state.user.as_ref().map(|user| user.id);
		}
		state.permissions.replace(permissions).unwrap();
		let mut user = state.user.as_ref().unwrap().clone();
		user.id = Id(777);
		let mut view = crate::MessagingUi::default();
		view.profile.command_open(user);
		view.user_action = Some(crate::user_menu::Action::ModView {
			guild,
			user: Id(777),
		});
		state.server_admin.pending = true;
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		let mut commands = Vec::new();
		let output = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1200.0, 900.0),
				)),
				..Default::default()
			},
			|ui| commands.extend(view.show(ui, &mut state)),
		);
		assert_eq!(view.profile.open_user().map(|user| user.id), Some(Id(777)));
		assert!(commands.iter().all(|command| !matches!(command, Command::ServerAdmin { action, .. } if matches!(action.as_ref(), server_admin::Action::Moderation(Action::Review { .. })))));
		output.drop_without_applying_deltas();
	}

	#[test]
	fn busy_mod_view_open_preserves_the_current_view_and_displays_one_notice() {
		let mut state = test_support::demo_state();
		let guild = Id(10);
		let mut permissions = test_support::permission_snapshot(&state);
		for row in &mut permissions.guilds {
			row.owner = state.user.as_ref().map(|user| user.id);
		}
		state.permissions.replace(permissions).unwrap();
		assert!(state.can_open_mod_view(guild));
		state.server_admin.guild = Some(guild);
		state.server_admin.pending = true;
		let mut view = crate::MessagingUi {
			language: crate::i18n::Language::Czech,
			..Default::default()
		};
		let old_scope = (state.generation, guild, Id(776), state.selected);
		view.mod_view.scope = Some(old_scope);
		for _ in 0..2 {
			assert!(view.preview_mod_view(&mut state, guild, Id(777)).is_none());
			assert_eq!(view.mod_view.scope, Some(old_scope));
		}
		assert!(state.server_admin.pending);
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		for _ in 0..3 {
			let mut commands = Vec::new();
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(1200.0, 900.0),
					)),
					..Default::default()
				},
				|ui| commands.extend(view.show(ui, &mut state)),
			);
			fn count(shape: &egui::Shape, text: &str) -> usize {
				match shape {
					egui::Shape::Text(value) => usize::from(value.galley.job.text == text),
					egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, text)).sum(),
					_ => 0,
				}
			}
			let expected = crate::i18n::translate("mod-view-pending");
			assert_eq!(
				output
					.shapes
					.iter()
					.map(|shape| count(&shape.shape, &expected))
					.sum::<usize>(),
				1
			);
			assert!(commands.iter().all(|command| !matches!(command, Command::ServerAdmin { action, .. } if matches!(action.as_ref(), server_admin::Action::Moderation(Action::Review { .. })))));
			output.drop_without_applying_deltas();
		}
		assert_eq!(view.mod_view.scope, Some(old_scope));
		assert!(state.server_admin.pending);
		crate::i18n::set_current(crate::i18n::Language::English);
	}

	#[test]
	fn members_context_menu_opens_an_actionable_drawer_above_the_conversation() {
		fn render(
			ctx: &egui::Context,
			view: &mut crate::MessagingUi,
			state: &mut State,
			events: Vec<egui::Event>,
		) -> (egui::FullOutput, Vec<Command>) {
			let mut commands = vec![];
			let output = ctx.run_ui(
				egui::RawInput {
					screen_rect: Some(egui::Rect::from_min_size(
						egui::Pos2::ZERO,
						egui::vec2(1440.0, 1100.0),
					)),
					events,
					..Default::default()
				},
				|ui| commands.extend(view.show(ui, state)),
			);
			(output, commands)
		}
		fn position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
			output
				.shapes
				.iter()
				.find_map(|shape| match &shape.shape {
					egui::Shape::Text(text) if text.galley.job.text == label => {
						Some(text.pos + text.galley.size() / 2.0)
					}
					_ => None,
				})
				.unwrap_or_else(|| panic!("rendered production control {label}"))
		}
		fn click(pos: egui::Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
			vec![
				egui::Event::PointerMoved(pos),
				egui::Event::PointerButton {
					pos,
					button,
					pressed: true,
					modifiers: egui::Modifiers::NONE,
				},
				egui::Event::PointerButton {
					pos,
					button,
					pressed: false,
					modifiers: egui::Modifiers::NONE,
				},
			]
		}
		crate::i18n::set_current(crate::i18n::Language::English);
		let mut state = test_support::demo_state();
		let channel = state.selected.unwrap();
		let guild = state.channel(channel).unwrap().guild.unwrap();
		let mut permissions = test_support::permission_snapshot(&state);
		permissions
			.guilds
			.iter_mut()
			.find(|entry| entry.id == guild)
			.unwrap()
			.owner = state.user.as_ref().map(|user| user.id);
		state.permissions.replace(permissions).unwrap();
		let mut user = state.user.as_ref().unwrap().clone();
		user.id = Id(777);
		user.name = "Route account".into();
		user.username = Some("route.account".into());
		user.avatar = None;
		user.cosmetics = None;
		let member = model::server_admin::Member {
			user,
			nick: Some("Route target".into()),
			roles: vec![],
			joined_at: Some(1_750_000_000_000),
			join_source: None,
			invite_code: None,
			flags: None,
			unusual_dm_until: None,
			timeout_until: None,
		};
		let roles = state
			.permissions
			.guilds
			.get(&guild)
			.unwrap()
			.roles
			.as_ref()
			.unwrap()
			.iter()
			.map(|role| model::server_admin::Role {
				role: role.clone(),
				managed: false,
			})
			.collect::<Vec<_>>();
		let mut view = crate::MessagingUi::default();
		let Command::ServerAdmin {
			guild: target,
			request,
			..
		} = view
			.preview_server_admin(&mut state, guild, "members")
			.unwrap()
		else {
			panic!("production Members read");
		};
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::ServerAdmin(client_core::server_admin::Event {
				guild: target,
				request,
				result: Ok(server_admin::Result::Members(
					model::server_admin::Members {
						items: vec![member.clone()],
						roles: roles.clone(),
						total: 1,
						..Default::default()
					},
				)),
			}),
		});
		let ctx = egui::Context::default();
		crate::design::apply(&ctx);
		for _ in 0..3 {
			render(&ctx, &mut view, &mut state, vec![])
				.0
				.drop_without_applying_deltas();
		}
		let history = state
			.timeline
			.iter()
			.map(|message| message.id)
			.collect::<Vec<_>>();
		let history_request = state.request;
		assert!(view.server_settings.is_open());
		let (output, _) = render(&ctx, &mut view, &mut state, vec![]);
		let row = position(&output, "Route target");
		output.drop_without_applying_deltas();
		render(
			&ctx,
			&mut view,
			&mut state,
			click(row, egui::PointerButton::Secondary),
		)
		.0
		.drop_without_applying_deltas();
		let (output, _) = render(&ctx, &mut view, &mut state, vec![]);
		let open = position(&output, "Open Mod View");
		output.drop_without_applying_deltas();
		let (output, commands) = render(
			&ctx,
			&mut view,
			&mut state,
			click(open, egui::PointerButton::Primary),
		);
		output.drop_without_applying_deltas();
		let mut reads = commands.into_iter().filter_map(|command| match command {
			Command::ServerAdmin {guild, request, action} if matches!(action.as_ref(), server_admin::Action::Moderation(Action::Review {user, ..}) if *user == member.user.id) => Some((guild, request)),
			_ => None,
		});
		let (target, request) = reads
			.next()
			.expect("clicked member dispatches the exact review");
		assert!(reads.next().is_none(), "one click dispatches one review");
		assert!(
			!view.server_settings.is_open(),
			"the foreground settings shell retires before the drawer opens"
		);
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::ServerAdmin(client_core::server_admin::Event {
				guild: target,
				request,
				result: Ok(server_admin::Result::Moderation(
					model::moderation::Outcome::Review(Box::new(model::moderation::Review {
						guild,
						channel,
						member,
						roles,
						messages: vec![],
						audit: vec![],
						audit_window: None,
						messages_error: None,
						audit_error: None,
					})),
				)),
			}),
		});
		for _ in 0..3 {
			render(&ctx, &mut view, &mut state, vec![])
				.0
				.drop_without_applying_deltas();
		}
		let (output, _) = render(&ctx, &mut view, &mut state, vec![]);
		let refresh = position(&output, "Refresh");
		assert!(ctx.memory(|memory| memory.top_modal_layer().is_none()));
		assert_eq!(
			ctx.layer_id_at(refresh),
			Some(egui::LayerId::new(
				egui::Order::Middle,
				egui::Id::unique("member-mod-view")
			))
		);
		output.drop_without_applying_deltas();
		let (output, commands) = render(
			&ctx,
			&mut view,
			&mut state,
			click(refresh, egui::PointerButton::Primary),
		);
		output.drop_without_applying_deltas();
		assert_eq!(commands.iter().filter(|command|matches!(command,Command::ServerAdmin {action,..} if matches!(action.as_ref(),server_admin::Action::Moderation(Action::Review {..})))).count(),1,
			"the visible drawer action receives pointer input");
		assert_eq!(state.selected, Some(channel));
		assert_eq!(state.request, history_request);
		assert_eq!(
			state
				.timeline
				.iter()
				.map(|message| message.id)
				.collect::<Vec<_>>(),
			history
		);
	}
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
