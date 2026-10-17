//! Explicit offline-only server profile preview; never used for live failures.
use client_core::{Event, State, auth::Failure, server_profile};
use model::{
	Id, Patch,
	server_profile::{Edit, Snapshot, Values},
};

pub fn global_profile(
	state: &State,
	user: Id,
	request: u64,
	changes: Option<model::ProfileEdit>,
) -> Event {
	let result = state
		.user
		.as_ref()
		.filter(|own| own.id == user)
		.ok_or(Failure::Protocol)
		.and_then(|own| {
			let mut profile = state
				.own_profile
				.data
				.clone()
				.unwrap_or_else(|| ui::synthetic_own_profile(own));
			if let Some(changes) = changes {
				if !changes.valid() {
					return Err(Failure::Capacity);
				}
				if let Some(name) = changes.global_name {
					profile.user.name = name.clone().unwrap_or_else(|| profile.username.clone());
					profile.global_name = name;
				}
				if let Some(bio) = changes.bio {
					profile.bio = bio;
				}
				if let Some(pronouns) = changes.pronouns {
					profile.pronouns = pronouns;
				}
				if let Some(color) = changes.accent_color {
					profile.accent_color = color;
				}
				if let Some(avatar) = changes.avatar {
					profile.user.avatar = avatar.map(|_| "0123456789abcdef0123456789abcdef".into());
				}
				if let Some(banner) = changes.banner {
					profile.banner = banner.map(|_| "0123456789abcdef0123456789abcdef".into());
				}
				if let Some(colors) = changes.theme_colors {
					profile.theme_colors = colors;
				}
				if let Some(style) = changes.name_style {
					profile
						.user
						.cosmetics
						.get_or_insert_with(Default::default)
						.name_style = style.map(model::ProfileNameStyle::into_display);
				}
			}
			Ok(Box::new(profile))
		});
	Event::ProfileEdited {
		user,
		request,
		result,
	}
}

pub fn execute(state: &State, user: Id, guild: Id, request: u64, edit: Option<Box<Edit>>) -> Event {
	let result = state
		.user
		.as_ref()
		.filter(|own| own.id == user)
		.ok_or(Failure::Protocol)
		.and_then(|own| {
			let mut snapshot = state
				.own_server_profile
				.data
				.as_ref()
				.filter(|snapshot| snapshot.guild() == Some(guild))
				.cloned()
				.unwrap_or_else(|| {
					let mut profile = ui::synthetic_own_profile(own);
					profile.guild = Some(model::GuildProfile {
						guild,
						roles: vec![],
						nick: None,
						avatar: None,
						banner: None,
						bio: String::new(),
						pronouns: String::new(),
						joined_at: None,
					});
					Snapshot {
						profile,
						values: Values::default(),
					}
				});
			if let Some(edit) = edit {
				if !edit.valid() {
					return Err(Failure::Capacity);
				}
				fn value<T>(patch: Patch<T>, current: &mut Option<T>) {
					match patch {
						Patch::Absent => {}
						Patch::Null => *current = None,
						Patch::Value(value) => *current = Some(value),
					}
				}
				fn image(patch: Patch<String>, current: &mut Option<String>) {
					match patch {
						Patch::Absent => {}
						Patch::Null => *current = None,
						Patch::Value(_) => {
							*current = Some("0123456789abcdef0123456789abcdef".into())
						}
					}
				}
				value(edit.nick, &mut snapshot.values.nick);
				value(edit.bio, &mut snapshot.values.bio);
				value(edit.pronouns, &mut snapshot.values.pronouns);
				value(edit.accent_color, &mut snapshot.values.accent_color);
				value(edit.theme_colors, &mut snapshot.values.theme_colors);
				value(edit.name_style, &mut snapshot.values.name_style);
				image(edit.avatar, &mut snapshot.values.avatar);
				image(edit.banner, &mut snapshot.values.banner);
				let member = snapshot.profile.guild.as_mut().ok_or(Failure::Protocol)?;
				member.nick.clone_from(&snapshot.values.nick);
				member.avatar.clone_from(&snapshot.values.avatar);
				member.banner.clone_from(&snapshot.values.banner);
				member.bio = snapshot.values.bio.clone().unwrap_or_default();
				member.pronouns = snapshot.values.pronouns.clone().unwrap_or_default();
			}
			if !snapshot.valid() {
				return Err(Failure::Capacity);
			}
			Ok(Box::new(snapshot))
		});
	Event::ServerProfile(server_profile::Event {
		user,
		guild,
		request,
		result,
	})
}

pub fn owned_cosmetics(
	state: &State,
	user: Id,
	guild: Option<Id>,
	request: u64,
	change: Option<model::profile_cosmetics::Change>,
) -> Event {
	use model::profile_cosmetics::{Current, Inventory, Item, Kind, Snapshot};
	let mut snapshot = state
		.owned_cosmetics
		.data
		.as_ref()
		.filter(|data| data.current.user == user && data.current.guild == guild)
		.cloned()
		.unwrap_or_else(|| Snapshot {
			inventory: Inventory {
				items: [Kind::Decoration, Kind::Effect, Kind::Nameplate, Kind::Frame]
					.into_iter()
					.enumerate()
					.map(|(index, kind)| Item {
						sku: Id(900 + index as u64),
						kind,
						name: format!("Synthetic {}", kind.label().to_lowercase()),
						expires_at: None,
						subscription: false,
					})
					.collect(),
				unsupported: false,
			},
			current: Current {
				user,
				guild,
				premium_type: Some(state.premium_type),
				decoration: None,
				nameplate: None,
				profile: Vec::new(),
			},
		});
	if let Some(change) = change {
		change.apply(&mut snapshot.current);
	}
	Event::OwnedCosmetics(client_core::owned_cosmetics::Event {
		user,
		guild,
		request,
		result: Ok(Box::new(snapshot)),
	})
}
