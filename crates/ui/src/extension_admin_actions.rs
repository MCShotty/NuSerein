//! Approved channel and server proposals reuse the native admission paths.
use crate::MessagingUi;
use client_core::{
	Command, State,
	channel_actions::{Action, CreateKind, Edit, Mute},
	server_actions::InviteOptions,
};
use extensions::{AppAction, ChannelEditInput, PermissionOverwriteInput};
use model::{Id, Patch};

fn id(value: &str) -> Result<Id, String> {
	value
		.parse::<u64>()
		.ok()
		.filter(|value| *value != 0)
		.map(Id)
		.ok_or_else(|| "Invalid identifier".into())
}

fn queue(command: Option<Command>, commands: &mut Vec<Command>) -> Result<(), String> {
	commands.push(command.ok_or(
		"This action is unavailable with the current permissions, data or pending operation",
	)?);
	Ok(())
}

fn overwrite(value: PermissionOverwriteInput) -> Result<model::permissions::Overwrite, String> {
	Ok(model::permissions::Overwrite {
		id: id(&value.id)?,
		kind: value.kind,
		allow: value
			.allow
			.parse()
			.map_err(|_| "Invalid permission allow mask")?,
		deny: value
			.deny
			.parse()
			.map_err(|_| "Invalid permission deny mask")?,
	})
}

fn edit(value: ChannelEditInput) -> Result<Edit, String> {
	Ok(Edit {
		name: value.name,
		topic: value.topic,
		slowmode: value.slowmode,
		nsfw: value.nsfw,
		overwrites: value
			.overwrites
			.into_iter()
			.map(overwrite)
			.collect::<Result<_, _>>()?,
		forum: None,
		voice: None,
	})
}

fn guild_anchor(state: &State, guild: Id, manage: bool) -> Option<Id> {
	state.guild(guild)?;
	state
		.selected
		.filter(|channel| {
			state
				.channel(*channel)
				.is_some_and(|c| c.guild == Some(guild))
				&& if manage {
					state.can_manage_channel(*channel)
				} else {
					state.can_view(*channel)
				}
		})
		.or_else(|| {
			state
				.channels
				.iter()
				.find(|channel| {
					channel.guild == Some(guild)
						&& if manage {
							state.can_manage_channel(channel.id)
						} else {
							state.can_view(channel.id)
						}
				})
				.map(|channel| channel.id)
		})
}

impl MessagingUi {
	pub(crate) fn apply_extension_admin_action(
		&mut self,
		state: &mut State,
		action: AppAction,
		commands: &mut Vec<Command>,
	) -> Result<(), String> {
		action.validate().map_err(|error| error.to_string())?;
		let debounced = matches!(
			&action,
			AppAction::SetChannelMute { .. }
				| AppAction::SetChannelNotifications { .. }
				| AppAction::SetGuildHideMuted { .. }
		);
		let revision = state.revision;
		let command = match action {
			AppAction::SetChannelMute {
				channel_id,
				duration_seconds,
			} => {
				let channel = id(&channel_id)?;
				let mute = match duration_seconds {
					None => Mute::Unmute,
					Some(0) => Mute::Forever,
					Some(seconds) => Mute::For(seconds),
				};
				state.request_channel_action(
					channel,
					if state.is_thread_channel(channel) {
						Action::PostMute(mute)
					} else {
						Action::Mute(mute)
					},
				)
			}
			AppAction::SetChannelNotifications { channel_id, level } => {
				let channel = id(&channel_id)?;
				state.request_channel_action(
					channel,
					if state.is_thread_channel(channel) {
						Action::PostNotifications(level)
					} else {
						Action::Notifications(level)
					},
				)
			}
			AppAction::SetGuildHideMuted { guild_id, hide } => {
				let guild = id(&guild_id)?;
				let channel = guild_anchor(state, guild, false)
					.ok_or("The server has no accessible channel")?;
				state.request_channel_action(channel, Action::HideMuted(hide))
			}
			AppAction::CreateChannel {
				guild_id,
				name,
				kind,
			} => {
				let guild = id(&guild_id)?;
				let channel = guild_anchor(state, guild, true)
					.ok_or("The server has no manageable channel")?;
				let kind = match kind.as_str() {
					"text" => CreateKind::Text,
					"voice" => CreateKind::Voice,
					"forum" => CreateKind::Forum,
					_ => return Err("Invalid channel kind".into()),
				};
				state.request_channel_action(channel, Action::Create { name, kind })
			}
			AppAction::CreateCategory { guild_id, name } => {
				let guild = id(&guild_id)?;
				let channel = guild_anchor(state, guild, true)
					.ok_or("The server has no manageable channel")?;
				state.request_channel_action(channel, Action::CreateCategory { name })
			}
			AppAction::DuplicateChannel { channel_id, name } => {
				state.request_channel_action(id(&channel_id)?, Action::Duplicate { name })
			}
			AppAction::EditChannel {
				channel_id,
				before,
				after,
			} => state.request_channel_action(
				id(&channel_id)?,
				Action::Edit {
					before: edit(before)?,
					after: edit(after)?,
				},
			),
			AppAction::DeleteChannel { channel_id } => {
				state.request_channel_action(id(&channel_id)?, Action::Delete)
			}
			AppAction::MoveChannel {
				channel_id,
				parent_id,
				position,
				lock_permissions,
				shifts,
			} => state.request_channel_action(
				id(&channel_id)?,
				Action::Move {
					parent: parent_id.as_deref().map(id).transpose()?,
					position,
					lock_permissions,
					shifts: shifts
						.into_iter()
						.map(|row| Ok((id(&row.channel_id)?, row.position)))
						.collect::<Result<_, String>>()?,
				},
			),
			AppAction::CreateServerInvite {
				guild_id,
				channel_id,
				max_age,
				max_uses,
				temporary,
			} => {
				let guild = id(&guild_id)?;
				let channel = channel_id
					.as_deref()
					.map(id)
					.transpose()?
					.or_else(|| state.invite_channel(guild))
					.ok_or("The server has no channel where invites can be created")?;
				state.create_server_invite_with_options(
					guild,
					channel,
					InviteOptions {
						max_age,
						max_uses,
						temporary,
					},
				)
			}
			AppAction::LeaveServer { guild_id } => state.leave_server(id(&guild_id)?),
			AppAction::LeaveGroup { channel_id } => state.leave_group(id(&channel_id)?),
			AppAction::RenameGroup { channel_id, name } => {
				state.edit_group(id(&channel_id)?, Some(name), Patch::Absent)
			}
			AppAction::CloseDm { channel_id } => state.close_dm(id(&channel_id)?),
			AppAction::SetConversationMuted { channel_id, muted } => {
				state.set_dm_muted(id(&channel_id)?, muted)
			}
			other => return self.apply_extension_server_action(state, other, commands),
		};
		if command.is_none() && debounced && state.revision != revision {
			return Ok(());
		}
		queue(command, commands)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn known_state() -> State {
		let mut state = test_support::demo_state();
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::NotificationPreferences(
				client_core::notifications::Event::Updates {
					entries: vec![model::guild_preferences::Update {
						guild: Some(Id(10)),
						fields: model::guild_preferences::Fields {
							hide_muted: Patch::Value(false),
							..Default::default()
						},
						..Default::default()
					}],
					replace: false,
				},
			),
		});
		state
	}

	fn apply(action: AppAction) -> (State, Command) {
		let mut state = known_state();
		let mut commands = Vec::new();
		MessagingUi::default()
			.apply_extension_admin_action(&mut state, action, &mut commands)
			.unwrap();
		if commands.is_empty() {
			state.flush_channel_preferences();
			commands.extend(state.poll_channel_preferences(std::time::Instant::now()));
		}
		assert_eq!(commands.len(), 1);
		(state, commands.pop().unwrap())
	}

	#[test]
	fn channel_and_server_actions_use_native_admission() {
		let (_, command) = apply(AppAction::SetChannelMute {
			channel_id: "20".into(),
			duration_seconds: Some(3600),
		});
		let Command::ChannelPreferences(request) = command else {
			panic!("debounced native preferences")
		};
		assert_eq!(request.guild, Id(10));
		assert_eq!(request.patch.channels.len(), 1);
		assert_eq!(request.patch.channels[0].channel, Id(20));
		assert_eq!(request.patch.channels[0].fields.muted, Patch::Value(true));
		assert!(
			matches!(request.patch.channels[0].fields.mute_until,Patch::Value(until) if until>std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64)
		);

		let (_, command) = apply(AppAction::SetChannelNotifications {
			channel_id: "20".into(),
			level: 1,
		});
		let Command::ChannelPreferences(request) = command else {
			panic!("debounced native preferences")
		};
		assert_eq!(request.patch.channels[0].channel, Id(20));
		assert_eq!(request.patch.channels[0].fields.level, Patch::Value(1));
		assert_eq!(request.patch.channels[0].fields.muted, Patch::Absent);

		let (_, command) = apply(AppAction::SetGuildHideMuted {
			guild_id: "10".into(),
			hide: true,
		});
		let Command::ChannelPreferences(request) = command else {
			panic!("debounced native preferences")
		};
		assert_eq!(request.guild, Id(10));
		assert_eq!(request.patch.fields.hide_muted, Patch::Value(true));
		assert!(request.patch.channels.is_empty());

		let (_, command) = apply(AppAction::CreateChannel {
			guild_id: "10".into(),
			name: "sdk-room".into(),
			kind: "forum".into(),
		});
		assert!(
			matches!(command, Command::ChannelAction { guild: Id(10), action: Action::Create { name, kind: CreateKind::Forum }, .. } if name == "sdk-room")
		);

		let (_, command) = apply(AppAction::CreateServerInvite {
			guild_id: "10".into(),
			channel_id: Some("20".into()),
			max_age: 3600,
			max_uses: 5,
			temporary: true,
		});
		assert!(matches!(
			command,
			Command::ServerAction {
				action: client_core::server_actions::Action::CreateInvite {
					guild: Id(10),
					channel: Id(20),
					options: InviteOptions {
						max_age: 3600,
						max_uses: 5,
						temporary: true
					}
				},
				..
			}
		));
	}

	#[test]
	fn destructive_and_conversation_actions_keep_native_guards() {
		let (_, command) = apply(AppAction::DeleteChannel {
			channel_id: "20".into(),
		});
		assert!(matches!(
			command,
			Command::ChannelAction {
				channel: Id(20),
				action: Action::Delete,
				..
			}
		));

		let (_, command) = apply(AppAction::RenameGroup {
			channel_id: "29".into(),
			name: "SDK group".into(),
		});
		assert!(
			matches!(command, Command::GroupAction { action: client_core::group_actions::Action::Edit { channel: Id(29), name: Some(name), icon: Patch::Absent }, .. } if name == "SDK group")
		);

		let (_, command) = apply(AppAction::CloseDm {
			channel_id: "22".into(),
		});
		assert!(matches!(
			command,
			Command::UserAction {
				action: client_core::user_actions::Action::CloseDm(Id(22)),
				..
			}
		));

		let (_, command) = apply(AppAction::SetConversationMuted {
			channel_id: "29".into(),
			muted: true,
		});
		assert!(matches!(
			command,
			Command::UserAction {
				action: client_core::user_actions::Action::Mute {
					channel: Id(29),
					muted: true
				},
				..
			}
		));

		let mut state = known_state();
		let mut commands = Vec::new();
		assert!(
			MessagingUi::default()
				.apply_extension_admin_action(
					&mut state,
					AppAction::CloseDm {
						channel_id: "29".into()
					},
					&mut commands,
				)
				.is_err()
		);
		assert!(commands.is_empty());
	}
	#[test]
	fn accepted_preferences_are_successful_without_an_immediate_command_but_admission_still_applies()
	 {
		let mut state = known_state();
		let mut commands = Vec::new();
		let mut ui = MessagingUi::default();
		ui.apply_extension_admin_action(
			&mut state,
			AppAction::SetGuildHideMuted {
				guild_id: "10".into(),
				hide: true,
			},
			&mut commands,
		)
		.unwrap();
		assert!(commands.is_empty());
		assert_eq!(state.hides_muted_channels(Id(10)), Some(true));
		assert!(
			ui.apply_extension_admin_action(
				&mut state,
				AppAction::SetChannelNotifications {
					channel_id: "999999".into(),
					level: 1
				},
				&mut commands
			)
			.is_err()
		);
		assert!(commands.is_empty());
		state.demo = false;
		state.auth = client_core::auth::AuthState::Authenticated;
		state.gateway_connected = false;
		assert!(
			ui.apply_extension_admin_action(
				&mut state,
				AppAction::SetGuildHideMuted {
					guild_id: "10".into(),
					hide: false
				},
				&mut commands
			)
			.is_err()
		);
		assert_eq!(state.hides_muted_channels(Id(10)), Some(true));
		assert!(commands.is_empty());
	}
	#[test]
	fn unchanged_known_preference_is_a_success_but_an_unknown_preference_is_not_admitted() {
		let mut state = known_state();
		let mut commands = Vec::new();
		let mut ui = MessagingUi::default();
		ui.apply_extension_admin_action(
			&mut state,
			AppAction::SetGuildHideMuted {
				guild_id: "10".into(),
				hide: false,
			},
			&mut commands,
		)
		.unwrap();
		assert!(commands.is_empty());
		state.flush_channel_preferences();
		assert!(
			state
				.poll_channel_preferences(std::time::Instant::now())
				.is_none()
		);
		let mut unknown = test_support::demo_state();
		assert_eq!(unknown.hides_muted_channels(Id(10)), None);
		assert!(
			ui.apply_extension_admin_action(
				&mut unknown,
				AppAction::SetGuildHideMuted {
					guild_id: "10".into(),
					hide: true
				},
				&mut commands
			)
			.is_err()
		);
		assert!(commands.is_empty());
	}
}
