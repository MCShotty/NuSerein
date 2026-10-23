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
		self.roles
			.retain(|id| roles.iter().any(|role| role.id == *id));
		let mut open = true;
		egui::Window::new("View Server As Role")
			.id(egui::Id::new("server-role-preview"))
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
				egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
					for channel in state.channels.iter().filter(|channel| channel.guild == Some(guild) && !matches!(channel.kind, 10..=12)) {
						let overwrites = state.permissions.channels.get(&channel.id).and_then(|row| row.overwrites.as_deref());
						let result = p::preview_roles(metadata, &self.roles, overwrites);
						let visible = result.as_ref().is_some_and(|result| result.bits & p::VIEW_CHANNEL != 0);
						ui.horizontal(|ui| {
							ui.label(if visible { "✓" } else if result.is_some() { "—" } else { "?" });
							if ui.selectable_label(self.channel == Some(channel.id), &channel.name).clicked() { self.channel = Some(channel.id); }
							ui.weak(if visible { "Visible" } else if result.is_some() { "Hidden by permissions" } else { "Permissions unavailable" });
						});
						if self.channel == Some(channel.id) {
							ui.indent(channel.id, |ui| {
								if let Some(result) = result {
									for (bit, name) in [(p::VIEW_CHANNEL,"View channel"),(p::READ_MESSAGE_HISTORY,"Read history"),(p::SEND_MESSAGES,"Send messages"),(p::CONNECT,"Connect"),(p::SPEAK,"Speak"),(p::MANAGE_CHANNELS,"Manage channel")] {
										ui.label(format!("{} {name}", if result.bits & bit != 0 { "✓" } else { "—" }));
									}
									explanation(ui, &result);
								}
							});
						}
					}
				});
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
		let _ = ctx.run(egui::RawInput::default(), |ctx| preview.show(ctx, &state));
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
