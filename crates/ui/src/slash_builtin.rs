//! Local slash commands. Server application commands are resolved separately.
use client_core::{Command, MAX_DRAFT_BYTES, State};
use model::Id;

/// Localize only known application-owned outcomes; preserve unknown status text.
pub(super) fn status_text(status: &str) -> String {
	let key = match status {
		"You don't have permission to use this command in this channel."
		| "You don't have permission to use this command here" => "action-command-permission",
		"The slash command result exceeds the message length limit." => "action-command-too-long",
		"Use up to 64 characters in a GIF or sticker search." => "action-command-search-limit",
		"Choose a recipient: /msg @user [message]" => "action-msg-recipient",
		"Add some text after the slash command." => "action-command-add-text",
		"Several users match. Use a user mention or ID with /msg." => "action-msg-ambiguous",
		"User not found. Use a known user's mention, ID, or exact username with /msg." => {
			"action-msg-not-found"
		}
		"Finish editing or uploading before using a slash command." => "action-command-finish-edit",
		"Slash command exceeds the session input budget. Your draft was kept."
		| "Clear some draft space before using /msg. Your draft was kept." => "composer-send-budget",
		"Send or remove the selected attachments before using /msg." => "action-msg-attachments",
		"Wait for the previous direct message to open." => "action-msg-wait",
		"Open a DM with this user first, or choose a friend with /msg." => "action-msg-open-first",
		"This DM already has a draft. Your /msg text was kept in the original conversation." => {
			"action-msg-existing-draft"
		}
		"Draft budget full. Clear an existing draft before using /msg." => "action-msg-draft-limit",
		"Direct message opened. Review the message, then press Enter to send." => {
			"action-msg-staged"
		}
		"Wait for the current command or file selection to finish" => "action-command-busy",
		"Finish the current application interaction first" => "action-app-busy",
		"Choose a command from the list before running it." => "action-command-choose",
		"Wait for the current direct message action to finish" => "action-direct-busy",
		"Calls are unavailable in this session" => "action-direct-call-unavailable",
		"This direct message action is unavailable; your message was kept" => {
			"action-direct-unavailable"
		}
		"Calls are unavailable in the offline preview. No microphone is accessed." => {
			"action-call-preview"
		}
		"Voice is unavailable in this session." => "action-call-unavailable",
		"Reconnect to Discord before calling." => "action-call-reconnect",
		"Waiting for the previous call to disconnect." => "action-call-wait",
		"You are already in this call." => "action-call-already",
		"Joining this channel is unavailable with current permission information." => {
			"action-call-permission"
		}
		"Application commands are unavailable in this conversation" => "action-app-unavailable",
		"Refresh the application command list first" => "action-app-refresh",
		"This command is no longer available" => {
			"slash-commands-composer-command-unavailable-your-arguments-are-kept"
		}
		"Choose an accessible channel of the requested type" => "action-app-channel",
		"Enter a channel identifier" => "action-app-channel-id",
		"Choose a conversation first" => "action-app-conversation",
		"Application interaction could not be started" => "action-app-start",
		"Command value exceeds its byte limit" => "action-app-value-length",
		"Command value does not match its choices or limits" => "action-app-value-limits",
		"Choose a subcommand" => "action-app-subcommand",
		"Complete every required command option" => "action-app-required",
		"Command submission exceeds its limits" => "action-app-size",
		"This command option is unsupported"
		| "Invalid subcommand path"
		| "Invalid command schema or too many values"
		| "Unknown or repeated command option" => "action-app-invalid",
		"Free some draft space before choosing a command." => "action-app-draft-space",
		"Enter a whole number" => "action-app-whole-number",
		"Choose true or false" => "action-app-boolean",
		"Enter a number" => "action-app-number",
		"This option is unsupported" => "action-app-unsupported",
		"Choose from the list or enter an ID" => "action-app-choose-id",
		"Value does not match this option's limits" => "action-app-option-limits",
		"Choose one of the listed options" => "action-app-choose-option",
		_ => return crate::i18n::translate_if_key(status),
	};
	crate::i18n::translate(key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Builtin {
	Gif,
	Me,
	Msg,
	Shrug,
	Spoiler,
	Sticker,
	Tableflip,
	Unflip,
}

impl Builtin {
	pub fn description(self) -> String {
		crate::i18n::translate(match self {
			Self::Gif => "slash-builtin-gif",
			Self::Me => "slash-builtin-me",
			Self::Msg => "slash-builtin-msg",
			Self::Shrug => "slash-builtin-shrug",
			Self::Spoiler => "slash-builtin-spoiler",
			Self::Sticker => "slash-builtin-sticker",
			Self::Tableflip => "slash-builtin-tableflip",
			Self::Unflip => "slash-builtin-unflip",
		})
	}
	pub fn available(self, state: &State, channel: Id) -> bool {
		state.can_compose(channel)
	}
}

pub(super) struct Descriptor {
	pub command: Builtin,
	pub name: &'static str,
	pub usage: &'static str,
}

pub(super) const ALL: &[Descriptor] = &[
	Descriptor {
		command: Builtin::Gif,
		name: "gif",
		usage: "/gif [search]",
	},
	Descriptor {
		command: Builtin::Me,
		name: "me",
		usage: "/me text",
	},
	Descriptor {
		command: Builtin::Msg,
		name: "msg",
		usage: "/msg @user [message]",
	},
	Descriptor {
		command: Builtin::Shrug,
		name: "shrug",
		usage: "/shrug [text]",
	},
	Descriptor {
		command: Builtin::Spoiler,
		name: "spoiler",
		usage: "/spoiler text",
	},
	Descriptor {
		command: Builtin::Sticker,
		name: "sticker",
		usage: "/sticker [search]",
	},
	Descriptor {
		command: Builtin::Tableflip,
		name: "tableflip",
		usage: "/tableflip [text]",
	},
	Descriptor {
		command: Builtin::Unflip,
		name: "unflip",
		usage: "/unflip [text]",
	},
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Invocation<'a> {
	pub command: Builtin,
	pub arguments: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action<'a> {
	Text(String),
	Gifs(&'a str),
	Stickers(&'a str),
	Message { recipient: &'a str, text: &'a str },
}

/// Completion is only active for the leading command token, never inline text.
pub(super) fn query(draft: &str) -> Option<&str> {
	let query = draft.strip_prefix('/')?;
	(!query.chars().any(char::is_whitespace)).then_some(query)
}

/// Unknown commands remain available for server application-command lookup.
pub(super) fn parse(draft: &str) -> Option<Invocation<'_>> {
	let (name, arguments) = split_word(draft.strip_prefix('/')?);
	let command = ALL.iter().find(|entry| entry.name == name)?.command;
	Some(Invocation { command, arguments })
}

fn split_word(text: &str) -> (&str, &str) {
	let end = text.find(char::is_whitespace).unwrap_or(text.len());
	(&text[..end], text[end..].trim_start())
}

impl<'a> Invocation<'a> {
	pub fn action(self) -> Result<Action<'a>, &'static str> {
		const TOO_LONG: &str = "The slash command result exceeds the message length limit.";
		if self.arguments.len() > client_core::MAX_CONTENT * 4
			|| self.arguments.chars().count() > client_core::MAX_CONTENT
		{
			return Err(TOO_LONG);
		}
		let text = self.arguments;
		let content = match self.command {
			Builtin::Gif | Builtin::Sticker if text.chars().count() > 64 => {
				return Err("Use up to 64 characters in a GIF or sticker search.");
			}
			Builtin::Gif => return Ok(Action::Gifs(text)),
			Builtin::Sticker => return Ok(Action::Stickers(text)),
			Builtin::Msg => {
				let (recipient, text) = split_word(text);
				if recipient.is_empty() {
					return Err("Choose a recipient: /msg @user [message]");
				}
				return Ok(Action::Message { recipient, text });
			}
			Builtin::Me | Builtin::Spoiler if text.trim().is_empty() => {
				return Err("Add some text after the slash command.");
			}
			Builtin::Me => format!("*{text}*"),
			Builtin::Spoiler => format!("||{text}||"),
			Builtin::Shrug | Builtin::Tableflip | Builtin::Unflip => {
				// Escape Markdown punctuation so the emoticon survives message rendering.
				let emoticon = match self.command {
					Builtin::Shrug => r"¯\\\_(ツ)\_/¯",
					Builtin::Tableflip => "(╯°□°)╯︵ ┻━┻",
					_ => "┬─┬ ノ( ゜-゜ノ)",
				};
				if text.is_empty() {
					emoticon.to_owned()
				} else {
					format!("{text} {emoticon}")
				}
			}
		};
		if content.chars().count() > client_core::MAX_CONTENT {
			return Err(TOO_LONG);
		}
		Ok(Action::Text(content))
	}
}

/// One pending DM handoff, bounded by the composer's 2,000-character input limit.
pub(super) struct PendingMessage {
	generation: u64,
	user: Id,
	origin: Id,
	source: String,
	text: String,
}

fn direct_channel(state: &State, user: Id) -> Option<Id> {
	state.channels.iter().find_map(|channel| {
		(channel.guild.is_none()
			&& channel.kind == 1
			&& channel.recipients.len() == 1
			&& channel.recipients[0].id == user)
			.then_some(channel.id)
	})
}

fn recipient(state: &State, channel: Id, query: &str) -> Result<Id, &'static str> {
	let token = query
		.strip_prefix("<@")
		.and_then(|text| text.strip_suffix('>'))
		.map(|text| text.strip_prefix('!').unwrap_or(text))
		.unwrap_or(query);
	let id = token.parse::<u64>().ok().filter(|id| *id != 0).map(Id);
	let name = query.strip_prefix('@').unwrap_or(query);
	let known = crate::mentions::known_users(state, channel);
	let mut found = None;
	for user in known.iter().chain(state.friends()).chain(
		state
			.channels
			.iter()
			.flat_map(|channel| &channel.recipients),
	) {
		if user.webhook
			|| state.user.as_ref().is_some_and(|owner| owner.id == user.id)
			|| state.user_blocked(user.id) == Some(true)
		{
			continue;
		}
		let matches = id.map_or_else(
			|| {
				user.name.eq_ignore_ascii_case(name)
					|| state
						.friend_username(user.id)
						.is_some_and(|username| username.eq_ignore_ascii_case(name))
			},
			|id| id == user.id,
		);
		if matches {
			if found.is_some_and(|id| id != user.id) {
				return Err("Several users match. Use a user mention or ID with /msg.");
			}
			found = Some(user.id);
		}
	}
	found.ok_or("User not found. Use a known user's mention, ID, or exact username with /msg.")
}

impl crate::MessagingUi {
	pub(super) fn action_notice(
		&mut self,
		state: &mut State,
		status: &'static str,
		level: crate::design::Level,
	) {
		state.status = status;
		self.toasts.push(level, status_text(status));
	}

	pub(super) fn handle_builtin_slash(
		&mut self,
		state: &mut State,
		channel: Id,
		ctx: &egui::Context,
		commands: &mut Vec<Command>,
	) -> bool {
		let Some(source) = state
			.drafts
			.get(&channel)
			.filter(|text| parse(text).is_some())
		else {
			return false;
		};
		let source = source.clone();
		let invocation = parse(&source).expect("recognized builtin");
		if !invocation.command.available(state, channel) {
			self.action_notice(
				state,
				"You don't have permission to use this command in this channel.",
				crate::design::Level::Warning,
			);
			return true;
		}
		let action = match invocation.action() {
			Ok(action) => action,
			Err(error) => {
				self.action_notice(state, error, crate::design::Level::Warning);
				return true;
			}
		};
		if state.selected != Some(channel) || self.editing.is_some() || self.upload_busy {
			self.action_notice(
				state,
				"Finish editing or uploading before using a slash command.",
				crate::design::Level::Warning,
			);
			return true;
		}
		match action {
			Action::Text(content) => {
				let original_bytes = state.drafts.get(&channel).map_or(0, String::capacity);
				if state.draft_bytes().saturating_sub(original_bytes) + content.capacity()
					> MAX_DRAFT_BYTES
				{
					self.action_notice(
						state,
						"Slash command exceeds the session input budget. Your draft was kept.",
						crate::design::Level::Warning,
					);
					return true;
				}
				let original = state.drafts.insert(channel, content).expect("source draft");
				let files = self.selected_files();
				let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
				if let Some(command) = state.prepare_send_with_attachments(&names) {
					self.stage_pending_upload(ctx, &command);
					self.clear_draft(state, channel);
					self.timeline.follow_latest(state);
					commands.push(command);
				} else {
					state.drafts.insert(channel, original);
					self.composer_send_failure(state);
				}
			}
			Action::Gifs(query) => {
				self.emoji_picker.open_gifs(query);
				self.clear_draft(state, channel);
			}
			Action::Stickers(query) => {
				self.emoji_picker.search_stickers(query);
				self.clear_draft(state, channel);
			}
			Action::Message {
				recipient: query,
				text,
			} => {
				if self.attachment.is_some() || !self.attachment_files.is_empty() {
					self.action_notice(
						state,
						"Send or remove the selected attachments before using /msg.",
						crate::design::Level::Warning,
					);
					return true;
				}
				if self.slash_direct.is_some() {
					self.action_notice(
						state,
						"Wait for the previous direct message to open.",
						crate::design::Level::Warning,
					);
					return true;
				}
				let user = match recipient(state, channel, query) {
					Ok(user) => user,
					Err(error) => {
						self.action_notice(state, error, crate::design::Level::Warning);
						return true;
					}
				};
				let text = text.to_owned();
				if state.draft_bytes() + source.capacity() + text.capacity() > MAX_DRAFT_BYTES {
					self.action_notice(
						state,
						"Clear some draft space before using /msg. Your draft was kept.",
						crate::design::Level::Warning,
					);
					return true;
				}
				let pending = PendingMessage {
					generation: state.generation,
					user,
					origin: channel,
					source,
					text,
				};
				if let Some(target) = direct_channel(state, user) {
					if let Err(error) = self.check_slash_direct(state, &pending, target) {
						self.action_notice(state, error, crate::design::Level::Warning);
						return true;
					}
					if let Some(command) = state.select(target) {
						commands.push(command);
					}
					if state.selected == Some(target) {
						self.stage_slash_direct(state, pending, target);
					}
				} else if state.friend(user).is_none() {
					self.action_notice(
						state,
						"Open a DM with this user first, or choose a friend with /msg.",
						crate::design::Level::Warning,
					);
				} else if let Some(command) = state.open_friend_dm(user) {
					self.slash_direct = Some(pending);
					commands.push(command);
				} else {
					self.toasts.push(
						crate::design::Level::Warning,
						crate::i18n::translate("action-direct-unavailable"),
					);
				}
			}
		}
		true
	}

	pub(super) fn finish_slash_direct(&mut self, state: &mut State) {
		let Some(pending) = self.slash_direct.take() else {
			return;
		};
		if pending.generation != state.generation
			|| state.drafts.get(&pending.origin) != Some(&pending.source)
		{
			return;
		}
		if let Some(target) =
			direct_channel(state, pending.user).filter(|id| state.selected == Some(*id))
		{
			match self.check_slash_direct(state, &pending, target) {
				Ok(()) => self.stage_slash_direct(state, pending, target),
				Err(error) => self.action_notice(state, error, crate::design::Level::Warning),
			}
		} else if state.selected == Some(pending.origin) && state.user_action_pending() {
			self.slash_direct = Some(pending);
		}
	}

	fn check_slash_direct(
		&self,
		state: &State,
		pending: &PendingMessage,
		target: Id,
	) -> Result<(), &'static str> {
		if !pending.text.is_empty()
			&& target != pending.origin
			&& state
				.drafts
				.get(&target)
				.is_some_and(|text| !text.is_empty())
		{
			return Err(
				"This DM already has a draft. Your /msg text was kept in the original conversation.",
			);
		}
		if !pending.text.is_empty()
			&& !state.drafts.contains_key(&target)
			&& state.drafts.len() >= 64
			&& self.draft_restore_pending
		{
			return Err("Draft budget full. Clear an existing draft before using /msg.");
		}
		let source_bytes = state
			.drafts
			.get(&pending.origin)
			.map_or(0, String::capacity);
		if state.draft_bytes().saturating_sub(source_bytes) + pending.text.capacity()
			> MAX_DRAFT_BYTES
		{
			return Err("Clear some draft space before using /msg. Your draft was kept.");
		}
		Ok(())
	}

	fn stage_slash_direct(&mut self, state: &mut State, pending: PendingMessage, target: Id) {
		self.clear_draft(state, pending.origin);
		if !pending.text.is_empty() {
			state.drafts.insert(target, pending.text);
			self.draft_changes.push(target);
		}
		self.focus_switched_composer = true;
		self.action_notice(
			state,
			"Direct message opened. Review the message, then press Enter to send.",
			crate::design::Level::Info,
		);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn missing_recipient_notifies_once_and_preserves_the_original_draft() {
		struct ResetLanguage;
		impl Drop for ResetLanguage {
			fn drop(&mut self) {
				crate::i18n::set_current(crate::i18n::Language::English);
			}
		}
		let _reset = ResetLanguage;
		crate::i18n::set_current(crate::i18n::Language::Spanish);
		let mut state = test_support::demo_state();
		let channel = state.selected.unwrap();
		state.drafts.insert(channel, "/msg".into());
		let mut view = crate::MessagingUi::default();
		let ctx = egui::Context::default();
		let mut commands = Vec::new();
		for _ in 0..2 {
			assert!(view.handle_builtin_slash(&mut state, channel, &ctx, &mut commands));
			crate::composer::tests::frame(&ctx, |ui| view.toasts.show(ui.ctx(), 0.0));
		}
		let labels = crate::composer::tests::frame(&ctx, |ui| view.toasts.show(ui.ctx(), 0.0));
		let expected = crate::i18n::translate("action-msg-recipient");
		assert_ne!(expected, "Choose a recipient: /msg @user [message]");
		assert!(!expected.starts_with("Unknown localization key"));
		assert_eq!(labels.iter().filter(|label| *label == &expected).count(), 1);
		assert_eq!(state.drafts[&channel], "/msg");
		assert_eq!(state.status, "Choose a recipient: /msg @user [message]");
		assert!(commands.is_empty());
	}

	#[test]
	fn builtins_preserve_text_resolve_actions_and_reject_invalid_input() {
		assert_eq!(query("/"), Some(""));
		assert_eq!(query("/shr"), Some("shr"));
		for literal in ["look /shrug", " /me hi", "/meow", "/unknown", "//me"] {
			assert_eq!(parse(literal), None);
		}
		assert_eq!(query("/me text"), None);
		for (input, expected) in [
			("/me  ahoj 世界\n🙂", "*ahoj 世界\n🙂*"),
			("/spoiler secret", "||secret||"),
			("/shrug", r"¯\\\_(ツ)\_/¯"),
			("/shrug hi", r"hi ¯\\\_(ツ)\_/¯"),
			("/tableflip", "(╯°□°)╯︵ ┻━┻"),
			("/unflip", "┬─┬ ノ( ゜-゜ノ)"),
		] {
			assert_eq!(
				parse(input).unwrap().action(),
				Ok(Action::Text(expected.into()))
			);
		}
		assert_eq!(
			parse("/gif cats").unwrap().action(),
			Ok(Action::Gifs("cats"))
		);
		assert_eq!(
			parse("/sticker").unwrap().action(),
			Ok(Action::Stickers(""))
		);
		assert_eq!(
			parse("/msg <@42> hello 世界").unwrap().action(),
			Ok(Action::Message {
				recipient: "<@42>",
				text: "hello 世界"
			})
		);
		for input in ["/me", "/spoiler\n\t", "/msg"] {
			assert!(parse(input).unwrap().action().is_err());
		}
		let fits = format!("/me {}", "🙂".repeat(client_core::MAX_CONTENT - 2));
		assert!(parse(&fits).unwrap().action().is_ok());
		let exceeds = format!("/shrug {}", "x".repeat(client_core::MAX_CONTENT - 7));
		assert!(parse(&exceeds).unwrap().action().is_err());
	}

	#[test]
	fn execution_sends_text_preserves_rejected_drafts_and_stages_direct_messages() {
		let mut state = test_support::demo_state();
		let mut ui = crate::MessagingUi::default();
		let ctx = egui::Context::default();
		let mut commands = Vec::new();
		let origin = state.selected.unwrap();
		state.drafts.insert(origin, "/me hello 世界".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert!(
			matches!(&commands[0], Command::Send { content, channel, .. }
			if content == "*hello 世界*" && *channel == origin)
		);
		assert!(!state.drafts.contains_key(&origin));
		commands.clear();

		state.demo = false;
		state.gateway_connected = false;
		state
			.drafts
			.insert(origin, "/spoiler send during recovery".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert!(
			matches!(&commands[0], Command::Send { content, channel, .. }
			if content == "||send during recovery||" && *channel == origin)
		);
		assert!(!state.drafts.contains_key(&origin));
		commands.clear();
		state.auth = client_core::auth::AuthState::Expired;
		state.drafts.insert(origin, "/spoiler keep me".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert_eq!(state.drafts[&origin], "/spoiler keep me");
		assert!(commands.is_empty());
		state.auth = client_core::auth::AuthState::Authenticated;
		state.demo = true;

		ui.preview_attachment("keep.txt", 1, None);
		state.drafts.insert(origin, "/gif cats".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert!(!state.drafts.contains_key(&origin));
		assert_eq!(ui.selected_files(), vec![("keep.txt".into(), 1)]);
		assert!(commands.is_empty());
		ui.attachment = None;
		ui.attachment_files.clear();

		let dm = state
			.channels
			.iter()
			.find(|channel| channel.kind == 1)
			.unwrap()
			.clone();
		let source = format!("/msg <@{}> hello", dm.recipients[0].id);
		state.drafts.insert(origin, source.clone());
		state.drafts.insert(dm.id, "existing draft".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert_eq!(state.drafts[&origin], source);
		assert_eq!(state.drafts[&dm.id], "existing draft");
		assert_eq!(state.selected, Some(origin));
		state.drafts.remove(&dm.id);
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		assert_eq!(state.selected, Some(dm.id));
		assert_eq!(state.drafts[&dm.id], "hello");
		assert!(!state.drafts.contains_key(&origin));
		assert!(
			!commands
				.iter()
				.any(|command| matches!(command, Command::Send { .. }))
		);

		state.select(origin);
		commands.clear();
		state
			.drafts
			.insert(origin, "/msg robin.synthetic new conversation".into());
		assert!(ui.handle_builtin_slash(&mut state, origin, &ctx, &mut commands));
		let Command::UserAction {
			action: client_core::user_actions::Action::OpenDm(user),
			request,
			..
		} = commands.remove(0)
		else {
			panic!("expected an explicit open-DM action");
		};
		assert!(ui.slash_direct.is_some());
		let mut opened = dm;
		opened.id = Id(991);
		opened.recipients = vec![state.friend(user).unwrap().clone()];
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::UserAction(client_core::user_actions::Event::DmOpened {
				user,
				request,
				result: Ok(Box::new(opened)),
			}),
		});
		state.select_opened_dm();
		ui.finish_slash_direct(&mut state);
		assert_eq!(state.selected, Some(Id(991)));
		assert_eq!(state.drafts[&Id(991)], "new conversation");
		assert!(!state.drafts.contains_key(&origin));
		assert!(ui.slash_direct.is_none());
	}
}
