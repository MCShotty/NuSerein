//! Native inventory selection for an explicit owned equip. No purchases or claims.
use crate::{MessagingUi, design};
use client_core::{Command, State};
use model::{
	Id,
	profile_cosmetics::{Change, Kind},
};

const KINDS: [Kind; 4] = [Kind::Decoration, Kind::Effect, Kind::Nameplate, Kind::Frame];

#[derive(Default)]
pub(super) struct Navigation {
	generation: Option<u64>,
	guild: Option<Id>,
	requested: bool,
	synced: Option<u64>,
	selection: [Option<Id>; KINDS.len()],
}
impl Navigation {
	fn matches(&self, state: &State) -> bool {
		self.generation == Some(state.generation) && self.guild == state.owned_cosmetics.guild
	}
	fn sync_scope(&mut self, state: &mut State) {
		if self.generation != Some(state.generation) {
			*self = Self {
				generation: Some(state.generation),
				..Self::default()
			};
		}
		if self
			.guild
			.is_some_and(|guild| !state.can_edit_server_profile(guild))
		{
			// The core ignores this close while a write is pending. Until it settles,
			// the old profile's inventory must not appear under the new scope.
			state.close_owned_cosmetics();
			self.select_scope(None);
		}
	}
	fn select_scope(&mut self, guild: Option<Id>) {
		self.guild = guild;
		self.requested = false;
		self.synced = None;
		self.selection = [None; KINDS.len()];
	}
}
impl MessagingUi {
	pub(super) fn owned_cosmetic_settings(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		ui.collapsing("Owned profile cosmetics", |ui| {
			let nav = &mut self.settings.owned_cosmetics;
			nav.sync_scope(state);
			design::hint(ui, "Equip items Discord confirms you own. Expired items and subscription items without the required subscription cannot be equipped.");
			let mut guild = nav.guild;
			ui.add_enabled_ui(!state.owned_cosmetics.pending && state.owned_cosmetics.draft.is_none(), |ui| {
				let label = ui.label(design::eyebrow(ui, "Apply to", design::palette(ui).muted));
				egui::ComboBox::from_id_salt("owned-cosmetics-scope")
					.width(ui.available_width().min(340.0))
					.selected_text(guild.and_then(|guild| state.guild(guild)).map_or("User Profile", |guild| guild.name.as_str()))
					.show_ui(ui, |ui| {
						ui.selectable_value(&mut guild, None, "User Profile");
						for server in &state.guilds {
							if state.can_edit_server_profile(server.id) {
								ui.selectable_value(&mut guild, Some(server.id), &server.name);
							}
						}
					})
					.response.labelled_by(label.id);
			});
			if guild != nav.guild {
				state.close_owned_cosmetics();
				nav.select_scope(guild);
			}
			if !nav.requested && !state.owned_cosmetics.pending {
				commands.extend(state.load_owned_cosmetics(nav.guild));
				nav.requested = true;
			}
			if state.owned_cosmetics.pending {
				ui.horizontal(|ui| {
					ui.add(crate::design::Busy::new().size(14.0));
					ui.label(if state.owned_cosmetics.saving { "Applying cosmetic…" } else { "Loading owned cosmetics…" });
				});
			}
			if let Some(error) = state.owned_cosmetics.error {
				design::notice(ui, design::Level::Error, error);
			}
			if !state.owned_cosmetics.pending && design::text_action(ui, "Reload owned cosmetics").clicked() {
				commands.extend(state.load_owned_cosmetics(nav.guild));
			}
			if !nav.matches(state) {
				return;
			}
			if let Some(draft) = state.owned_cosmetics.draft.filter(|_| !state.owned_cosmetics.pending) {
				design::notice(ui, design::Level::Warning, "Your previous equip is kept as an unsaved change. Reload to check what Discord applied before trying again.");
				ui.horizontal_wrapped(|ui| {
					if ui.add_enabled(state.can_equip_owned_cosmetic(draft), egui::Button::new("Apply saved change")).clicked() {
						commands.extend(state.equip_owned_cosmetic(draft));
					}
					if design::text_action(ui, "Discard saved change").clicked() {
						state.discard_owned_cosmetic_draft();
					}
				});
			}
			let Some(snapshot) = &state.owned_cosmetics.data else { return; };
			let equipped = KINDS.map(|kind| snapshot.current.equipped(kind));
			if nav.synced != Some(state.owned_cosmetics.request) && !state.owned_cosmetics.pending && state.owned_cosmetics.error.is_none() {
				nav.selection = equipped;
				nav.synced = Some(state.owned_cosmetics.request);
			}
			if snapshot.inventory.unsupported { design::hint(ui, "Some owned product types are unsupported and are left unchanged."); }
			if snapshot.inventory.items.is_empty() { design::hint(ui, "Discord returned no supported owned cosmetics for this account."); }
			let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_secs());
			let mut apply = None;
			ui.add_enabled_ui(!state.owned_cosmetics.pending && !state.owned_cosmetics.reload_required && state.owned_cosmetics.draft.is_none(), |ui| {
				if nav.selection != equipped && design::text_action(ui, "owned-cosmetics-reset-selection").clicked() {
					nav.selection = equipped;
				}
				for (index, kind) in KINDS.into_iter().enumerate() {
					let equipped = equipped[index];
					ui.label(design::eyebrow(ui, kind.label(), design::palette(ui).muted));
					egui::ComboBox::from_id_salt(("owned-cosmetic-kind", kind.wire()))
						.width(ui.available_width().min(340.0))
						.selected_text(nav.selection[index].map_or_else(|| "None / use account default".into(), |sku| {
							snapshot.inventory.find(kind, sku).map_or_else(|| format!("Equipped item {sku}"), |item| item.name.clone())
						}))
						.show_ui(ui, |ui| {
							ui.selectable_value(&mut nav.selection[index], None, "None / use account default");
							if let Some(sku) = equipped.filter(|sku| snapshot.inventory.find(kind, *sku).is_none()) {
								ui.selectable_value(&mut nav.selection[index], Some(sku), format!("Keep equipped item {sku}"));
							}
							for item in snapshot.inventory.items.iter().filter(|item| item.kind == kind) {
								let allowed = Change { kind, sku: Some(item.sku) }.allowed(snapshot, state.premium_type == 2, now);
								let label = if item.expires_at.is_some_and(|expiry| expiry <= now) {
									format!("{} (expired)", item.name)
								} else if !allowed {
									format!("{} (subscription required)", item.name)
								} else {
									item.name.clone()
								};
								ui.add_enabled_ui(allowed, |ui| { ui.selectable_value(&mut nav.selection[index], Some(item.sku), label); });
							}
						});
					if nav.selection[index] != equipped {
						let change = Change { kind, sku: nav.selection[index] };
						let label = if change.sku.is_some() { "Equip" } else { "Remove / use account default" };
						if ui.add_enabled(state.can_equip_owned_cosmetic(change), egui::Button::new(label)).clicked() {
							apply = Some(change);
						}
					}
					ui.add_space(8.0);
				}
			});
			if let Some(change) = apply { commands.extend(state.equip_owned_cosmetic(change)); }
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn losing_server_profile_access_clears_unsubmitted_choices_and_hides_old_inventory() {
		let mut state = State::default();
		let old = Id(100);
		state.owned_cosmetics.guild = Some(old);
		state.owned_cosmetics.pending = true;
		state.owned_cosmetics.saving = true;
		let mut nav = Navigation {
			generation: Some(state.generation),
			guild: Some(old),
			requested: true,
			synced: Some(42),
			selection: [Some(Id(7)); KINDS.len()],
		};
		nav.sync_scope(&mut state);
		assert_eq!(nav.guild, None);
		assert!(!nav.matches(&state));
		assert!(!nav.requested && nav.synced.is_none());
		assert_eq!(nav.selection, [None; KINDS.len()]);
		assert!(state.owned_cosmetics.saving);
	}
}
