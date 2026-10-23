//! A recoverable, account-scoped channel selector. Browsing never acknowledges messages.
use crate::{MessagingUi, design};
use client_core::{Command, State, channel_actions::Action};
use egui::RichText;
use model::Id;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
	#[default]
	All,
	Shown,
	Hidden,
}
#[derive(Clone, Copy)]
enum Row {
	Category(Id),
	Channel(Id),
}
#[derive(Default)]
pub(super) struct Browser {
	scope: Option<(u64, Id)>,
	open: bool,
	search: String,
	filter: Filter,
	key: Option<(u64, String, Filter, bool)>,
	rows: Vec<Row>,
	attempted: Vec<Id>,
	import: bool,
}
impl Browser {
	pub fn open(&mut self, state: &State, guild: Id) {
		if self.scope != Some((state.generation, guild)) {
			*self = Self::default();
		}
		self.scope = Some((state.generation, guild));
		self.open = true;
	}
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		active: Option<Id>,
		local: &model::ChannelPreferences,
		show_locked: bool,
		commands: &mut Vec<Command>,
	) {
		let Some((generation, guild)) = self.scope else {
			return;
		};
		if generation != state.generation || Some(guild) != active || state.guild(guild).is_none() {
			*self = Self::default();
			return;
		}
		if !self.open {
			return;
		}
		let mut open = self.open;
		let mut action = None;
		let mut navigate = None;
		egui::Window::new("Browse Channels").id(egui::Id::new(("browse-channels",generation,guild))).open(&mut open).default_size(egui::vec2(700.0,560.0)).min_width(280.0).resizable(true).show(ctx, |ui| {
			ui.label("Choose the channels in your sidebar. Hidden channels remain available here.");
			let available=state.personal_channel_controls_available(guild);
			ui.horizontal_wrapped(|ui| {
				let mut all=state.personally_shows_all_channels(guild);
				if ui.add_enabled(available,egui::Checkbox::new(&mut all,"Show All Channels")).changed() { action=state.personal_show_all_change(guild,all); }
				if ui.add_enabled(available,egui::Button::new("Import device favorites…")).clicked() {self.import=true;}
			});
			if self.import {
				ui.group(|ui| {
					ui.label("Add this device’s existing favorites to your Discord account? Existing account favorites are kept.");
					ui.horizontal(|ui| {
						if ui.add_enabled(available,egui::Button::new("Import favorites")).clicked() {action=state.personal_import_favorites(guild,&local.favorites); self.import=false;}
						if ui.button("Cancel").clicked() {self.import=false;}
					});
				});
			}
			ui.horizontal_wrapped(|ui| {
				ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search channels or descriptions").char_limit(100).desired_width(260.0));
				for (filter,label) in [(Filter::All,"All"),(Filter::Shown,"Shown"),(Filter::Hidden,"Hidden")] {ui.selectable_value(&mut self.filter,filter,label);}
			});
			if !available && !state.channel_action_pending() {ui.weak("Channel preferences are unavailable. Reconnect to load your account settings.");}
			let key=(state.channel_list_revision(),self.search.clone(),self.filter,show_locked);
			if self.key.as_ref()!=Some(&key) {
				let query=self.search.trim().to_lowercase();
				let mut channels:Vec<_>=state.channels.iter().filter(|channel| channel.guild==Some(guild) && !matches!(channel.kind,4|10..=12)
					&& (show_locked || state.can_view(channel.id))
					&& (query.is_empty() || channel.name.to_lowercase().contains(&query) || state.channel_description(channel.id).is_some_and(|description|description.to_lowercase().contains(&query)))
					&& match self.filter {Filter::All=>true,Filter::Shown=>state.channel_personally_selected(channel.id),Filter::Hidden=>!state.channel_personally_selected(channel.id)}).take(client_core::MAX_NAV).collect();
				channels.sort_unstable_by_key(|channel| (channel.parent_id.and_then(|id|state.channel(id)).map_or((-1,Id(0)),|category|(category.position,category.id)),channel.position,channel.id));
				self.rows.clear();
				let mut parent=None;
				for channel in channels {
					if channel.parent_id!=parent {parent=channel.parent_id; if let Some(parent)=parent.filter(|id|state.channel(*id).is_some_and(|c|c.kind==4 && c.guild==Some(guild))) {self.rows.push(Row::Category(parent));}}
					self.rows.push(Row::Channel(channel.id));
				}
				self.key=Some(key);
			}
			ui.separator();
			if self.rows.is_empty() {ui.label("No channels match this filter. Choose All or clear your search.");}
			let mut description=None;
			egui::ScrollArea::vertical().id_salt(("browse-channel-rows",guild)).max_height((ui.available_height()-35.0).max(180.0)).show_rows(ui,58.0,self.rows.len(),|ui,range| {
				for index in range {
					match self.rows[index] {
						Row::Category(id)=>if let Some(category)=state.channel(id) {
							ui.horizontal(|ui| {
								ui.strong(&category.name);
								for (label,shown) in [("Show category",true),("Hide category",false)] {if ui.add_enabled(available && state.can_view(id),egui::Button::new(label).small()).clicked() {action=state.personal_visibility_change(id,shown);}}
							});
						},
						Row::Channel(id)=>if let Some(channel)=state.channel(id) {
							let allowed=state.can_view(id);
							let mut shown=state.channel_personally_selected(id);
							ui.horizontal(|ui| {
								if ui.add_enabled(available && allowed,egui::Checkbox::new(&mut shown,"")).on_hover_text("Show this channel in your sidebar").changed() {action=state.personal_visibility_change(id,shown);}
								ui.vertical(|ui| {
									let label=format!("{} {}",if matches!(channel.kind,2|13) {"◖"} else {"#"},channel.name);
									if ui.add_enabled(allowed,egui::Button::new(RichText::new(label).strong()).frame(false)).clicked() {navigate=Some(id);}
									if !allowed {ui.small("Unavailable through your current permissions");}
									else if state.guild_channel_effectively_muted(id)==Some(true) {ui.small(if state.hides_muted_channels(guild)==Some(true) {"Muted · filtered by Hide Muted Channels"} else {"Muted"});}
									else if let Some(value)=state.channel_description(id).filter(|value|!value.is_empty()) {ui.add(egui::Label::new(value).truncate());}
									else if !shown {ui.small("Hidden from your sidebar");}
								});
							});
							if allowed && description.is_none() && state.channel_description(id).is_none() && !self.attempted.contains(&id) && self.attempted.len()<64 {description=Some(id);}
						},
					}
				}
			});
			if state.channel_action_pending() {ui.horizontal(|ui|{ui.spinner();ui.weak(if state.channel_description_pending() {"Loading channel information…"} else {"Updating channel preferences…"});});}
			if let Some((_,message,false))=state.channels.iter().filter(|c|c.guild==Some(guild)).find_map(|c|state.channel_action_status(c.id).map(|(message,success)|(c.id,message,success))) {
				ui.colored_label(design::palette(ui).danger,message);
				if ui.button("Retry loading descriptions").clicked() {self.attempted.clear();}
			}
			if action.is_none() && !state.channel_action_pending() && let Some(id)=description && let Some(command)=state.request_channel_action(id,Action::Description) {if self.attempted.len() >= 64 {self.attempted.remove(0);}
				self.attempted.push(id); commands.push(command);}
		});
		self.open = open;
		if let Some(action) = action
			&& let Some(command) = state.request_personal_channel_action(guild, action)
		{
			commands.push(command);
		}
		if let Some(channel) = navigate {
			self.open = false;
			commands.extend(state.select(channel));
		}
	}
}
impl MessagingUi {
	#[cfg(feature = "demo")]
	pub fn preview_browse_channels(&mut self, state: &State, guild: Id) {
		self.guild = Some(guild);
		self.browse_channels.open(state, guild);
	}
}
