//! Synthetic configuration only; no Discord traffic.
use client_core::State;
use model::{Id, community_admin::*};
pub fn execute(state: &State, guild: Id, action: Action) -> Snapshot {
	let data = match action {
		Action::Artwork {
			channel,
			resource,
			image,
			before,
		} => {
			let mut guide = state
				.server_admin
				.community
				.as_ref()
				.and_then(|s| {
					if let Data::Guide(g) = &s.data {
						Some(g.clone())
					} else {
						None
					}
				})
				.unwrap_or_default();
			let hash = if matches!(image, model::Patch::Null) {
				None
			} else {
				Some(
					if before.as_deref() == Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa") {
						"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
					} else {
						"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
					}
					.into(),
				)
			};
			if let Some(channel) = channel {
				let rows = if resource {
					&mut guide.resources
				} else {
					&mut guide.actions
				};
				if let Some(row) = rows.iter_mut().find(|r| r.channel == channel) {
					row.icon = hash;
				}
			} else {
				guide.header = hash;
			}
			Data::Guide(guide)
		}
		Action::Edit { after, .. } => *after,
		Action::Load(section) => state
			.server_admin
			.community
			.as_ref()
			.filter(|s| s.guild == guild && s.data.section() == section)
			.map(|s| s.data.clone())
			.unwrap_or_else(|| match section {
				Section::Onboarding => Data::Onboarding(Onboarding {
					default_channels: state
						.channels
						.iter()
						.filter(|c| c.guild == Some(guild) && c.kind == 0)
						.take(8)
						.map(|c| c.id)
						.collect(),
					prompts: vec![
						Prompt {
							id: Id(88001),
							title: "What would you like to do here?".into(),
							options: vec![OptionRow {
								id: Id(88002),
								title: "Meet the community".into(),
								description: Some(
									"Find conversations and people with shared interests.".into(),
								),
								emoji: Emoji {
									id: None,
									name: Some("👋".into()),
									animated: false,
								},
								channels: state
									.channels
									.iter()
									.filter(|c| c.guild == Some(guild) && c.kind == 0)
									.take(2)
									.map(|c| c.id)
									.collect(),
								roles: vec![],
							}],
							..Default::default()
						},
						Prompt {
							id: Id(88003),
							title: "Choose your interests".into(),
							in_onboarding: false,
							options: vec![OptionRow {
								id: Id(88004),
								title: "Design and development".into(),
								emoji: Emoji {
									id: None,
									name: Some("🎨".into()),
									animated: false,
								},
								..Default::default()
							}],
							..Default::default()
						},
					],
					..Default::default()
				}),
				Section::Screening => Data::Screening(Screening {
					enabled: true,
					description: Some("A friendly place to build and share.".into()),
					fields: vec![Field {
						source: Some(0),
						kind: "TERMS".into(),
						label: "Read and agree to the server rules".into(),
						required: true,
						values: vec![
							"Be kind and respect other members.".into(),
							"Keep conversations in the right channels.".into(),
							"No spam, harassment or unsolicited promotions.".into(),
						],
						..Default::default()
					}],
					..Default::default()
				}),
				Section::Guide => Data::Guide(Guide {
					authors: state.user.as_ref().map(|u| vec![u.id]).unwrap_or_default(),
					message: "Welcome [@username]! We're glad you're here.".into(),
					header_available: true,
					actions: state
						.channels
						.iter()
						.filter(|c| c.guild == Some(guild) && c.kind == 0)
						.take(3)
						.enumerate()
						.map(|(index, c)| GuideChannel {
							channel: c.id,
							title: [
								"Read our community rules",
								"Introduce yourself",
								"Find your first conversation",
							][index]
								.into(),
							description: "Make yourself at home.".into(),
							action: u8::from(index > 0),
							..Default::default()
						})
						.collect(),
					resources: vec![],
					..Default::default()
				}),
			}),
	};
	let roles = state
		.permissions
		.guilds
		.get(&guild)
		.and_then(|g| g.roles.as_ref())
		.map(|roles| {
			roles
				.iter()
				.map(|r| Role {
					id: r.id,
					name: r.name.clone(),
					bits: r.bits,
					position: r.position,
					managed: false,
				})
				.collect()
		})
		.unwrap_or_default();
	Snapshot { guild, data, roles }
}

/// Independent ordinary-member fixture, also usable without server-administration permissions.
pub fn execute_member(
	state: &State,
	guild: Id,
	user: Id,
	request: u64,
	action: model::member_guide::Action,
) -> client_core::Event {
	let data = execute(state, guild, Action::Load(Section::Guide));
	let Data::Guide(mut guide) = data.data else {
		unreachable!()
	};
	guide.enabled = true;
	guide.header_available = true;
	guide.header = Some("dddddddddddddddddddddddddddddddd".into());
	let mut completed = state
		.member_guide
		.snapshot
		.as_ref()
		.filter(|s| s.guild == guild && s.user == user)
		.map(|s| s.completed.clone())
		.unwrap_or_default();
	if let model::member_guide::Action::Complete { channel, .. } = action
		&& !completed.contains(&channel)
	{
		completed.push(channel);
	}
	client_core::Event::MemberGuide(client_core::member_guide::Event {
		guild,
		user,
		request,
		result: Ok(model::member_guide::Snapshot {
			guild,
			user,
			guide,
			completed,
			header_known: true,
		}),
	})
}
