//! Read-only role simulation. It never changes State or dispatches a command.
use crate::design;
use client_core::State;
use model::{Id, permissions as p};

#[derive(Default)]
pub(super) struct Preview {
	scope: Option<(u64, Id)>,
	roles: Vec<Id>,
	channel: Option<Id>,
	search: String,
	cache_revision: Option<(u64, u64)>,
	cache_roles: Vec<Id>,
	rows: Vec<ChannelRow>,
	detail: Option<(Id, Option<p::PermissionEvaluation>)>,
	#[cfg(test)]
	rebuilds: usize,
	#[cfg(test)]
	visited_rows: usize,
}
struct ChannelRow {
	id: Id,
	index: usize,
	bits: Option<u128>,
}
impl Preview {
	pub fn active(&self) -> bool {
		self.scope.is_some()
	}
	pub fn open(&mut self, state: &State, guild: Id, role: Id) {
		if state.guild_permission(guild, p::ADMINISTRATOR) {
			*self = Self {
				scope: Some((state.generation, guild)),
				roles: if role == guild { vec![] } else { vec![role] },
				..Default::default()
			};
		}
	}
	fn refresh(&mut self, state: &State, guild: Id, metadata: &p::Guild) {
		let revision = (
			state.channel_list_revision(),
			state.channel_labels_revision(),
		);
		if self.cache_revision == Some(revision) && self.cache_roles == self.roles {
			return;
		}
		if let Some(roles) = metadata.roles.as_deref() {
			self.roles
				.retain(|id| roles.iter().any(|role| role.id == *id));
		}
		self.rows = state
			.channels
			.iter()
			.enumerate()
			.filter(|(_, channel)| channel.guild == Some(guild) && !matches!(channel.kind, 10..=12))
			.map(|(index, channel)| ChannelRow {
				id: channel.id,
				index,
				bits: p::preview_roles(
					metadata,
					&self.roles,
					state
						.permissions
						.channels
						.get(&channel.id)
						.and_then(|row| row.overwrites.as_deref()),
				)
				.map(|result| result.bits),
			})
			.collect();
		self.cache_revision = Some(revision);
		self.cache_roles.clone_from(&self.roles);
		self.detail = None;
		#[cfg(test)]
		{
			self.rebuilds += 1;
		}
		if self
			.channel
			.is_some_and(|id| !self.rows.iter().any(|row| row.id == id))
		{
			self.channel = None;
		}
	}
	pub fn show(&mut self, ctx: &egui::Context, state: &State) {
		let Some((generation, guild)) = self.scope else {
			return;
		};
		if generation != state.generation || !state.guild_permission(guild, p::ADMINISTRATOR) {
			*self = Self::default();
			return;
		}
		let Some(metadata) = state.permissions.guilds.get(&guild) else {
			*self = Self::default();
			return;
		};
		let Some(roles) = metadata.roles.as_deref() else {
			*self = Self::default();
			return;
		};
		self.refresh(state, guild, metadata);
		let mut open = true;
		egui::Window::new("View Server As Role")
			.id(egui::Id::unique("server-role-preview"))
			.open(&mut open)
			.default_width(760.0)
			.max_height(ctx.content_rect().height() * 0.85)
			.show(ctx, |ui| {
				egui::Frame::new().fill(design::palette(ui).raised).inner_margin(12).corner_radius(8).show(ui, |ui| {
					ui.horizontal_wrapped(|ui| {
						ui.strong("Previewing selected roles · Read only");
						if ui.button("Disable preview / Back to Roles").clicked() { self.scope = None; }
					});
					ui.weak("Voice suppression, private-thread membership and member-specific restrictions are not simulated.");
				});
				ui.horizontal_wrapped(|ui| {
					ui.menu_button("Select Roles", |ui| {
						ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search roles").char_limit(100));
						let query = self.search.to_lowercase();
						egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
							ui.weak("@everyone is always included");
							for role in roles.iter().filter(|role| role.id != guild && role.name.to_lowercase().contains(&query)) {
								let mut selected = self.roles.contains(&role.id);
								if ui.checkbox(&mut selected, &role.name).changed() {
									if selected && self.roles.len() < p::MAX_MEMBER_ROLES { self.roles.push(role.id); }
									else { self.roles.retain(|id| *id != role.id); }
								}
							}
						});
					});
					ui.label(if self.roles.is_empty() { "@everyone".into() } else { roles.iter().filter(|role| self.roles.contains(&role.id)).map(|role| role.name.as_str()).collect::<Vec<_>>().join(", ") });
				});
				ui.separator();
				self.refresh(state, guild, metadata);
				let row_height = ui.text_style_height(&egui::TextStyle::Body).max(28.0);
				#[cfg(test)]
				{ self.visited_rows = 0; }
				egui::ScrollArea::vertical().id_salt("role-preview-channels").max_height(300.0).show_rows(ui, row_height, self.rows.len(), |ui, range| {
					for row in &self.rows[range] {
						#[cfg(test)]
						{ self.visited_rows += 1; }
						let Some(channel) = state.channels.get(row.index).filter(|channel| channel.id == row.id) else { continue; };
						let visible = row.bits.is_some_and(|bits| bits & p::VIEW_CHANNEL != 0);
						ui.horizontal(|ui| {
							ui.set_min_height(row_height);
							ui.label(if visible { "✓" } else if row.bits.is_some() { "—" } else { "?" });
							if ui.add(egui::Button::selectable(self.channel == Some(row.id), &channel.name).truncate()).clicked() { self.channel = Some(row.id); }
							ui.weak(if visible { "Visible" } else if row.bits.is_some() { "Hidden by permissions" } else { "Permissions unavailable" });
						});
					}
				});
				if let Some(channel) = self.channel {
					if self.detail.as_ref().is_none_or(|(id, _)| *id != channel) {
						let overwrites = state.permissions.channels.get(&channel).and_then(|row| row.overwrites.as_deref());
						self.detail = Some((channel, p::preview_roles(metadata, &self.roles, overwrites)));
					}
					ui.separator();
					ui.strong(state.channel(channel).map_or("Channel permissions", |channel| channel.name.as_str()));
					if let Some((_, Some(result))) = &self.detail {
						ui.horizontal_wrapped(|ui| {
							for (bit, name) in [(p::VIEW_CHANNEL,"View channel"),(p::READ_MESSAGE_HISTORY,"Read history"),(p::SEND_MESSAGES,"Send messages"),(p::CONNECT,"Connect"),(p::SPEAK,"Speak"),(p::MANAGE_CHANNELS,"Manage channel")] {
								ui.label(format!("{} {name}", if result.bits & bit != 0 { "✓" } else { "—" }));
							}
						});
						explanation(ui, result);
					} else { ui.weak("Permissions unavailable"); }
				}
			});
		if !open || self.scope.is_none() {
			*self = Self::default();
		}
	}
}

pub(super) fn explanation(ui: &mut egui::Ui, result: &p::PermissionEvaluation) {
	ui.collapsing("Why these permissions?", |ui| {
		ui.weak("Explicit permissions are combined in this order. Channel access and action prerequisites still apply.");
		for step in &result.steps {
			let label = match step.source {
				p::PermissionSource::Owner => "Server owner: all permissions",
				p::PermissionSource::Everyone => "Start with @everyone",
				p::PermissionSource::AssignedRoles => "Combine assigned role permissions",
				p::PermissionSource::Administrator => "Administrator bypasses channel restrictions",
				p::PermissionSource::EveryoneOverwrite => "Apply @everyone channel override",
				p::PermissionSource::RoleOverwrites => "Combine role channel overrides; allows win within this stage",
				p::PermissionSource::MemberOverwrite => "Apply the member-specific channel override",
				p::PermissionSource::Timeout => "Timeout restricts interaction",
			};
			ui.label(label);
			for (bit,name) in [(p::VIEW_CHANNEL,"View"),(p::SEND_MESSAGES,"Send"),(p::READ_MESSAGE_HISTORY,"History"),(p::CONNECT,"Connect"),(p::SPEAK,"Speak")] {
				if step.before & bit != step.after & bit { ui.weak(format!("{name}: {}", if step.after & bit != 0 {"allowed"}else{"denied"})); }
			}
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn role_preview_virtualizes_channels_and_reuses_unchanged_permission_results() {
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		let selected = state.selected.unwrap();
		let mut snapshot = test_support::permission_snapshot(&state);
		let metadata = snapshot
			.guilds
			.iter_mut()
			.find(|entry| entry.id == guild)
			.unwrap();
		metadata.owner = state.user.as_ref().map(|user| user.id);
		metadata
			.roles
			.as_mut()
			.unwrap()
			.iter_mut()
			.find(|role| role.id == guild)
			.unwrap()
			.bits = p::VIEW_CHANNEL;
		let mut extra_role = metadata.roles.as_ref().unwrap()[0].clone();
		extra_role.id = Id(899_999);
		extra_role.bits = p::SEND_MESSAGES;
		metadata.roles.as_mut().unwrap().push(extra_role);
		for channel in snapshot
			.channels
			.iter_mut()
			.filter(|channel| channel.guild == guild)
		{
			channel.overwrites = Some(vec![]);
		}
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Permissions(client_core::permissions::Event::Snapshot(
				snapshot,
			)),
		});
		let source = state.channel(selected).unwrap().clone();
		for index in 0..500 {
			let mut channel = source.clone();
			channel.id = Id(900_000 + index);
			channel.name = format!("virtual-channel-{index}");
			state.channels.push(channel);
		}
		state.revision += 1;
		let mut preview = Preview::default();
		preview.open(&state, guild, guild);
		let ctx = egui::Context::default();
		let _ = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1120.0, 760.0),
				)),
				..Default::default()
			},
			|ui| preview.show(ui.ctx(), &state),
		);
		assert_eq!(preview.rebuilds, 1);
		assert!(preview.rows.len() >= 500);
		assert!(
			preview.visited_rows > 0 && preview.visited_rows < 30,
			"only viewport rows are laid out, not all 500 channels"
		);
		let initial = preview
			.rows
			.iter()
			.find(|row| row.id == selected)
			.unwrap()
			.bits;
		assert_eq!(initial, Some(p::VIEW_CHANNEL));
		let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
			preview.show(ui.ctx(), &state)
		});
		assert_eq!(
			preview.rebuilds, 1,
			"a repaint does not reevaluate offscreen channels"
		);
		let mut message = test_support::message(90_001, selected);
		message.author.id = Id(900_001);
		message.mentions.clear();
		message.mention_roles.clear();
		message.mention_everyone = false;
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Message(message),
		});
		let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
			preview.show(ui.ctx(), &state)
		});
		assert_eq!(
			preview.rebuilds, 1,
			"ordinary guild traffic keeps the preview cache"
		);
		let mut role = state
			.permissions
			.guilds
			.get(&guild)
			.unwrap()
			.roles
			.as_ref()
			.unwrap()
			.iter()
			.find(|role| role.id == guild)
			.unwrap()
			.clone();
		role.bits = 0;
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::Permissions(client_core::permissions::Event::Role {
				guild,
				role,
			}),
		});
		let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
			preview.show(ui.ctx(), &state)
		});
		assert_eq!(preview.rebuilds, 2);
		assert_eq!(
			preview
				.rows
				.iter()
				.find(|row| row.id == selected)
				.unwrap()
				.bits,
			Some(0)
		);
		let extra_role = state
			.permissions
			.guilds
			.get(&guild)
			.unwrap()
			.roles
			.as_ref()
			.unwrap()
			.iter()
			.find(|role| role.id != guild)
			.unwrap()
			.id;
		preview.roles.push(extra_role);
		let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
			preview.show(ui.ctx(), &state)
		});
		assert_eq!(
			preview.rebuilds, 3,
			"selected-role changes reevaluate the simulation"
		);
		assert_eq!(state.selected, Some(selected));
	}
	#[test]
	fn role_preview_is_read_only_and_retires_after_permission_or_account_change() {
		let mut state = test_support::demo_state();
		let snapshot = test_support::permission_snapshot(&state);
		state.permissions.replace(snapshot).unwrap();
		let guild = state.guilds[0].id;
		let actor = state.user.as_ref().unwrap().id;
		state.permissions.guilds.get_mut(&guild).unwrap().owner = Some(actor);
		state.permissions.clear_cache();
		let original = state.permissions.guilds.get(&guild).unwrap().clone();
		let selected = state.selected;
		let mut preview = Preview::default();
		preview.open(&state, guild, guild);
		assert!(preview.active());
		let ctx = egui::Context::default();
		let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
			preview.show(ui.ctx(), &state)
		});
		assert_eq!(state.permissions.guilds.get(&guild), Some(&original));
		assert_eq!(state.selected, selected);
		state.generation = state.generation.wrapping_add(1);
		preview.show(&ctx, &state);
		assert!(!preview.active());
		state.permissions.guilds.get_mut(&guild).unwrap().owner = Some(Id(999));
		state.permissions.guilds.get_mut(&guild).unwrap().member = Some(p::Member {
			roles: vec![],
			timeout_until: None,
		});
		for role in state
			.permissions
			.guilds
			.get_mut(&guild)
			.unwrap()
			.roles
			.as_mut()
			.unwrap()
		{
			role.bits = p::VIEW_CHANNEL;
		}
		state.permissions.clear_cache();
		preview.open(&state, guild, guild);
		assert!(!preview.active());
	}
}
