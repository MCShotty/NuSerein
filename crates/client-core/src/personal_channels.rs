//! Account channel selection and sidebar presentation; never grants channel access.
use crate::{Command, State, channel_actions::Action};
use model::{Id, channel_visibility as p};

impl State {
	pub fn can_personally_manage_channel(&self, channel: Id) -> bool {
		self.can_view(channel)
			|| self.channel(channel).is_some_and(|category| {
				category.kind == 4
					&& category.guild.is_some()
					&& self.channels.iter().any(|child| {
						child.guild == category.guild
							&& child.parent_id == Some(channel)
							&& self.can_view(child.id)
					})
			})
	}
	pub fn personal_channel_flags(&self, channel: Id) -> Option<u64> {
		let guild = self.channel(channel)?.guild?;
		self.account_channel_preference(guild, channel)
			.map(|(flags, _)| flags)
	}
	pub fn personally_shows_all_channels(&self, guild: Id) -> bool {
		self.account_guild_flags(guild).is_some_and(|flags| {
			flags & p::OPT_IN_CHANNELS_ON == 0 || flags & p::OPT_IN_CHANNELS_OFF != 0
		})
	}
	/// Saved membership is independent of Show All and temporary sidebar reveals.
	pub fn channel_in_custom_list(&self, channel: Id) -> Option<bool> {
		let source = self.channel(channel)?;
		let Some(guild) = source.guild else {
			return Some(true);
		};
		let flags = self.account_guild_flags(guild)?;
		if matches!(source.kind, 10..=12)
			&& let Some(parent) =
				source
					.parent_id
					.and_then(|id| self.channel(id))
					.filter(|parent| {
						parent.guild == Some(guild)
							&& matches!(parent.kind, 0 | 5 | 15 | 16)
							&& parent.id != source.id
							&& parent.parent_id != Some(source.id)
					}) {
			return self.channel_in_custom_list(parent.id);
		}
		if flags & p::GUILD_MASK == 0 {
			return Some(true);
		}
		let own = self.personal_channel_flags(channel)? & p::OPT_IN_ENABLED != 0;
		if own || source.kind == 4 {
			return Some(own);
		}
		if !matches!(source.kind, 10..=12)
			&& let Some(parent) =
				source
					.parent_id
					.and_then(|id| self.channel(id))
					.filter(|parent| {
						parent.guild == Some(guild) && parent.kind == 4 && parent.id != source.id
					}) {
			return self
				.personal_channel_flags(parent.id)
				.map(|flags| flags & p::OPT_IN_ENABLED != 0);
		}
		Some(false)
	}
	pub fn channel_personally_selected(&self, channel: Id) -> bool {
		let Some(source) = self.channel(channel) else {
			return false;
		};
		source
			.guild
			.is_none_or(|guild| self.personally_shows_all_channels(guild))
			|| self.channel_in_custom_list(channel) == Some(true)
	}
	pub fn channel_account_favorite(&self, channel: Id) -> bool {
		self.personal_channel_flags(channel)
			.is_some_and(|flags| flags & p::FAVORITED != 0)
	}
	pub fn account_category_collapsed(&self, category: Id) -> Option<bool> {
		let guild = self.channel(category)?.guild?;
		self.preference_bool(guild, Some(category), "collapsed")
	}
	pub fn personal_channel_controls_available(&self, guild: Id) -> bool {
		self.guild(guild).is_some()
			&& self.channel_preferences_known(guild)
			&& (self.demo || self.gateway_connected)
	}
	pub fn personal_change_allowed(&self, guild: Id, change: &p::Change, after: bool) -> bool {
		change.valid()
			&& self.guild(guild).is_some()
			&& change.guild_flags.is_none_or(|(old, new)| {
				self.account_guild_flags(guild) == Some(if after { new } else { old })
			}) && change.channels.iter().all(|row| {
			self.channel(row.channel).is_some_and(|source| {
				source.guild == Some(guild)
					&& self.can_personally_manage_channel(row.channel)
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
		if !self.personal_channel_controls_available(guild)
			|| !self.can_personally_manage_channel(target)
			|| matches!(source.kind, 10..=12)
		{
			return None;
		}
		let category = source.kind == 4;
		let all = self.personally_shows_all_channels(guild);
		let initialize = !shown && all && self.account_guild_flags(guild)? & p::GUILD_MASK == 0;
		let parent = (!category)
			.then(|| {
				source
					.parent_id
					.and_then(|id| self.channel(id))
					.filter(|parent| parent.kind == 4 && parent.guild == Some(guild))
			})
			.flatten();
		let break_category = !shown
			&& parent.is_some_and(|parent| {
				self.personal_channel_flags(parent.id)
					.is_some_and(|flags| flags & p::OPT_IN_ENABLED != 0)
			});
		let mut wanted = std::collections::BTreeMap::<Id, bool>::new();
		for channel in self.channels.iter().filter(|channel| {
			channel.guild == Some(guild)
				&& !matches!(channel.kind, 4 | 10..=12)
				&& self.can_view(channel.id)
		}) {
			if initialize {
				wanted.insert(channel.id, true);
			}
			if category && channel.parent_id == Some(target) && !shown {
				wanted.insert(channel.id, false);
			}
			if break_category && channel.parent_id == source.parent_id {
				wanted.insert(channel.id, self.channel_in_custom_list(channel.id)?);
			}
		}
		if break_category {
			wanted.insert(parent?.id, false);
		}
		wanted.insert(target, shown);
		let mut change = p::Change::default();
		if !shown && all {
			let before = self.account_guild_flags(guild)?;
			change.guild_flags = Some((before, (before & !p::GUILD_MASK) | p::OPT_IN_CHANNELS_ON));
		}
		for (channel, shown) in wanted {
			let before = self.personal_channel_flags(channel)?;
			let after = if shown {
				before | p::OPT_IN_ENABLED
			} else {
				before & !p::OPT_IN_ENABLED
			};
			if before != after {
				change.channels.push(p::RowChange {
					channel,
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
			if source.guild != Some(guild)
				|| source.kind != 4
				|| !self.can_personally_manage_channel(*id)
			{
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
pub(crate) mod tests {
	use super::*;
	use crate::{
		Envelope, Event,
		channel_actions::{self, Outcome},
		notifications::{self, Setting},
	};
	pub(crate) fn state() -> State {
		let mut state = State {
			demo: true,
			gateway_connected: true,
			user: Some(model::User {
				member_avatar: None,
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
					hide_muted_channels: Some(false),
					suppress_everyone: Some(false),
					suppress_roles: Some(false),
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
		assert!(
			state
				.request_personal_channel_action(Id(2), action)
				.is_none()
		);
		state.flush_channel_preferences();
		let Command::ChannelPreferences(request) = state
			.poll_channel_preferences(std::time::Instant::now())
			.expect("personal preference queued")
		else {
			panic!("personal preference command")
		};
		assert!(state.channel_preference_request_allowed(&request));
		let result = state.demo_channel_preferences(&request);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ChannelPreferences(result),
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
		state.request_channel_action(Id(3), action);
		state.flush_channel_preferences();
		let Command::ChannelPreferences(request) = state
			.poll_channel_preferences(std::time::Instant::now())
			.unwrap()
		else {
			panic!()
		};
		state
			.apply_guild_preference_update(model::guild_preferences::Update {
				guild: Some(Id(2)),
				fields: model::guild_preferences::Fields {
					flags: model::Patch::Value(p::OPT_IN_CHANNELS_OFF | (1 << 56)),
					..Default::default()
				},
				..Default::default()
			})
			.unwrap();
		let result = state.demo_channel_preferences(&request);
		state.finish_channel_preferences(result).unwrap();
		assert_eq!(
			state.account_guild_flags(Id(2)),
			Some(p::OPT_IN_CHANNELS_OFF | (1 << 56))
		);
		assert_eq!(
			state.channel_preference_status(Id(2), None),
			Some(crate::preferences_sync::Status::Conflict)
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
	#[test]
	fn hiding_during_show_all_does_not_unhide_earlier_choices() {
		let mut state = state();
		let action = state.personal_visibility_change(Id(4), false).unwrap();
		finish(&mut state, action);
		assert!(state.channel_personally_selected(Id(3)));
		assert!(!state.channel_personally_selected(Id(4)));
		let action = state.personal_show_all_change(Id(2), true).unwrap();
		finish(&mut state, action);
		assert!(state.channel_personally_selected(Id(4)));
		let action = state.personal_visibility_change(Id(6), false).unwrap();
		finish(&mut state, action);
		assert!(state.channel_personally_selected(Id(3)));
		assert!(
			!state.channel_personally_selected(Id(4)),
			"hiding a different channel must preserve the earlier hidden channel"
		);
		assert!(!state.channel_personally_selected(Id(6)));
	}
	#[test]
	fn accessible_children_allow_personal_category_controls_without_category_access() {
		let mut state = state();
		state
			.permissions
			.channels
			.get_mut(&Id(5))
			.unwrap()
			.overwrites = Some(vec![model::permissions::Overwrite {
			id: Id(2),
			kind: 0,
			allow: 0,
			deny: model::permissions::VIEW_CHANNEL,
		}]);
		state.permissions.clear_cache();
		assert!(!state.can_view(Id(5)) && state.can_view(Id(3)));
		let action = state
			.personal_collapse_change(Id(2), &[Id(5)], true)
			.unwrap();
		finish(&mut state, action);
		assert_eq!(state.account_category_collapsed(Id(5)), Some(true));
		let action = state.personal_visibility_change(Id(5), false).unwrap();
		finish(&mut state, action);
		assert!(
			!state.channel_personally_selected(Id(3)) && !state.channel_personally_selected(Id(4))
		);
		assert!(state.channel_personally_selected(Id(6)));
	}
	#[test]
	fn orphan_threads_keep_show_all_and_never_inherit_an_unrelated_parent() {
		let mut state = state();
		let template = state.channel(Id(3)).unwrap().clone();
		for (id, kind, parent, guild) in [
			(10, 11, 999, 2),
			(11, 11, 5, 2),
			(12, 11, 13, 2),
			(13, 12, 12, 2),
			(14, 11, 14, 2),
			(15, 11, 16, 2),
			(16, 0, 0, 99),
			(17, 11, 18, 2),
			(18, 0, 17, 2),
			(19, 11, 3, 2),
			(20, 11, 6, 2),
		] {
			state.channels.push(model::Channel {
				id: Id(id),
				guild: Some(Id(guild)),
				kind,
				parent_id: (parent != 0).then_some(Id(parent)),
				..template.clone()
			});
		}
		let orphans = [10, 11, 12, 13, 14, 15, 17, 20];
		for id in orphans {
			assert!(
				state.channel_personally_selected(Id(id)),
				"Show All retains orphan {id}"
			);
		}
		state
			.apply_notification_preferences(notifications::Event::Settings {
				entries: vec![Setting {
					guild: Some(Id(2)),
					flags: Some(p::OPT_IN_CHANNELS_ON),
					overrides_known: true,
					channel_flags: vec![
						(Id(3), Some(p::OPT_IN_ENABLED), Some(false)),
						(Id(5), Some(p::OPT_IN_ENABLED), Some(false)),
						(Id(6), Some(p::OPT_IN_ENABLED), Some(false)),
						(Id(10), Some(p::OPT_IN_ENABLED), Some(false)),
						(Id(18), Some(p::OPT_IN_ENABLED), Some(false)),
					],
					..Default::default()
				}],
				replace: false,
			})
			.unwrap();
		assert!(
			state.channel_personally_selected(Id(10)),
			"an orphan may retain its own selection"
		);
		for id in [11, 12, 13, 14, 15, 17, 20] {
			assert!(
				!state.channel_personally_selected(Id(id)),
				"orphan {id} must not inherit a category, voice, cyclic or other-server source"
			);
		}
		assert!(
			state.channel_personally_selected(Id(19)),
			"a valid thread inherits its text parent"
		);
		let action = state.personal_visibility_change(Id(3), false).unwrap();
		finish(&mut state, action);
		assert!(!state.channel_personally_selected(Id(19)));
	}
}
