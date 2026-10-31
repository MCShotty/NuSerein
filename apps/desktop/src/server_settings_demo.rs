//! Explicit offline fixture; never loads a saved session or changes a real guild.
use client_core::{Command, Envelope, Event, State};
use model::{
	Id,
	server_settings::{Edit, Settings, Trait},
};

fn snapshot(state: &State, guild: Id) -> Settings {
	let server = state
		.guilds
		.iter()
		.find(|server| server.id == guild)
		.expect("fixture guild");
	Settings {
		guild,
		name: server.name.clone(),
		icon: server.icon.clone(),
		banner_color: Some(0x245ae8),
		traits: vec![
			Trait {
				label: "Games".into(),
				emoji: Some("🎮".into()),
			},
			Trait {
				label: "Community".into(),
				emoji: None,
			},
		],
		description: "A synthetic workspace for friends, games and conversation.".into(),
		online_count: Some(4),
		member_count: Some(9),
		system_channel_id: state
			.channels
			.iter()
			.find(|channel| channel.guild == Some(guild) && channel.kind == 0)
			.map(|channel| channel.id),
		default_message_notifications: 1,
		activity_feed: Some(true),
		verification_level: 1,
		explicit_content_filter: 2,
		features: vec![
			"COMMUNITY".into(),
			model::server_settings::ACTIVITY_ENABLED.into(),
		],
		..Default::default()
	}
}

pub fn execute(state: &State, guild: Id, request: u64, edit: Option<Box<Edit>>) -> Event {
	let mut value = state
		.server_settings
		.snapshot
		.as_ref()
		.filter(|value| value.guild == guild)
		.cloned()
		.unwrap_or_else(|| snapshot(state, guild));
	if let Some(edit) = edit {
		edit.apply(&mut value);
		if matches!(edit.icon, model::Patch::Value(_)) {
			value.icon = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
		}
	}
	assert!(value.valid(), "synthetic server settings remain valid");
	Event::ServerSettings(client_core::server_settings::Event {
		guild,
		request,
		result: Ok(Box::new(value)),
		refreshed: None,
	})
}

pub fn execute_admin(
	state: &State,
	guild: Id,
	request: u64,
	action: model::server_admin::Action,
) -> Event {
	use model::server_admin::{
		Action, Emoji, Emojis, Member, Members, Result as Outcome, Role, Sound, Sounds, Sticker,
		Stickers,
	};
	assert!(
		action.valid(),
		"synthetic admin action must satisfy wire bounds"
	);
	let owner = state.user.as_ref().expect("fixture user");
	let result = match action {
		Action::CommandPermissions(action) => {
			match execute_command_permissions(state, guild, action) {
				Ok(page) => Outcome::CommandPermissions(page),
				Err(failure) => {
					return Event::ServerAdmin(client_core::server_admin::Event {
						guild,
						request,
						result: Err(failure),
					});
				}
			}
		}
		Action::Access(action) => Outcome::Access(execute_access(state, guild, action)),
		Action::Community(action) => {
			Outcome::Community(crate::community_admin_demo::execute(state, guild, action))
		}
		Action::AutoMod(action) => {
			let mut page =
				state
					.server_admin
					.automod
					.clone()
					.unwrap_or_else(|| model::automod::Snapshot {
						guild,
						rules: vec![model::automod::Rule {
							id: Id(9901),
							guild,
							creator: owner.id,
							settings: model::automod::Settings {
								name: "Synthetic keyword filter".into(),
								metadata: model::automod::Metadata {
									keywords: vec!["sample phrase".into()],
									..Default::default()
								},
								..Default::default()
							},
						}],
					});
			match action {
				model::automod::Action::Load => {}
				model::automod::Action::Create { settings, .. } => {
					let id = Id(page
						.rules
						.iter()
						.map(|rule| rule.id.0)
						.max()
						.unwrap_or(9900)
						.saturating_add(1));
					page.rules.push(model::automod::Rule {
						id,
						guild,
						creator: owner.id,
						settings: *settings,
					});
				}
				model::automod::Action::Edit { id, after, .. } => {
					if let Some(rule) = page.rules.iter_mut().find(|rule| rule.id == id) {
						rule.settings = *after;
					}
				}
				model::automod::Action::Delete { id, .. } => {
					page.rules.retain(|rule| rule.id != id)
				}
			}
			Outcome::AutoMod(page)
		}
		Action::Moderation(action) => {
			use model::moderation::{Action as M, Change, Outcome as O};
			let result = match action {
				M::Bans { after } => {
					let mut user = owner.clone();
					user.id = Id(9902);
					user.name = "Synthetic banned member".into();
					let items = if after.is_none() {
						vec![model::moderation::Ban {
							user,
							reason: Some("Synthetic moderation reason".into()),
						}]
					} else {
						vec![]
					};
					O::Bans(model::moderation::Bans {
						guild,
						items,
						next: None,
					})
				}
				M::BanSearch { query } => {
					let mut user = owner.clone();
					user.id = Id(9902);
					user.name = query;
					O::Bans(model::moderation::Bans {
						guild,
						items: vec![model::moderation::Ban {
							user,
							reason: Some("Synthetic moderation reason".into()),
						}],
						next: None,
					})
				}
				M::BulkBan { users, .. } => O::Bulk(model::moderation::BulkReport::pending(&users)),
				M::BulkRefresh { users } => O::Bulk(model::moderation::BulkReport {
					users: users
						.into_iter()
						.map(|id| (id, model::moderation::BanStatus::Banned))
						.collect(),
					accepted: true,
				}),
				M::Review {
					user,
					channel,
					audit,
				} => {
					let member = state
						.server_admin
						.members
						.as_ref()
						.and_then(|page| page.items.iter().find(|row| row.user.id == user))
						.cloned()
						.unwrap_or_else(|| Member {
							user: model::User {
								member_avatar: None,
								id: user,
								name: "Synthetic member".into(),
								..owner.clone()
							},
							nick: Some("Server nickname".into()),
							roles: vec![],
							joined_at: Some(1_750_000_000_000),
							join_source: Some(5),
							invite_code: None,
							flags: Some(0),
							unusual_dm_until: None,
							timeout_until: None,
						});
					let messages = state
						.timeline
						.iter()
						.filter(|message| message.author.id == user)
						.take(3)
						.map(|message| model::moderation::ReviewMessage {
							id: message.id,
							channel: message.channel,
							excerpt: message.content.clone(),
						})
						.collect();
					O::Review(Box::new(model::moderation::Review {
						guild,
						channel,
						member,
						roles: state
							.server_admin
							.members
							.as_ref()
							.map_or(Vec::new(), |page| page.roles.clone()),
						messages,
						audit: if audit {
							vec![model::moderation::ReviewAudit {
								id: Id(1_000_000_000_000_000_000),
								actor: Some(owner.id),
								action: 24,
								reason: Some("Synthetic role update".into()),
							}]
						} else {
							vec![]
						},
						audit_window: audit.then_some((
							Id(1_000_000_000_000_000_000),
							Id(1_000_000_000_000_000_000),
						)),
						messages_error: None,
						audit_error: None,
					}))
				}
				M::Member(id) => {
					let mut user = owner.clone();
					user.id = id;
					user.name = "Synthetic member".into();
					O::Member(Box::new(Member {
						user,
						nick: None,
						roles: vec![],
						joined_at: None,
						join_source: None,
						invite_code: None,
						flags: None,
						unusual_dm_until: None,
						timeout_until: None,
					}))
				}
				M::Unban { user, .. } => O::Changed { user, member: None },
				M::Change { user, change, .. } => {
					let member = if let Change::Timeout { until } = change {
						state
							.server_admin
							.moderation_target
							.clone()
							.map(|mut member| {
								member.timeout_until = until.map(|at| at / 1_000_000);
								Box::new(member)
							})
					} else {
						None
					};
					O::Changed { user, member }
				}
			};
			Outcome::Moderation(result)
		}
		Action::AuditLog(query) => execute_audit_log(state, guild, query),
		Action::Invites(action) => execute_invites(state, guild, action),
		Action::Integrations(action) => execute_integrations(state, guild, action),
		Action::Roles(action) => execute_roles(state, guild, action),
		Action::LoadEmojis
		| Action::CreateEmoji { .. }
		| Action::RenameEmoji { .. }
		| Action::DeleteEmoji { .. } => {
			let mut page = state.server_admin.emojis.clone().unwrap_or_else(|| Emojis {
				items: vec![Emoji {
					emoji: model::CustomEmoji {
						id: Id(9001),
						name: "serein".into(),
						animated: false,
						available: true,
						managed: false,
						roles: Some(Vec::new()),
					},
					uploader: Some(owner.clone()),
				}],
				static_limit: Some(50),
				animated_limit: Some(50),
			});
			match action {
				Action::CreateEmoji { name, image } => {
					let id = Id(page
						.items
						.iter()
						.map(|row| row.emoji.id.0)
						.max()
						.unwrap_or(9000) + 1);
					page.items.push(Emoji {
						emoji: model::CustomEmoji {
							id,
							name,
							animated: image.starts_with("data:image/gif"),
							available: true,
							managed: false,
							roles: Some(Vec::new()),
						},
						uploader: Some(owner.clone()),
					});
				}
				Action::RenameEmoji { id, name } => {
					if let Some(row) = page.items.iter_mut().find(|row| row.emoji.id == id) {
						row.emoji.name = name;
					}
				}
				Action::DeleteEmoji { id } => page.items.retain(|row| row.emoji.id != id),
				_ => {}
			}
			Outcome::Emojis(page)
		}
		Action::LoadStickers
		| Action::CreateSticker { .. }
		| Action::EditSticker { .. }
		| Action::DeleteSticker { .. } => {
			let mut page = state
				.server_admin
				.stickers
				.clone()
				.unwrap_or_else(|| Stickers {
					items: [
						(9101, "Wave", "hello,wave"),
						(9102, "Smile", "smile,happy"),
						(9103, "Celebrate", "party,celebrate"),
					]
					.into_iter()
					.map(|(id, name, tags)| Sticker {
						sticker: model::Sticker {
							id: Id(id),
							name: name.into(),
							description: "Original synthetic sticker artwork".into(),
							tags: tags.into(),
							format_type: 1,
							guild_id: Some(guild),
							pack_id: None,
							available: true,
						},
						uploader: Some(owner.clone()),
					})
					.collect(),
					limit: Some(5),
				});
			match action {
				Action::CreateSticker {
					name,
					description,
					tags,
					..
				} => {
					let id = Id(page
						.items
						.iter()
						.map(|row| row.sticker.id.0)
						.max()
						.unwrap_or(9100) + 1);
					page.items.push(Sticker {
						sticker: model::Sticker {
							id,
							name,
							description,
							tags,
							format_type: 1,
							guild_id: Some(guild),
							pack_id: None,
							available: true,
						},
						uploader: Some(owner.clone()),
					});
				}
				Action::EditSticker {
					id,
					name,
					description,
					tags,
				} => {
					if let Some(row) = page.items.iter_mut().find(|row| row.sticker.id == id) {
						row.sticker.name = name;
						row.sticker.description = description;
						row.sticker.tags = tags;
					}
				}
				Action::DeleteSticker { id } => page.items.retain(|row| row.sticker.id != id),
				_ => {}
			}
			Outcome::Stickers(page)
		}
		Action::LoadSounds
		| Action::CreateSound { .. }
		| Action::EditSound { .. }
		| Action::DeleteSound { .. } => {
			let sound = |id: u64, name: &str, emoji: Option<&str>, volume: f32| Sound {
				sound: model::soundboard::Sound {
					id: Id(id),
					name: name.into(),
					volume,
					emoji_id: None,
					emoji_name: emoji.map(str::to_owned),
					guild: Some(guild),
					available: true,
				},
				uploader: Some(owner.clone()),
			};
			let mut page = state.server_admin.sounds.clone().unwrap_or_else(|| Sounds {
				items: vec![
					sound(9301, "Synthetic fanfare", Some("\u{1f3ba}"), 1.0),
					sound(9302, "Drum roll", Some("\u{1f941}"), 0.8),
					sound(9303, "A sound without an emoji", None, 0.5),
				],
				limit: Some(8),
			});
			match action {
				Action::CreateSound {
					name,
					emoji,
					volume,
					..
				} => {
					let id = page
						.items
						.iter()
						.map(|row| row.sound.id.0)
						.max()
						.unwrap_or(9300) + 1;
					page.items.push(sound(
						id,
						&name,
						(!emoji.is_empty()).then_some(emoji.as_str()),
						f32::from(volume) / 100.0,
					));
				}
				Action::EditSound {
					id,
					name,
					emoji,
					volume,
				} => {
					if let Some(row) = page.items.iter_mut().find(|row| row.sound.id == id) {
						row.sound.name = name;
						row.sound.volume = f32::from(volume) / 100.0;
						match emoji {
							model::Patch::Absent => {}
							model::Patch::Null => row.sound.emoji_name = None,
							model::Patch::Value(emoji) => row.sound.emoji_name = Some(emoji),
						}
					}
				}
				Action::DeleteSound { id } => page.items.retain(|row| row.sound.id != id),
				_ => {}
			}
			Outcome::Sounds(page)
		}
		Action::LoadMembers(query) => {
			let mut page = {
				let roles: Vec<_> = state
					.permissions
					.guilds
					.get(&guild)
					.and_then(|guild| guild.roles.as_ref())
					.into_iter()
					.flatten()
					.map(|role| Role {
						role: role.clone(),
						managed: false,
					})
					.collect();
				let roles = state.server_admin.roles.as_ref().map_or(roles, |catalog| {
					catalog
						.items
						.iter()
						.map(|role| Role {
							role: role.permission_role(),
							managed: role.managed,
						})
						.collect()
				});
				let items = [
					"Avery", "Mika", "Rowan", "Sam", "Taylor", "Morgan", "Alex", "Jamie",
				]
				.into_iter()
				.enumerate()
				.map(|(index, name)| {
					let mut user = owner.clone();
					user.id = Id(((1_620_070_400_000u64 + index as u64 * 86_400_000
						- 1_420_070_400_000)
						<< 22) | 1);
					user.name = name.into();
					user.avatar = None;
					Member {
						user,
						nick: None,
						roles: roles
							.iter()
							.filter(|role| role.role.id != guild)
							.skip(index % 4)
							.take(1)
							.map(|role| role.role.id)
							.collect(),
						joined_at: Some(1_789_200_000_000 - index as i128 * 86_400_000),
						join_source: Some(1),
						invite_code: Some("demo".into()),
						flags: Some(0),
						unusual_dm_until: None,
						timeout_until: None,
					}
				})
				.collect::<Vec<_>>();
				Members {
					total: items.len() as u64,
					items,
					roles,
					next: None,
					features: Vec::new(),
					show_in_channel_list: Some(false),
				}
			};
			if let Some(cached) = &state.server_admin.members {
				for row in &mut page.items {
					if let Some(previous) = cached
						.items
						.iter()
						.find(|previous| previous.user.id == row.user.id)
					{
						*row = previous.clone();
					}
				}
			}
			page.items.retain(|row| {
				query.search.is_empty()
					|| row
						.user
						.name
						.to_lowercase()
						.contains(&query.search.to_lowercase())
					|| row.user.id.to_string() == query.search
			});
			match query.sort {
				2 => page.items.sort_by_key(|row| row.joined_at),
				3 => page.items.sort_by_key(|row| std::cmp::Reverse(row.user.id)),
				4 => page.items.sort_by_key(|row| row.user.id),
				_ => page
					.items
					.sort_by_key(|row| std::cmp::Reverse(row.joined_at)),
			}
			Outcome::Members(page)
		}
		Action::SetRole { user, .. } | Action::SetNickname { user, .. } => {
			let mut member = state
				.server_admin
				.members
				.as_ref()
				.and_then(|page| page.items.iter().find(|row| row.user.id == user))
				.expect("fixture target member")
				.clone();
			match action {
				Action::SetRole { role, assigned, .. } => {
					member.roles.retain(|id| *id != role);
					if assigned {
						member.roles.push(role);
					}
				}
				Action::SetNickname { nick, .. } => {
					member.nick = (!nick.is_empty()).then_some(nick)
				}
				_ => {}
			}
			Outcome::Member(member)
		}
		Action::Voice { user, .. } => Outcome::VoiceUpdated(user),
		Action::Kick { user } => Outcome::Kicked(user),
		Action::Prune { .. } => Outcome::Pruned(Some(0)),
		Action::ShowMembers { enabled } => Outcome::ChannelList(enabled),
	};
	assert!(
		result.valid(),
		"synthetic administration response must stay bounded"
	);
	Event::ServerAdmin(client_core::server_admin::Event {
		guild,
		request,
		result: Ok(result),
	})
}

fn execute_access(
	state: &State,
	guild: Id,
	action: model::server_access::Action,
) -> model::server_access::Outcome {
	use model::server_access::{
		Action, Answer, Application, Applications, Configuration, Incident, Outcome, Settings,
		Status,
	};
	let owner = state.user.as_ref().expect("fixture user");
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos() as i128;
	let application = |id: Id, status: Status| {
		state
			.server_admin
			.access
			.detail
			.as_ref()
			.filter(|v| v.id == id)
			.or_else(|| {
				state
					.server_admin
					.access
					.applications
					.as_ref()?
					.items
					.iter()
					.find(|v| v.id == id)
			})
			.cloned()
			.unwrap_or_else(|| {
				let mut user = owner.clone();
				user.id = Id(id.0.saturating_add(100));
				user.name = ["Avery", "Mika", "Rowan"][(id.0 % 3) as usize].into();
				user.username = Some(format!("synthetic.{}", user.name.to_lowercase()).into());
				user.avatar = None;
				Application {
					id,
					guild,
					user_id: user.id,
					user: Some(user),
					status,
					created_at: 1_789_243_200_000_000_000,
					reviewed_at: (status != Status::Pending).then_some(1_789_246_800_000_000_000),
					moderator: (status != Status::Pending).then(|| owner.clone()),
					reason: (status == Status::Rejected)
						.then(|| "Please complete the community questions.".into()),
					answers: vec![
						Answer {
							label: "What brings you here?".into(),
							answer: "Sharing projects and meeting the community.".into(),
						},
						Answer {
							label: "I agree to the server rules".into(),
							answer: "Yes".into(),
						},
					],
					interview: None,
				}
			})
	};
	let settings = || {
		state
			.server_admin
			.access
			.settings
			.as_ref()
			.filter(|v| v.guild == guild)
			.cloned()
			.unwrap_or_else(|| {
				let channel = state
					.channels
					.iter()
					.find(|c| c.guild == Some(guild) && matches!(c.kind, 0 | 5))
					.map(|c| c.id);
				Settings {
					guild,
					owner: state
						.permissions
						.guilds
						.get(&guild)
						.and_then(|v| v.owner)
						.unwrap_or(owner.id),
					features: vec!["COMMUNITY".into()],
					configuration: Configuration {
						community: true,
						rules: channel,
						updates: channel,
						alerts: channel,
					},
					mfa_level: 1,
					verification_level: 1,
					content_filter: 2,
					incident: Some(Incident {
						invites_until: Some(now + 3_600_000_000_000),
						dms_until: None,
					}),
				}
			})
	};
	match action {
		Action::Applications { status, before } => Outcome::Applications(Applications {
			guild,
			status,
			items: if before.is_none() {
				let offset = match status {
					Status::Pending => 0,
					Status::Approved => 10,
					Status::Rejected => 20,
				};
				(0..3)
					.map(|n| {
						application(
							Id(((1_789_243_200_000u64 - 1_420_070_400_000) << 22) | (n + offset)),
							status,
						)
					})
					.collect()
			} else {
				vec![]
			},
			total: Some(3),
			next: None,
		}),
		Action::Application(id) => Outcome::Application(application(id, Status::Pending)),
		Action::Decide {
			request,
			status,
			reason,
		} => {
			let mut value = application(request, Status::Pending);
			value.status = status;
			value.reviewed_at = Some(now);
			value.moderator = Some(owner.clone());
			value.reason = (!reason.is_empty()).then_some(reason);
			Outcome::Application(value)
		}
		Action::Interview { request } => {
			let mut value = application(request, Status::Pending);
			value.interview = Some(request);
			let recipients = value.user.clone().into_iter().collect();
			Outcome::Interview {
				application: value,
				channel: Box::new(model::Channel {
					icon: None,
					last_message: None,
					id: request,
					guild: None,
					parent_id: None,
					position: 0,
					name: "Application interview (synthetic)".into(),
					kind: 3,
					recipients,
					member_list_id: None,
					message_count: None,
					tags: None,
				}),
			}
		}
		Action::Recipient(id) => {
			let mut user = owner.clone();
			user.id = id;
			user.name = "Morgan (synthetic)".into();
			user.username = Some("synthetic.morgan".into());
			Outcome::Recipient(Box::new(model::server_admin::Member {
				user,
				nick: Some("Morgan".into()),
				roles: vec![],
				joined_at: Some(1_789_243_200_000),
				join_source: Some(1),
				invite_code: None,
				flags: None,
				unusual_dm_until: None,
				timeout_until: None,
			}))
		}
		Action::Settings => Outcome::Settings(settings()),
		Action::Incident { after, .. } => Outcome::Settings(Settings {
			incident: Some(after),
			..settings()
		}),
		Action::Configure { after, .. } => {
			let mut value = settings();
			value.configuration = after;
			value.features.retain(|feature| feature != "COMMUNITY");
			if after.community {
				value.features.push("COMMUNITY".into());
			}
			Outcome::Settings(value)
		}
		Action::MfaLevel { after, .. } => Outcome::Settings(Settings {
			mfa_level: after,
			..settings()
		}),
		Action::Transfer { recipient, .. } => Outcome::Settings(Settings {
			owner: recipient,
			..settings()
		}),
	}
}

#[cfg(test)]
mod access_tests {
	use super::*;
	use model::server_access::{Action, Outcome, Status};
	#[test]
	fn application_fixture_switches_status_and_opens_only_an_explicit_interview() {
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let Outcome::Applications(pending) = execute_access(
			&state,
			guild,
			Action::Applications {
				status: Status::Pending,
				before: None,
			},
		) else {
			panic!("application page")
		};
		let request = pending.items[0].id;
		assert!(pending.valid());
		assert!(pending.items.iter().all(|v| v.interview.is_none()));
		state.server_admin.access.applications = Some(pending);
		for status in [Status::Approved, Status::Rejected] {
			let page = execute_access(
				&state,
				guild,
				Action::Applications {
					status,
					before: None,
				},
			);
			assert!(page.valid());
			let Outcome::Applications(page) = page else {
				panic!("application page")
			};
			assert!(page.items.iter().all(|v| v.status == status));
		}
		let result = execute_access(&state, guild, Action::Interview { request });
		assert!(result.valid());
		let Outcome::Interview {
			application,
			channel,
		} = result
		else {
			panic!("interview")
		};
		assert_eq!(channel.id, request);
		assert_eq!(application.interview, Some(request));
		assert_eq!(channel.kind, 3);
	}
	#[test]
	fn security_fixture_preserves_unrelated_configuration_and_features() {
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let Outcome::Settings(mut original) = execute_access(&state, guild, Action::Settings)
		else {
			panic!("settings")
		};
		original.features.push("FUTURE_FEATURE".into());
		state.server_admin.access.settings = Some(original.clone());
		let result = execute_access(
			&state,
			guild,
			Action::MfaLevel {
				before: original.mfa_level,
				after: 0,
			},
		);
		assert!(result.valid());
		let Outcome::Settings(value) = result else {
			panic!("settings")
		};
		assert_eq!(value.configuration, original.configuration);
		assert_eq!(value.incident, original.incident);
		assert_eq!(value.features, original.features);
		assert_eq!(value.mfa_level, 0);
	}
}

fn role_catalog(state: &State, guild: Id) -> model::server_roles::Catalog {
	use model::server_roles::{Catalog, Colors, Role};
	state.server_admin.roles.clone().unwrap_or_else(|| {
		let mut items = vec![Role {
			id: guild,
			name: "@everyone".into(),
			permissions: model::permissions::VIEW_CHANNEL,
			member_count: Some(8),
			..Default::default()
		}];
		for (index, (name, color, count, managed)) in [
			("Founder", 0xa93226, 2, false),
			("Game For Dev Shooting Blaster 2D", 0x34495e, 0, false),
			("Members", 0x1abc9c, 4, false),
			("Marketing", 0xf1c40f, 1, false),
			("Support", 0x99aab5, 0, false),
			("Bots", 0x99aab5, 2, true),
			("Tickety", 0x99aab5, 1, true),
			("carl-bot", 0x99aab5, 1, true),
		]
		.into_iter()
		.enumerate()
		{
			items.push(Role {
				id: Id(9500 + index as u64),
				name: name.into(),
				colors: Colors {
					primary: color,
					..Default::default()
				},
				permissions: model::permissions::VIEW_CHANNEL,
				position: 8 - index as i32,
				member_count: Some(count),
				managed,
				..Default::default()
			});
		}
		items.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
		Catalog {
			guild,
			items,
			features: vec!["ROLE_ICONS".into(), "ENHANCED_ROLE_COLORS".into()],
		}
	})
}

fn execute_roles(
	state: &State,
	guild: Id,
	action: model::server_roles::Action,
) -> model::server_admin::Result {
	use model::server_roles::{Action, Result as Outcome, Role};
	let mut catalog = role_catalog(state, guild);
	let mut selected = None;
	match action {
		Action::Load => {}
		Action::Create(edit) => {
			let id = Id(catalog
				.items
				.iter()
				.map(|role| role.id.0)
				.max()
				.unwrap_or(9500)
				+ 1);
			let mut role = Role {
				id,
				position: 1,
				member_count: Some(0),
				..Default::default()
			};
			edit.apply(&mut role);
			for existing in &mut catalog.items {
				if existing.position >= 1 {
					existing.position += 1;
				}
			}
			catalog.items.push(role);
			selected = Some(id);
		}
		Action::Edit { id, edit, .. } => {
			if let Some(role) = catalog.items.iter_mut().find(|role| role.id == id) {
				edit.apply(role);
				if matches!(edit.icon, model::Patch::Value(_)) {
					role.icon = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
				}
			}
			selected = Some(id);
		}
		Action::Delete(id) => catalog.items.retain(|role| role.id != id),
		Action::Move { id, position } => {
			if let Some(old) = catalog
				.items
				.iter()
				.find(|role| role.id == id)
				.map(|role| role.position)
			{
				for role in &mut catalog.items {
					if role.id == id {
						role.position = position;
					} else if old < position && role.position > old && role.position <= position {
						role.position -= 1;
					} else if old > position && role.position >= position && role.position < old {
						role.position += 1;
					}
				}
			}
		}
		Action::Members { role, query } => {
			let event = execute_admin(
				state,
				guild,
				0,
				model::server_admin::Action::LoadMembers(query),
			);
			let Event::ServerAdmin(client_core::server_admin::Event {
				result: Ok(model::server_admin::Result::Members(mut page)),
				..
			}) = event
			else {
				unreachable!()
			};
			page.roles = catalog
				.items
				.iter()
				.map(|role| model::server_admin::Role {
					role: role.permission_role(),
					managed: role.managed,
				})
				.collect();
			if let Some(role) = role {
				page.items.retain(|member| member.roles.contains(&role));
			}
			page.total = page.items.len() as u64;
			return model::server_admin::Result::Roles(Outcome::Members { role, page });
		}
	}
	catalog
		.items
		.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
	model::server_admin::Result::Roles(Outcome::Catalog { catalog, selected })
}

fn invite_snapshot(state: &State, guild: Id) -> model::server_invites::Snapshot {
	use model::server_invites::{Invite, Snapshot};
	if let Some(snapshot) = &state.server_admin.invites {
		return snapshot.clone();
	}
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos() as i128;
	let channel = state.invite_channel(guild);
	Snapshot {
		guild,
		features: vec!["COMMUNITY".into()],
		items: [
			"Avery", "Mika", "Rowan", "Sam", "Taylor", "Morgan", "Alex", "Jamie",
		]
		.into_iter()
		.enumerate()
		.map(|(index, name)| {
			let mut user = state.user.clone().expect("fixture owner");
			user.id = Id(9800 + index as u64);
			user.name = name.into();
			user.avatar = None;
			Invite {
				code: format!("demo-link-{}", index + 1),
				inviter: Some(user),
				channel,
				channel_name: Some(
					[
						"general",
						"game-night",
						"announcements",
						"a-long-channel-name-for-the-community",
					][index % 4]
						.into(),
				),
				uses: Some(if index == 2 { 6 } else { 0 }),
				max_uses: Some(0),
				max_age: Some(if index == 5 { 0 } else { 30 * 86400 }),
				created_at: Some(now - 3600 * 1_000_000_000),
				expires_at: (index != 5).then_some(
					now + (if index == 6 {
						435
					} else {
						2_505_600 - index as i128 * 85123
					}) * 1_000_000_000,
				),
				temporary: Some(false),
				roles: None,
			}
		})
		.collect(),
	}
}

fn execute_invites(
	state: &State,
	guild: Id,
	action: model::server_invites::Action,
) -> model::server_admin::Result {
	use model::server_invites::Action;
	let mut snapshot = invite_snapshot(state, guild);
	match action {
		Action::Load => {}
		Action::Revoke { code } => snapshot.items.retain(|invite| invite.code != code),
		Action::SetPaused { paused } => {
			snapshot
				.features
				.retain(|feature| feature != "INVITES_DISABLED");
			if paused {
				snapshot.features.push("INVITES_DISABLED".into());
			}
		}
	}
	model::server_admin::Result::Invites(snapshot)
}

/// Same offline invite creation for the desktop demo and framebuffer harness.
pub fn execute_action(
	state: &mut State,
	action: client_core::server_actions::Action,
	request: u64,
) -> Event {
	use client_core::server_actions::Action;
	let result = match action {
		Action::CreateInvite {
			guild,
			channel,
			options,
		} => {
			let code = format!("synthetic-{request}");
			if state.server_admin.guild == Some(guild) {
				let mut snapshot = invite_snapshot(state, guild);
				let now = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_nanos() as i128;
				snapshot.items.push(model::server_invites::Invite {
					code: code.clone(),
					inviter: state.user.clone(),
					channel: Some(channel),
					channel_name: state.channel(channel).map(|channel| channel.name.clone()),
					uses: Some(0),
					max_uses: Some(u64::from(options.max_uses)),
					max_age: Some(u64::from(options.max_age)),
					created_at: Some(now),
					expires_at: (options.max_age != 0)
						.then_some(now + i128::from(options.max_age) * 1_000_000_000),
					temporary: Some(options.temporary),
					roles: None,
				});
				if !snapshot.valid() {
					return Event::ServerAction(client_core::server_actions::Event::Written {
						action,
						request,
						result: Err(client_core::auth::Failure::Protocol),
					});
				}
				state.server_admin.invites = Some(snapshot);
			}
			Some(code)
		}
		Action::Leave(_) | Action::Delete(_) | Action::Notifications { .. } => None,
	};
	Event::ServerAction(client_core::server_actions::Event::Written {
		action,
		request,
		result: Ok(result),
	})
}

pub fn open(state: &mut State, messaging: &mut ui::MessagingUi) {
	let guild = state.guilds[0].id;
	let user = state.user.as_ref().expect("fixture user").id;
	let mut permissions = test_support::permission_snapshot(state);
	let permission = permissions
		.guilds
		.iter_mut()
		.find(|permission| permission.id == guild)
		.unwrap();
	permission.owner = Some(user);
	let scenario = std::env::args().find_map(|arg| {
		arg.strip_prefix("--demo-server-permission=")
			.map(str::to_owned)
	});
	if let Some(scenario) = scenario.as_deref() {
		permission.owner = Some(Id(u64::MAX));
		if let Some(roles) = &mut permission.roles {
			roles[0].bits |= match scenario {
				"admin" => model::permissions::ADMINISTRATOR,
				"manager" => model::permissions::MANAGE_GUILD,
				"webhooks" => model::permissions::MANAGE_WEBHOOKS,
				"audit" => model::permissions::VIEW_AUDIT_LOG,
				_ => 0,
			};
		}
		if scenario == "unknown" {
			permission.roles = None;
		}
	}
	state.apply(Envelope {
		generation: state.generation,
		event: Event::Permissions(client_core::permissions::Event::Snapshot(permissions)),
	});
	if let Some(page) =
		std::env::args().find_map(|arg| arg.strip_prefix("--demo-server-page=").map(str::to_owned))
	{
		if let Some(Command::ServerAdmin {
			guild,
			request,
			action,
		}) = messaging.preview_server_admin(state, guild, &page)
		{
			let event = execute_admin(state, guild, request, *action);
			state.apply(Envelope {
				generation: state.generation,
				event,
			});
		}
		return;
	}
	if let Some(Command::ServerSettings {
		guild,
		request,
		edit,
	}) = state.load_server_settings(guild)
	{
		let event = execute(state, guild, request, edit);
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	}
	if let Some(Command::ServerSettings {
		guild,
		request,
		edit,
	}) = messaging.preview_server_settings(state, guild)
	{
		let event = execute(state, guild, request, edit);
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	}
}

fn execute_command_permissions(
	state: &State,
	guild: Id,
	action: model::command_permissions::Action,
) -> Result<model::command_permissions::Snapshot, client_core::auth::Failure> {
	use client_core::auth::Failure;
	use model::command_permissions as m;
	let application = action.application();
	if state.server_admin.guild != Some(guild)
		|| !state
			.server_admin
			.integrations
			.as_ref()
			.is_some_and(|page| {
				page.guild == guild
					&& page.integrations.as_ref().is_some_and(|items| {
						items.iter().any(|item| {
							item.application
								.as_ref()
								.is_some_and(|app| app.id == application)
						})
					})
			}) {
		return Err(Failure::Forbidden);
	}
	let mut page = state
		.server_admin
		.command_permissions
		.as_ref()
		.filter(|page| page.guild == guild && page.application == application)
		.cloned()
		.unwrap_or_else(|| m::Snapshot {
			guild,
			application,
			commands: ["help", "server-info"]
				.into_iter()
				.enumerate()
				.map(|(index, name)| m::Command {
					id: Id(application.0.saturating_add(100 + index as u64)),
					name: name.into(),
					kind: 1,
				})
				.collect(),
			permissions: vec![m::Permissions {
				id: application,
				overrides: vec![m::Override {
					id: guild,
					kind: 1,
					permission: true,
				}],
			}],
		});
	match action {
		m::Action::Load { .. } => {}
		m::Action::Save { before, after, .. } => {
			let actual = page.get(before.id).ok_or(Failure::Forbidden)?;
			let merged = m::Permissions::merge(&before, &after, &actual).ok_or(
				Failure::ProtocolAt("Command permissions changed; reload and review your changes"),
			)?;
			page.permissions.retain(|rules| rules.id != merged.id);
			page.permissions.push(merged);
		}
	}
	if !page.valid() {
		return Err(Failure::Protocol);
	}
	Ok(page)
}

fn execute_integrations(
	state: &State,
	guild: Id,
	action: model::server_integrations::Action,
) -> model::server_admin::Result {
	use model::server_integrations::{Action, Application, Integration, Snapshot, Source, Webhook};
	let channel = state
		.channels
		.iter()
		.find(|channel| channel.guild == Some(guild) && channel.kind == 0)
		.unwrap()
		.id;
	let mut page = state.server_admin.integrations.clone().unwrap_or(Snapshot {
		guild,
		channel: None,
		integrations: None,
		webhooks: None,
	});
	if page.integrations.is_none() {
		page.integrations = Some(
			[
				("Orbit", "Moderation and community tools."),
				("Tickets", "A helping hand for your support team."),
			]
			.into_iter()
			.enumerate()
			.map(|(index, (name, description))| {
				let id = Id(9800 + index as u64);
				let mut bot = state.user.as_ref().unwrap().clone();
				bot.id = id;
				bot.name = name.into();
				bot.avatar = None;
				bot.kind = model::AccountKind::Bot;
				Integration {
					id,
					name: name.into(),
					kind: "discord".into(),
					enabled: true,
					user: state.user.clone(),
					synced_at: None,
					role_id: None,
					application: Some(Application {
						id,
						name: name.into(),
						icon: None,
						description: description.into(),
						bot: Some(bot),
					}),
				}
			})
			.collect(),
		);
	}
	if page.webhooks.is_none() {
		page.webhooks = Some(
			(0..3)
				.map(|index| Webhook {
					id: Id(9900 + index),
					guild,
					channel: Some(channel),
					kind: if index == 2 { 2 } else { 1 },
					name: Some(
						["Build updates", "Support updates", "Community news"][index as usize]
							.into(),
					),
					avatar: None,
					application_id: (index == 1).then_some(Id(9801)),
					user: state.user.clone(),
					source_guild: (index == 2).then(|| Source {
						id: Id(9990),
						name: Some("NuSerein Community".into()),
					}),
					source_channel: (index == 2).then(|| Source {
						id: Id(9991),
						name: Some("announcements".into()),
					}),
				})
				.collect(),
		);
	}
	match action {
		Action::CopyWebhookUrl {
			webhook, channel, ..
		} => {
			return model::server_admin::Result::WebhookUrl(
				model::server_integrations::WebhookUrl::new(
					guild,
					webhook,
					channel,
					"SYNTHETIC_DEMO_WEBHOOK_TOKEN",
				)
				.expect("valid synthetic URL"),
			);
		}
		Action::Load {
			channel,
			integrations,
			webhooks,
		} => {
			page.channel = channel;
			if !integrations {
				page.integrations = None;
			}
			if !webhooks {
				page.webhooks = None;
			}
		}
		Action::CreateWebhook {
			channel,
			name,
			scope,
		} => {
			page.channel = scope;
			let items = page.webhooks.as_mut().unwrap();
			let id = Id(items.iter().map(|item| item.id.0).max().unwrap_or(9900) + 1);
			items.push(Webhook {
				id,
				guild,
				channel: Some(channel),
				kind: 1,
				name: Some(name),
				avatar: None,
				application_id: None,
				user: state.user.clone(),
				source_guild: None,
				source_channel: None,
			});
			page.integrations = None;
		}
		Action::EditWebhook {
			scope,
			webhook,
			channel,
			name,
		} => {
			page.channel = scope;
			let hook = page
				.webhooks
				.as_mut()
				.unwrap()
				.iter_mut()
				.find(|hook| hook.id == webhook)
				.unwrap();
			hook.channel = Some(channel);
			hook.name = Some(name);
			page.integrations = None;
		}
		Action::SetWebhookAvatar {
			scope,
			webhook,
			avatar,
		} => {
			page.channel = scope;
			let hook = page
				.webhooks
				.as_mut()
				.unwrap()
				.iter_mut()
				.find(|hook| {
					hook.id == webhook
						&& hook.kind == 1 && scope.is_none_or(|channel| hook.channel == Some(channel))
				})
				.expect("eligible synthetic webhook avatar target");
			hook.avatar = match avatar {
				model::Patch::Value(_) => Some("cccccccccccccccccccccccccccccccc".into()),
				model::Patch::Null => None,
				model::Patch::Absent => unreachable!("validated avatar patch"),
			};
			page.integrations = None;
		}
		Action::DeleteWebhook { webhook, scope } => {
			page.channel = scope;
			page.webhooks
				.as_mut()
				.unwrap()
				.retain(|hook| hook.id != webhook);
			page.integrations = None;
		}
		Action::DeleteIntegration { integration } => {
			page.integrations
				.as_mut()
				.unwrap()
				.retain(|item| item.id != integration);
			page.webhooks = None;
		}
	}
	if let Some(channel) = page.channel
		&& let Some(hooks) = &mut page.webhooks
	{
		hooks.retain(|hook| hook.channel == Some(channel));
	}
	model::server_admin::Result::Integrations(page)
}

fn execute_audit_log(
	state: &State,
	guild: Id,
	query: model::server_audit_log::Query,
) -> model::server_admin::Result {
	use model::{
		Patch,
		server_audit_log::{Change, Entry, PAGE_SIZE, Page},
	};
	let mut moderator = state.user.as_ref().unwrap().clone();
	moderator.id = Id(9801);
	moderator.name = "Morgan".into();
	moderator.avatar = None;
	let mut owner = state.user.as_ref().unwrap().clone();
	owner.name = "Avery".into();
	let users = vec![owner, moderator];
	let channel = state
		.channels
		.iter()
		.find(|channel| channel.guild == Some(guild) && channel.kind == 0)
		.unwrap()
		.id;
	let entries: Vec<_> = (0..75)
		.map(|index| {
			let action_type = [30, 61, 60, 1, 40, 40, 50, 31, 72][index % 9];
			let (key, name) = match action_type {
				30 | 31 => ("name", "Community".to_owned()),
				60 | 61 => ("name", "serein".to_owned()),
				1 => ("name", "Synthetic Workspace".to_owned()),
				40 => ("code", format!("demoInvite{index}")),
				50 => ("name", "Build updates".to_owned()),
				_ => ("count", "1".to_owned()),
			};
			let mut changes = vec![Change {
				key: key.into(),
				old: if matches!(action_type, 1 | 31 | 61) {
					Patch::Value("Previous name".into())
				} else {
					Patch::Absent
				},
				new: Patch::Value(name.clone()),
			}];
			if action_type == 40 {
				changes.extend(
					[
						("channel_id", channel.to_string()),
						("max_uses", "0".into()),
						("max_age", "2592000".into()),
						("temporary", "false".into()),
					]
					.into_iter()
					.map(|(key, value)| Change {
						key: key.into(),
						old: Patch::Absent,
						new: Patch::Value(value),
					}),
				);
			}
			Entry {
				// Fixed synthetic September 12, 2026 timestamps; never real account history.
				id: Id(
					((1_789_243_200_000u64 - index as u64 * 900_000 - 1_420_070_400_000) << 22)
						+ index as u64,
				),
				user_id: Some(users[index % 2].id),
				target_id: Some(if action_type == 40 {
					name
				} else {
					(9700 + index).to_string()
				}),
				action_type,
				reason: (index == 0).then(|| "Organize community permissions (synthetic).".into()),
				changes,
				options: vec![],
			}
		})
		.filter(|entry| {
			query.user.is_none_or(|user| entry.user_id == Some(user))
				&& query
					.action
					.is_none_or(|action| entry.action_type == action)
				&& query.before.is_none_or(|before| entry.id < before)
		})
		.take(PAGE_SIZE)
		.collect();
	let page = Page {
		guild,
		has_more: entries.len() == PAGE_SIZE,
		entries,
		users,
	};
	assert!(page.valid_response());
	model::server_admin::Result::AuditLog(page)
}

#[cfg(test)]
mod permissions_tests {
	use super::*;
	use model::{command_permissions as m, server_admin::Action, server_integrations as i};

	fn perform(state: &mut State, guild: Id, action: Action) {
		let Command::ServerAdmin {
			guild,
			request,
			action,
		} = state
			.request_server_admin(guild, action)
			.expect("eligible synthetic administration command")
		else {
			panic!("production administration command");
		};
		let event = execute_admin(state, guild, request, *action);
		assert!(matches!(&event, Event::ServerAdmin(event) if event.result.is_ok()));
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
		assert!(state.server_admin.error.is_none());
		assert!(!state.server_admin.pending);
	}
	fn fixture() -> (State, Id) {
		let mut state = test_support::demo_state();
		open(&mut state, &mut ui::MessagingUi::default());
		let guild = state.guilds[0].id;
		perform(
			&mut state,
			guild,
			Action::Integrations(i::Action::Load {
				channel: None,
				integrations: true,
				webhooks: true,
			}),
		);
		(state, guild)
	}

	#[test]
	fn command_permission_demo_loads_saves_and_preserves_unknown_overrides() {
		let (mut state, guild) = fixture();
		let app = state
			.server_admin
			.integrations
			.as_ref()
			.unwrap()
			.integrations
			.as_ref()
			.unwrap()[0]
			.application
			.as_ref()
			.unwrap()
			.id;
		perform(
			&mut state,
			guild,
			Action::CommandPermissions(m::Action::Load { application: app }),
		);
		let page = state.server_admin.command_permissions.as_mut().unwrap();
		assert_eq!(page.commands.len(), 2);
		let command = page.commands[0].id;
		page.permissions[0].overrides.push(m::Override {
			id: Id(777),
			kind: 77,
			permission: false,
		});
		let before = page.get(app).unwrap();
		let mut after = before.clone();
		after.overrides[0].permission = false;
		perform(
			&mut state,
			guild,
			Action::CommandPermissions(m::Action::Save {
				application: app,
				before,
				after,
			}),
		);
		let page = state.server_admin.command_permissions.as_ref().unwrap();
		let defaults = page.get(app).unwrap();
		assert!(
			defaults
				.overrides
				.iter()
				.any(|row| row.kind == 77 && !row.permission)
		);
		assert!(
			defaults
				.overrides
				.iter()
				.any(|row| row.id == guild && row.kind == 1 && !row.permission)
		);
		let before = page.get(command).unwrap();
		let mut after = before.clone();
		after.overrides.push(m::Override {
			id: guild,
			kind: 1,
			permission: true,
		});
		perform(
			&mut state,
			guild,
			Action::CommandPermissions(m::Action::Save {
				application: app,
				before: before.clone(),
				after: after.clone(),
			}),
		);
		assert!(
			state
				.server_admin
				.command_permissions
				.as_ref()
				.unwrap()
				.get(command)
				.unwrap()
				.equivalent(&after)
		);
		assert!(matches!(
			execute_command_permissions(
				&state,
				guild,
				m::Action::Save {
					application: app,
					before,
					after
				}
			),
			Err(client_core::auth::Failure::ProtocolAt(_))
		));
		assert!(matches!(
			execute_command_permissions(&state, Id(999_999), m::Action::Load { application: app }),
			Err(client_core::auth::Failure::Forbidden)
		));
	}

	#[test]
	fn webhook_avatar_demo_changes_only_the_approved_avatar_and_supports_removal() {
		let (mut state, guild) = fixture();
		let original = state
			.server_admin
			.integrations
			.as_ref()
			.unwrap()
			.webhooks
			.as_ref()
			.unwrap()[0]
			.clone();
		perform(
			&mut state,
			guild,
			Action::Integrations(i::Action::SetWebhookAvatar {
				scope: None,
				webhook: original.id,
				avatar: model::Patch::Value("data:image/png;base64,U1lOVEhFVElD".into()),
			}),
		);
		let updated = &state
			.server_admin
			.integrations
			.as_ref()
			.unwrap()
			.webhooks
			.as_ref()
			.unwrap()[0];
		assert!(updated.avatar.is_some());
		assert_eq!(updated.name, original.name);
		assert_eq!(updated.channel, original.channel);
		assert_eq!(updated.id, original.id);
		perform(
			&mut state,
			guild,
			Action::Integrations(i::Action::SetWebhookAvatar {
				scope: None,
				webhook: original.id,
				avatar: model::Patch::Null,
			}),
		);
		let updated = &state
			.server_admin
			.integrations
			.as_ref()
			.unwrap()
			.webhooks
			.as_ref()
			.unwrap()[0];
		assert!(updated.avatar.is_none());
		assert_eq!(updated.name, original.name);
		assert_eq!(updated.channel, original.channel);
		let followed = state
			.server_admin
			.integrations
			.as_ref()
			.unwrap()
			.webhooks
			.as_ref()
			.unwrap()
			.iter()
			.find(|hook| hook.kind == 2)
			.unwrap()
			.id;
		assert!(
			state
				.request_server_admin(
					guild,
					Action::Integrations(i::Action::SetWebhookAvatar {
						scope: None,
						webhook: followed,
						avatar: model::Patch::Null
					})
				)
				.is_none()
		);
	}
}
