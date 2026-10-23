//! Account channel selection and sidebar presentation; never grants channel access.
use crate::{Command, State, channel_actions::Action};
use model::{Id, channel_visibility as p};

impl State {
	pub fn personal_channel_flags(&self, channel: Id) -> Option<u64> {
		let guild = self.channel(channel)?.guild?;
		self.account_channel_preference(guild, channel)
			.map(|(flags, _)| flags)
	}
	pub fn personally_shows_all_channels(&self, guild: Id) -> bool {
		self.account_guild_flags(guild).is_none_or(|flags| {
			flags & p::OPT_IN_CHANNELS_ON == 0 || flags & p::OPT_IN_CHANNELS_OFF != 0
		})
	}
	pub fn channel_personally_selected(&self, channel: Id) -> bool {
		let Some(source) = self.channel(channel) else {
			return false;
		};
		let Some(guild) = source.guild else {
			return true;
		};
		if matches!(source.kind, 10..=12) {
			return source
				.parent_id
				.and_then(|id| self.channel(id))
				.filter(|parent| parent.guild == Some(guild) && !matches!(parent.kind, 10..=12))
				.is_some_and(|parent| self.channel_personally_selected(parent.id));
		}
		self.personally_shows_all_channels(guild)
			|| self
				.personal_channel_flags(channel)
				.is_some_and(|flags| flags & p::OPT_IN_ENABLED != 0)
	}
	pub fn channel_account_favorite(&self, channel: Id) -> bool {
		self.personal_channel_flags(channel)
			.is_some_and(|flags| flags & p::FAVORITED != 0)
	}
	pub fn account_category_collapsed(&self, category: Id) -> Option<bool> {
		let guild = self.channel(category)?.guild?;
		self.account_channel_preference(guild, category)
			.map(|(_, collapsed)| collapsed)
	}
	pub fn personal_channel_controls_available(&self, guild: Id) -> bool {
		self.guild(guild).is_some()
			&& self.account_guild_flags(guild).is_some()
			&& (self.demo || self.gateway_connected)
			&& (!self.channel_action_pending() || self.channel_description_pending())
	}
	pub fn personal_change_allowed(&self, guild: Id, change: &p::Change, after: bool) -> bool {
		change.valid()
			&& self.guild(guild).is_some()
			&& change.guild_flags.is_none_or(|(old, new)| {
				self.account_guild_flags(guild) == Some(if after { new } else { old })
			}) && change.channels.iter().all(|row| {
			self.channel(row.channel).is_some_and(|source| {
				source.guild == Some(guild)
					&& self.can_view(row.channel)
					&& (row.collapsed.is_none() || source.kind == 4)
			}) && self
				.account_channel_preference(guild, row.channel)
				.is_some_and(|(flags, collapsed)| {
					row.flags
						.is_none_or(|(old, new)| flags == if after { new } else { old })
						&& row
							.collapsed
							.is_none_or(|(old, new)| collapsed == if after { new } else { old })
				})
		})
	}
	pub fn personal_visibility_change(&self, target: Id, shown: bool) -> Option<Action> {
		let source = self.channel(target)?;
		let guild = source.guild?;
		if !self.personal_channel_controls_available(guild) || !self.can_view(target) {
			return None;
		}
		let category = source.kind == 4;
		let targeted = |channel: &model::Channel| {
			channel.id == target || (category && channel.parent_id == Some(target))
		};
		let mut change = p::Change::default();
		let all = self.personally_shows_all_channels(guild);
		if !shown && all {
			let before = self.account_guild_flags(guild)?;
			change.guild_flags = Some((before, (before & !p::GUILD_MASK) | p::OPT_IN_CHANNELS_ON));
		}
		for channel in self.channels.iter().filter(|channel| {
			channel.guild == Some(guild)
				&& !matches!(channel.kind, 4 | 10..=12)
				&& self.can_view(channel.id)
		}) {
			let selected = if targeted(channel) {
				shown
			} else if !shown && all {
				true
			} else {
				continue;
			};
			let (before, _) = self.account_channel_preference(guild, channel.id)?;
			let after = if selected {
				before | p::OPT_IN_ENABLED
			} else {
				before & !p::OPT_IN_ENABLED
			};
			if before != after {
				change.channels.push(p::RowChange {
					channel: channel.id,
					flags: Some((before, after)),
					collapsed: None,
				});
			}
			if change.channels.len() > p::MAX_CHANGES {
				return None;
			}
		}
		change.channels.shrink_to_fit();
		change.valid().then_some(Action::Personal(Box::new(change)))
	}
	pub fn personal_show_all_change(&self, guild: Id, all: bool) -> Option<Action> {
		if !self.personal_channel_controls_available(guild) {
			return None;
		}
		let before = self.account_guild_flags(guild)?;
		let after = (before & !p::GUILD_MASK)
			| if all {
				p::OPT_IN_CHANNELS_OFF
			} else {
				p::OPT_IN_CHANNELS_ON
			};
		let mut change = p::Change {
			guild_flags: (before != after).then_some((before, after)),
			channels: Vec::new(),
		};
		if !all && before & p::GUILD_MASK == 0 {
			for channel in self.channels.iter().filter(|channel| {
				channel.guild == Some(guild)
					&& !matches!(channel.kind, 4 | 10..=12)
					&& self.can_view(channel.id)
			}) {
				let (before, _) = self.account_channel_preference(guild, channel.id)?;
				if before & p::OPT_IN_ENABLED == 0 {
					change.channels.push(p::RowChange {
						channel: channel.id,
						flags: Some((before, before | p::OPT_IN_ENABLED)),
						collapsed: None,
					});
				}
				if change.channels.len() > p::MAX_CHANGES {
					return None;
				}
			}
			change.channels.shrink_to_fit();
		}
		change.valid().then_some(Action::Personal(Box::new(change)))
	}
	pub fn personal_channel_flag_change(
		&self,
		channel: Id,
		mask: u64,
		enabled: u64,
	) -> Option<Action> {
		let guild = self.channel(channel)?.guild?;
		if mask & !p::CHANNEL_MASK != 0
			|| enabled & !mask != 0
			|| !self.personal_channel_controls_available(guild)
			|| !self.can_view(channel)
		{
			return None;
		}
		let (before, _) = self.account_channel_preference(guild, channel)?;
		let after = (before & !mask) | enabled;
		let change = p::Change {
			guild_flags: None,
			channels: vec![p::RowChange {
				channel,
				flags: Some((before, after)),
				collapsed: None,
			}],
		};
		change.valid().then_some(Action::Personal(Box::new(change)))
	}
	pub fn personal_collapse_change(
		&self,
		guild: Id,
		categories: &[Id],
		collapsed: bool,
	) -> Option<Action> {
		if !self.personal_channel_controls_available(guild) || categories.len() > p::MAX_CHANGES {
			return None;
		}
		let mut change = p::Change::default();
		for id in categories {
			let source = self.channel(*id)?;
			if source.guild != Some(guild) || source.kind != 4 || !self.can_view(*id) {
				return None;
			}
			let (_, before) = self.account_channel_preference(guild, *id)?;
			if before != collapsed {
				change.channels.push(p::RowChange {
					channel: *id,
					flags: None,
					collapsed: Some((before, collapsed)),
				});
			}
		}
		change.channels.shrink_to_fit();
		change.valid().then_some(Action::Personal(Box::new(change)))
	}
	pub fn request_personal_channel_action(
		&mut self,
		guild: Id,
		action: Action,
	) -> Option<Command> {
		let channel = self
			.channels
			.iter()
			.find(|channel| channel.guild == Some(guild) && self.can_view(channel.id))?
			.id;
		self.request_channel_action(channel, action)
	}
	pub fn personal_import_favorites(&self, guild: Id, favorites: &[Id]) -> Option<Action> {
		if favorites.len() > model::ChannelPreferences::MAX_ENTRIES
			|| !self.personal_channel_controls_available(guild)
		{
			return None;
		}
		let mut change = p::Change::default();
		for id in favorites.iter().copied() {
			if !self
				.channel(id)
				.is_some_and(|c| c.guild == Some(guild) && c.kind != 4 && self.can_view(id))
				|| change.channels.iter().any(|row| row.channel == id)
			{
				continue;
			}
			let (before, _) = self.account_channel_preference(guild, id)?;
			if before & p::FAVORITED == 0 {
				change.channels.push(p::RowChange {
					channel: id,
					flags: Some((before, before | p::FAVORITED)),
					collapsed: None,
				});
			}
		}
		change.channels.shrink_to_fit();
		change.valid().then_some(Action::Personal(Box::new(change)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Envelope, Event,
		channel_actions::{self, Outcome},
		notifications::{self, Setting},
	};
	fn state() -> State {
		let mut state = State {
			demo: true,
			gateway_connected: true,
			user: Some(model::User {
				id: Id(1),
				name: "Owner of this session".into(),
				username: None,
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
			}),
			guilds: vec![model::Guild {
				id: Id(2),
				name: "Synthetic".into(),
				icon: None,
				emojis: None,
				stickers: None,
				default_message_notifications: Some(0),
			}],
			channels: (3..=6)
				.map(|id| model::Channel {
					id: Id(id),
					guild: Some(Id(2)),
					name: format!("channel-{id}"),
					kind: if id == 5 {
						4
					} else if id == 6 {
						2
					} else {
						0
					},
					parent_id: if id == 3 || id == 4 {
						Some(Id(5))
					} else {
						None
					},
					position: id as i32,
					icon: None,
					last_message: None,
					recipients: vec![],
					member_list_id: None,
					message_count: None,
					tags: None,
				})
				.collect(),
			..State::default()
		};
		state
			.permissions
			.replace(model::permissions::Snapshot {
				guilds: vec![model::permissions::Guild {
					id: Id(2),
					owner: Some(Id(99)),
					roles: Some(vec![model::permissions::Role {
						id: Id(2),
						bits: model::permissions::VIEW_CHANNEL
							| model::permissions::READ_MESSAGE_HISTORY,
						name: "everyone".into(),
						icon: None,
						unicode_emoji: None,
						color: 0,
						secondary_color: None,
						tertiary_color: None,
						position: 0,
						hoist: false,
					}]),
					member: Some(model::permissions::Member {
						roles: vec![],
						timeout_until: None,
					}),
				}],
				channels: (3..=6)
					.map(|id| model::permissions::Channel {
						id: Id(id),
						guild: Id(2),
						overwrites: Some(vec![]),
					})
					.collect(),
			})
			.unwrap();
		state
			.apply_notification_preferences(notifications::Event::Settings {
				entries: vec![Setting {
					guild: Some(Id(2)),
					flags: Some(1 << 55),
					overrides_known: true,
					muted: Some(false),
					level: Some(0),
					channels: vec![(Id(3), Some(true), Some(2))],
					channel_flags: vec![(Id(3), Some(1 << 54), Some(false))],
					..Setting::default()
				}],
				replace: true,
			})
			.unwrap();
		state
	}
	fn finish(state: &mut State, action: Action) {
		let command = state
			.request_personal_channel_action(Id(2), action)
			.expect("ordinary member can change personal preferences");
		let Command::ChannelAction {
			guild,
			channel,
			request,
			action: Action::Personal(change),
		} = command
		else {
			panic!("personal command")
		};
		assert!(state.personal_channel_dispatch_allowed(
			guild,
			channel,
			request,
			&Action::Personal(change.clone())
		));
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelAction(channel_actions::Event::Finished {
				guild,
				channel,
				request,
				result: Ok(Outcome::Personal(change)),
			}),
		});
	}
	#[test]
	fn first_hide_preserves_other_channels_unknown_flags_and_notifications() {
		let mut state = state();
		assert!(!state.can_manage_guild(Id(2)));
		let action = state.personal_visibility_change(Id(3), false).unwrap();
		finish(&mut state, action);
		assert!(!state.channel_personally_selected(Id(3)));
		assert!(state.channel_personally_selected(Id(4)));
		assert!(state.channel_personally_selected(Id(6)));
		assert_eq!(state.personal_channel_flags(Id(3)), Some(1 << 54));
		assert_eq!(
			state.account_guild_flags(Id(2)),
			Some((1 << 55) | p::OPT_IN_CHANNELS_ON)
		);
		assert_eq!(state.guild_channel_muted(Id(3)), Some(true));
		assert_eq!(state.channel_notification_level(Id(3)), Some(2));
		let action = state.personal_show_all_change(Id(2), true).unwrap();
		finish(&mut state, action);
		assert!(state.channel_personally_selected(Id(3)));
		let action = state.personal_show_all_change(Id(2), false).unwrap();
		finish(&mut state, action);
		assert!(
			!state.channel_personally_selected(Id(3)),
			"Show All preserves hidden choices"
		);
		let action = state.personal_visibility_change(Id(3), true).unwrap();
		finish(&mut state, action);
		assert!(state.channel_personally_selected(Id(3)));
		assert_eq!(
			state.guild_channel_muted(Id(3)),
			Some(true),
			"restoring never unmutes"
		);
	}
	#[test]
	fn first_custom_list_and_category_changes_are_recoverable() {
		let mut state = state();
		let action = state.personal_show_all_change(Id(2), false).unwrap();
		finish(&mut state, action);
		assert!(
			[3, 4, 6]
				.iter()
				.all(|id| state.channel_personally_selected(Id(*id)))
		);
		let action = state.personal_visibility_change(Id(5), false).unwrap();
		finish(&mut state, action);
		assert!(
			!state.channel_personally_selected(Id(3)) && !state.channel_personally_selected(Id(4))
		);
		assert!(state.channel_personally_selected(Id(6)));
		let action = state.personal_visibility_change(Id(5), true).unwrap();
		finish(&mut state, action);
		let action = state
			.personal_collapse_change(Id(2), &[Id(5)], true)
			.unwrap();
		finish(&mut state, action);
		assert_eq!(state.account_category_collapsed(Id(5)), Some(true));
		assert!(state.channel_personally_selected(Id(3)));
	}
	#[test]
	fn favorites_are_explicit_and_preserve_other_bits() {
		let mut state = state();
		assert!(!state.channel_account_favorite(Id(3)));
		let action = state
			.personal_import_favorites(Id(2), &[Id(3), Id(3), Id(999)])
			.unwrap();
		finish(&mut state, action);
		assert!(state.channel_account_favorite(Id(3)));
		assert_eq!(
			state.personal_channel_flags(Id(3)),
			Some((1 << 54) | p::FAVORITED)
		);
		assert!(state.personally_shows_all_channels(Id(2)));
	}
	#[test]
	fn changed_settings_and_lost_permissions_retire_queued_writes() {
		let mut state = state();
		let action = state.personal_visibility_change(Id(3), false).unwrap();
		let command = state.request_channel_action(Id(3), action).unwrap();
		let Command::ChannelAction {
			guild,
			channel,
			request,
			action,
		} = command
		else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::NotificationPreferences(notifications::Event::Settings {
				entries: vec![Setting {
					guild: Some(guild),
					flags: Some(p::OPT_IN_CHANNELS_OFF | (1 << 56)),
					..Setting::default()
				}],
				replace: false,
			}),
		});
		assert!(!state.personal_channel_dispatch_allowed(guild, channel, request, &action));
		let Action::Personal(change) = action else {
			panic!()
		};
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelAction(channel_actions::Event::Finished {
				guild,
				channel,
				request,
				result: Ok(Outcome::Personal(change)),
			}),
		});
		assert_eq!(
			state.account_guild_flags(guild),
			Some(p::OPT_IN_CHANNELS_OFF | (1 << 56))
		);
		state.permissions.channels.remove(&Id(3));
		state.permissions.clear_cache();
		assert!(state.personal_visibility_change(Id(3), false).is_none());
		assert!(state.personal_import_favorites(Id(2), &[Id(3)]).is_none());
	}
	#[test]
	fn explicit_changes_supersede_one_optional_topic_read() {
		let mut state = state();
		let Command::ChannelAction {
			guild,
			channel,
			request,
			..
		} = state
			.request_channel_action(Id(3), Action::Description)
			.unwrap()
		else {
			panic!()
		};
		assert!(state.personal_channel_controls_available(guild));
		let action = state.personal_visibility_change(Id(4), false).unwrap();
		finish(&mut state, action);
		assert!(!state.channel_personally_selected(Id(4)));
		assert!(
			state
				.request_channel_action(Id(3), Action::Description)
				.is_none(),
			"another automatic read must not queue behind the retired read"
		);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelAction(channel_actions::Event::Finished {
				guild,
				channel,
				request,
				result: Ok(Outcome::Description("stale topic".into())),
			}),
		});
		assert_eq!(
			state.channel_description(Id(3)),
			None,
			"retired response must not populate cache"
		);
		assert!(
			state
				.request_channel_action(Id(3), Action::Description)
				.is_some()
		);
	}
}
