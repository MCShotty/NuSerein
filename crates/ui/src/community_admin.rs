//! Native community setup. Drafts are session-only and remain available after rejected writes.
use crate::{avatars::Avatars, design, dialog, i18n};
use client_core::{Command, State};
use model::{
	Id,
	community_admin::{self as m, Action, Data, Section},
	server_admin,
};

#[derive(Default)]
pub(super) struct CommunityUi {
	preview: bool,
	scope: Option<(u64, Id)>,
	section: Option<Section>,
	baseline: Option<Data>,
	draft: Option<Data>,
	attempted: bool,
	submitted: bool,
	revision: u64,
	sequence: u64,
	selected: Option<Id>,
	search: String,
	option_filters: std::collections::BTreeMap<Id, String>,
	author_search: String,
	pending: m::PendingApplications,
	error: Option<&'static str>,
	artwork_requested: Option<(Option<Id>, bool)>,
	artwork_target: Option<(Option<Id>, bool, u64)>,
	artwork_pending: bool,
	artwork_uri: Option<String>,
	artwork_preview: Option<egui::TextureHandle>,
	artwork_remove: Option<(Option<Id>, bool)>,
}
impl CommunityUi {
	pub fn has_changes(&self) -> bool {
		self.submitted
			|| self.draft != self.baseline
			|| self.artwork_pending
			|| self.artwork_uri.is_some()
	}
	pub fn take_artwork_request(&mut self) -> Option<(Option<Id>, bool, u64)> {
		let (channel, resource) = self.artwork_requested.take()?;
		self.sequence = self.sequence.wrapping_add(1);
		let target = (channel, resource, self.sequence);
		self.artwork_target = Some(target);
		Some(target)
	}
	pub fn artwork_active(&self, target: (Option<Id>, bool, u64)) -> bool {
		self.artwork_pending && self.artwork_target == Some(target)
	}
	pub fn accept_artwork(
		&mut self,
		ctx: &egui::Context,
		target: (Option<Id>, bool, u64),
		result: Result<Option<(String, egui::ColorImage)>, &'static str>,
	) {
		if self.artwork_target != Some(target) || !self.artwork_pending {
			return;
		}
		self.artwork_pending = false;
		match result {
			Ok(Some((uri, preview)))
				if uri.len() <= model::server_admin::MAX_IMAGE_URI
					&& preview.size[0] <= 512
					&& preview.size[1] <= 512 =>
			{
				self.artwork_uri = Some(uri);
				self.artwork_preview = Some(ctx.load_texture(
					"community-artwork",
					preview,
					egui::TextureOptions::LINEAR,
				));
			}
			Ok(None) => self.artwork_target = None,
			Ok(Some(_)) => self.error = Some("The prepared image exceeds the artwork limits."),
			Err(error) => self.error = Some(error),
		}
	}
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		page: (Section, bool),
		avatars: &mut Avatars,
		commands: &mut Vec<Command>,
	) {
		let (section, customize) = page;
		if self.scope != Some((state.generation, guild)) {
			*self = Self {
				scope: Some((state.generation, guild)),
				..Default::default()
			};
		}
		if self.section != Some(section) {
			if self.has_changes() {
				dialog::notice(
					ui,
					dialog::Level::Warning,
					"Save or discard the current community changes before switching pages.",
				);
				return;
			}
			self.section = Some(section);
			self.baseline = None;
			self.draft = None;
			self.attempted = false;
			self.selected = None;
		}
		if !state.can_open_community_settings(guild) {
			*self = Self::default();
			return;
		}
		let current = state
			.server_admin
			.community
			.as_ref()
			.filter(|s| s.guild == guild && s.data.section() == section);
		let snapshot_current = current.is_some();
		if self.submitted && !state.server_admin.pending {
			self.submitted = false;
			if state.server_admin.error.is_none() {
				self.baseline = current.map(|s| s.data.clone());
				self.draft = self.baseline.clone();
				self.error = None;
				self.artwork_target = None;
				self.artwork_uri = None;
				self.artwork_preview = None;
			}
		}
		let confirmed_draft = self
			.baseline
			.as_ref()
			.zip(self.draft.as_ref())
			.zip(current)
			.is_some_and(|((before, after), current)| {
				before != after && Data::confirms(before, after, &current.data)
			});
		if current.is_some()
			&& (self.draft.is_none()
				|| ((!self.has_changes() || confirmed_draft)
					&& self.revision != state.server_admin.revision))
		{
			self.baseline = current.map(|s| s.data.clone());
			self.draft = self.baseline.clone();
			self.attempted = true;
			self.revision = state.server_admin.revision;
		}
		let title = match (section, customize) {
			(Section::Onboarding, true) => "community-channels-roles",
			(Section::Onboarding, false) => "community-onboarding",
			(Section::Screening, _) => "community-screening",
			(Section::Guide, _) => "community-guide",
		};
		design::page_header(
			ui,
			title,
			Some(match section {
				Section::Onboarding => "community-onboarding-description",
				Section::Screening => "community-screening-description",
				Section::Guide => "community-guide-description",
			}),
			|_| {},
		);
		let available = !state.server_admin.pending && (state.demo || state.gateway_connected);
		let mut action = None;
		if !self.attempted && available {
			self.attempted = true;
			action = Some(Action::Load(section));
		}
		if let Some(error) = state.server_admin.error {
			dialog::notice(ui, dialog::Level::Error, error);
		}
		if let Some(error) = self.error {
			dialog::notice(ui, dialog::Level::Error, error);
		}
		if self.draft.is_some() && !snapshot_current {
			dialog::notice(ui, dialog::Level::Warning, "community-refresh-required");
		}
		if state.server_admin.pending {
			ui.horizontal(|ui| {
				ui.spinner();
				ui.label(i18n::translate("community-loading"));
			});
		}
		ui.horizontal_wrapped(|ui| {
			if ui
				.add_enabled(
					available,
					egui::Button::new(i18n::translate("community-reload")),
				)
				.clicked()
			{
				action = Some(Action::Load(section));
			}
			if ui
				.add_enabled(
					self.draft.is_some(),
					egui::Button::new("Preview member experience"),
				)
				.clicked()
			{
				self.preview = true;
			}
			if state.server_admin.needs_refresh {
				ui.weak(i18n::translate("community-refresh-required"));
			}
		});
		if !state.can_edit_community_settings(guild, section) {
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"community-manage-roles-required",
			);
		}
		let Some(mut draft) = self.draft.take() else {
			if let Some(action) = action {
				self.dispatch(state, guild, action, commands);
			}
			return;
		};
		ui.add_enabled_ui(
			available
				&& state.can_edit_community_settings(guild, section)
				&& self.artwork_target.is_none(),
			|ui| match &mut draft {
				Data::Onboarding(config) => self.onboarding(ui, state, guild, config, customize),
				Data::Screening(config) => self.screening(ui, config),
				Data::Guide(config) => self.guide(ui, state, guild, config, avatars, commands),
			},
		);
		self.draft = Some(draft);
		if let Some((channel, resource)) = self.artwork_remove.take()
			&& let Some(Data::Guide(guide)) = &self.baseline
		{
			action = Some(Action::Artwork {
				channel,
				resource,
				before: artwork_hash(guide, channel, resource),
				image: model::Patch::Null,
			});
		}
		if self.draft != self.baseline {
			ui.add_space(16.0);
			let (save, reset) = design::save_bar(
				ui,
				state.server_admin.saving.then_some("community-saving"),
				available
					&& snapshot_current
					&& !state.server_admin.needs_refresh
					&& self.draft.as_ref().is_some_and(Data::editable),
				available,
			);
			if reset {
				self.draft = self.baseline.clone();
				self.error = None;
			}
			if save && let (Some(before), Some(after)) = (&self.baseline, &self.draft) {
				action = Some(Action::Edit {
					before: Box::new(before.clone()),
					after: Box::new(after.clone()),
					pending: self.pending,
				});
			}
		}
		if let Some((channel, resource, _)) = self.artwork_target {
			if let Some(texture) = &self.artwork_preview {
				ui.image((
					texture.id(),
					if channel.is_some() {
						egui::vec2(96.0, 96.0)
					} else {
						egui::vec2(240.0, 80.0)
					},
				));
			}
			if self.artwork_pending {
				ui.spinner();
			}
			ui.horizontal_wrapped(|ui| {
				if ui
					.add_enabled(
						available
							&& snapshot_current && self.artwork_uri.is_some()
							&& !state.server_admin.needs_refresh,
						egui::Button::new(i18n::translate("community-save-artwork")),
					)
					.clicked() && let (Some(Data::Guide(guide)), Some(uri)) =
					(&self.baseline, &self.artwork_uri)
				{
					let before = artwork_hash(guide, channel, resource);
					action = Some(Action::Artwork {
						channel,
						resource,
						before,
						image: model::Patch::Value(uri.clone()),
					});
				}
				if ui
					.add_enabled(
						available && !self.artwork_pending,
						egui::Button::new(i18n::translate("community-discard-artwork")),
					)
					.clicked()
				{
					self.artwork_target = None;
					self.artwork_uri = None;
					self.artwork_preview = None;
				}
			});
		}
		if self.preview
			&& let Some(data) = &self.draft
		{
			egui::Window::new("Member experience preview").id(egui::Id::new("community-member-preview")).open(&mut self.preview).default_width(620.0).max_height(600.0).show(ui.ctx(),|ui|{
				ui.weak("Preview of your unsaved configuration. No answers are submitted and no roles or channels are changed.");
				egui::ScrollArea::vertical().show(ui,|ui|member_preview(ui,state,data));
			});
		}
		if let Some(action) = action {
			self.dispatch(state, guild, action, commands);
		}
	}
	fn dispatch(
		&mut self,
		state: &mut State,
		guild: Id,
		action: Action,
		commands: &mut Vec<Command>,
	) {
		let write = action.write();
		if let Some(command) =
			state.request_server_admin(guild, server_admin::Action::Community(action))
		{
			commands.push(command);
			self.submitted = write;
			self.error = None;
		} else {
			self.error = Some(
				"Check channel access, role hierarchy and the setup requirements before saving.",
			);
		}
	}
	fn next_id(&mut self, config: &m::Onboarding) -> Id {
		loop {
			self.sequence = self.sequence.wrapping_add(1).max(1);
			let id = Id(self.sequence);
			if !config
				.prompts
				.iter()
				.any(|p| p.id == id || p.options.iter().any(|o| o.id == id))
			{
				return id;
			}
		}
	}
	fn onboarding(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		guild: Id,
		config: &mut m::Onboarding,
		customize: bool,
	) {
		self.option_filters.retain(|id, _| {
			config
				.prompts
				.iter()
				.flat_map(|p| &p.options)
				.any(|o| o.id == *id)
		});
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_or(1, |d| d.as_millis().min(u64::MAX as u128) as u64);
		self.sequence = self.sequence.max(now);
		if !customize {
			ui.checkbox(
				&mut config.enabled,
				i18n::translate("community-onboarding-enabled"),
			);
			egui::ComboBox::from_id_salt("community-mode")
				.selected_text(if config.mode == 1 {
					i18n::translate("community-mode-advanced")
				} else {
					i18n::translate("community-mode-default")
				})
				.show_ui(ui, |ui| {
					ui.selectable_value(
						&mut config.mode,
						0,
						i18n::translate("community-mode-default"),
					);
					ui.selectable_value(
						&mut config.mode,
						1,
						i18n::translate("community-mode-advanced"),
					);
				});
			dialog::hint(ui, "community-onboarding-requirements");
			if config.below_requirements {
				dialog::notice(ui, dialog::Level::Warning, "community-below-requirements");
			}
		}
		section(ui, "community-default-channels");
		dialog::hint(ui, "community-default-description");
		channel_picker(
			ui,
			state,
			guild,
			&mut config.default_channels,
			"defaults",
			m::MAX_DEFAULT_CHANNELS,
			&mut self.search,
		);
		ui.add_space(16.0);
		section(
			ui,
			if customize {
				"community-custom-questions"
			} else {
				"community-join-questions"
			},
		);
		let mut remove = None;
		let mut shift = None;
		let mut selected = self.selected;
		for (index, prompt) in config
			.prompts
			.iter_mut()
			.enumerate()
			.filter(|(_, p)| customize != p.in_onboarding)
		{
			ui.push_id(prompt.id, |ui| {
				design::card(ui, |ui| {
					ui.horizontal_wrapped(|ui| {
						if ui
							.selectable_label(
								selected == Some(prompt.id),
								if prompt.title.is_empty() {
									i18n::translate("community-new-question")
								} else {
									prompt.title.clone()
								},
							)
							.clicked()
						{
							selected = Some(prompt.id);
						}
						if ui
							.small_button("↑")
							.on_hover_text(i18n::translate("community-move-up"))
							.clicked() && index > 0
						{
							shift = Some((index, index - 1));
						}
						if ui
							.small_button("↓")
							.on_hover_text(i18n::translate("community-move-down"))
							.clicked()
						{
							shift = Some((index, index + 1));
						}
						if ui
							.small_button(i18n::translate("community-remove"))
							.clicked()
						{
							remove = Some(index);
						}
					});
					if selected == Some(prompt.id) {
						self.prompt(ui, state, guild, prompt);
					}
				});
			});
		}
		self.selected = selected;
		if let Some((a, b)) = shift
			&& b < config.prompts.len()
		{
			config.prompts.swap(a, b);
		}
		if let Some(index) = remove {
			config.prompts.remove(index);
		}
		if ui
			.add_enabled(
				config.prompts.len() < m::MAX_PROMPTS,
				egui::Button::new(i18n::translate("community-add-question")),
			)
			.clicked()
		{
			let id = self.next_id(config);
			let option = self.next_id(config);
			self.selected = Some(id);
			config.prompts.push(m::Prompt {
				id,
				title: String::new(),
				in_onboarding: !customize,
				options: vec![m::OptionRow {
					id: option,
					..Default::default()
				}],
				..Default::default()
			});
		}
		if config.prompts.iter().any(|p| p.kind > 1) {
			dialog::notice(ui, dialog::Level::Warning, "community-unknown-prompt");
		}
	}
	fn prompt(&mut self, ui: &mut egui::Ui, state: &State, guild: Id, prompt: &mut m::Prompt) {
		if prompt.kind > 1 {
			ui.weak(i18n::translate("community-unknown-prompt"));
			return;
		}
		input(
			ui,
			"community-question-title",
			&mut prompt.title,
			100,
			false,
		);
		egui::ComboBox::from_id_salt("prompt-kind")
			.selected_text(if prompt.kind == 1 {
				i18n::translate("community-dropdown")
			} else {
				i18n::translate("community-multiple-choice")
			})
			.show_ui(ui, |ui| {
				ui.selectable_value(
					&mut prompt.kind,
					0,
					i18n::translate("community-multiple-choice"),
				);
				ui.selectable_value(&mut prompt.kind, 1, i18n::translate("community-dropdown"));
			});
		ui.checkbox(
			&mut prompt.single_select,
			i18n::translate("community-single-select"),
		);
		ui.checkbox(&mut prompt.required, i18n::translate("community-required"));
		ui.checkbox(
			&mut prompt.in_onboarding,
			i18n::translate("community-show-on-join"),
		);
		let mut remove = None;
		let mut shift = None;
		for (index, option) in prompt.options.iter_mut().enumerate() {
			ui.push_id(option.id, |ui| {
				egui::CollapsingHeader::new(if option.title.is_empty() {
					i18n::translate("community-new-answer")
				} else {
					option.title.clone()
				})
				.default_open(true)
				.show(ui, |ui| {
					input(ui, "community-answer-title", &mut option.title, 50, false);
					optional_input(
						ui,
						"community-answer-description",
						&mut option.description,
						100,
						true,
					);
					emoji_picker(ui, state, guild, &mut option.emoji);
					ui.label(i18n::translate("community-answer-channels"));
					let search = self.option_filters.entry(option.id).or_default();
					channel_picker(
						ui,
						state,
						guild,
						&mut option.channels,
						("option", option.id),
						m::MAX_ASSIGNMENTS,
						search,
					);
					role_picker(ui, state, guild, &mut option.roles);
					ui.horizontal_wrapped(|ui| {
						if ui
							.small_button("↑")
							.on_hover_text(i18n::translate("community-move-up"))
							.clicked() && index > 0
						{
							shift = Some((index, index - 1));
						}
						if ui
							.small_button("↓")
							.on_hover_text(i18n::translate("community-move-down"))
							.clicked()
						{
							shift = Some((index, index + 1));
						}
						if ui
							.small_button(i18n::translate("community-remove-answer"))
							.clicked()
						{
							remove = Some(index);
						}
					});
				});
			});
		}
		if let Some((a, b)) = shift
			&& b < prompt.options.len()
		{
			prompt.options.swap(a, b);
		}
		if let Some(index) = remove {
			prompt.options.remove(index);
		}
		if ui
			.add_enabled(
				prompt.options.len() < m::MAX_OPTIONS,
				egui::Button::new(i18n::translate("community-add-answer")),
			)
			.clicked()
		{
			self.sequence = self.sequence.wrapping_add(1).max(1);
			while prompt.options.iter().any(|o| o.id == Id(self.sequence)) {
				self.sequence = self.sequence.wrapping_add(1).max(1);
			}
			prompt.options.push(m::OptionRow {
				id: Id(self.sequence),
				..Default::default()
			});
		}
	}
	fn screening(&mut self, ui: &mut egui::Ui, config: &mut m::Screening) {
		let was_enabled = config.enabled;
		ui.checkbox(
			&mut config.enabled,
			i18n::translate("community-screening-enabled"),
		);
		if !config.enabled
			&& (was_enabled
				|| self
					.baseline
					.as_ref()
					.is_some_and(|d| matches!(d,Data::Screening(s) if s.enabled)))
		{
			dialog::notice(
				ui,
				dialog::Level::Warning,
				"community-disable-screening-warning",
			);
			ui.radio_value(
				&mut self.pending,
				m::PendingApplications::Approve,
				i18n::translate("community-approve-pending"),
			);
			ui.radio_value(
				&mut self.pending,
				m::PendingApplications::Reject,
				i18n::translate("community-reject-pending"),
			);
		}
		optional_input(
			ui,
			"community-server-description",
			&mut config.description,
			300,
			true,
		);
		let mut remove = None;
		let mut shift = None;
		for (index, field) in config.fields.iter_mut().enumerate() {
			ui.push_id(("screening-field", index), |ui| {
				design::card(ui, |ui| {
					if field.supported() {
						input(ui, "community-field-label", &mut field.label, 300, false);
						ui.checkbox(&mut field.required, i18n::translate("community-required"));
						optional_input(
							ui,
							"community-field-description",
							&mut field.description,
							300,
							true,
						);
						if field.kind == "TERMS" {
							string_rows(ui, &mut field.values, 16, 300, "community-add-rule");
						} else if field.kind == "MULTIPLE_CHOICE" {
							string_rows(ui, &mut field.choices, 8, 150, "community-add-choice");
						} else {
							optional_input(
								ui,
								"community-field-placeholder",
								&mut field.placeholder,
								300,
								false,
							);
						}
					} else {
						dialog::notice(ui, dialog::Level::Warning, "community-unknown-field");
						ui.label(&field.label);
					}
					ui.horizontal_wrapped(|ui| {
						if ui
							.small_button("↑")
							.on_hover_text(i18n::translate("community-move-up"))
							.clicked() && index > 0
						{
							shift = Some((index, index - 1));
						}
						if ui
							.small_button("↓")
							.on_hover_text(i18n::translate("community-move-down"))
							.clicked()
						{
							shift = Some((index, index + 1));
						}
						if ui
							.small_button(i18n::translate("community-remove"))
							.clicked()
						{
							remove = Some(index);
						}
					});
				});
			});
		}
		if let Some((a, b)) = shift
			&& b < config.fields.len()
		{
			config.fields.swap(a, b);
		}
		if let Some(index) = remove {
			config.fields.remove(index);
		}
		ui.horizontal_wrapped(|ui| {
			for (kind, label) in [
				("TERMS", "community-add-rules"),
				("TEXT_INPUT", "community-add-short-answer"),
				("PARAGRAPH", "community-add-paragraph"),
				("MULTIPLE_CHOICE", "community-add-multiple-choice"),
			] {
				if ui
					.add_enabled(
						config.fields.len() < m::MAX_FIELDS
							&& (kind == "TERMS" || config.manual_approval),
						egui::Button::new(i18n::translate(label)),
					)
					.clicked()
				{
					config.fields.push(m::Field {
						kind: kind.into(),
						required: true,
						values: if kind == "TERMS" {
							vec![String::new()]
						} else {
							vec![]
						},
						choices: if kind == "MULTIPLE_CHOICE" {
							vec![String::new()]
						} else {
							vec![]
						},
						..Default::default()
					});
				}
			}
		});
		if !config.manual_approval {
			dialog::hint(ui, "community-manual-approval-required");
		}
		ui.add_space(12.0);
		section(ui, "community-preview");
		for field in &config.fields {
			ui.strong(&field.label);
			for rule in &field.values {
				ui.label(format!("• {rule}"));
			}
			if field.kind == "TERMS" {
				let mut checked = false;
				ui.add_enabled(
					false,
					egui::Checkbox::new(&mut checked, i18n::translate("community-agree-preview")),
				);
			}
		}
	}
	fn guide(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		guild: Id,
		config: &mut m::Guide,
		avatars: &mut Avatars,
		commands: &mut Vec<Command>,
	) {
		ui.checkbox(
			&mut config.enabled,
			i18n::translate("community-guide-enabled"),
		);
		input(
			ui,
			"community-welcome-message",
			&mut config.message,
			300,
			true,
		);
		dialog::hint(ui, "community-welcome-placeholder");
		section(ui, "community-welcome-authors");
		ui.horizontal_wrapped(|ui| {
			for author in config.authors.clone() {
				ui.push_id(author, |ui| {
					let name = author_name(state, author);
					if ui.button(format!("{name} ×")).clicked() {
						config.authors.retain(|id| *id != author);
					}
				});
			}
		});
		if let Some(own) = &state.user
			&& !config.authors.contains(&own.id)
			&& config.authors.len() < 10
			&& ui.button(i18n::translate("community-add-me")).clicked()
		{
			config.authors.push(own.id);
		}
		ui.horizontal_wrapped(|ui| {
			ui.add(
				egui::TextEdit::singleline(&mut self.author_search)
					.char_limit(100)
					.hint_text(i18n::translate("community-find-author")),
			);
			if ui.button(i18n::translate("community-search")).clicked()
				&& let Some(command) = state.request_server_admin(
					guild,
					server_admin::Action::LoadMembers(model::server_admin::Query {
						search: self.author_search.clone(),
						..Default::default()
					}),
				) {
				commands.push(command);
			}
		});
		if let Some(members) = state.server_admin.members.as_ref() {
			for member in &members.items {
				if state.community_author_allowed(guild, member.user.id)
					&& !config.authors.contains(&member.user.id)
					&& config.authors.len() < 10
					&& ui
						.button(format!(
							"+ {}",
							member.nick.as_deref().unwrap_or(&member.user.name)
						))
						.clicked()
				{
					config.authors.push(member.user.id);
				}
			}
		}
		dialog::hint(ui, "community-guide-requirements");
		guide_channels(ui, state, guild, &mut config.actions, true);
		guide_channels(ui, state, guild, &mut config.resources, false);
		section(ui, "community-guide-artwork");
		dialog::hint(ui, "community-guide-artwork-hint");
		let clean_form = matches!(&self.baseline, Some(Data::Guide(before)) if before == config);
		ui.add_enabled_ui(clean_form && self.artwork_target.is_none(), |ui| {
			if config.header_available {
				ui.strong(i18n::translate("community-welcome-sign"));
				self.artwork_controls(ui, None, false, config.header.is_some());
			}
			for (resource, rows) in [(false, &config.actions), (true, &config.resources)] {
				for row in rows {
					ui.push_id(("artwork", resource, row.channel), |ui| {
						ui.label(&row.title);
						self.artwork_controls(ui, Some(row.channel), resource, row.icon.is_some());
					});
				}
			}
		});
		ui.add_space(16.0);
		section(ui, "community-preview");
		design::card(ui, |ui| {
			if let Some(hash) = &config.header {
				avatars.show_guide_header(ui, guild, hash, state.demo);
			}
			ui.strong(config.message.replace(
				"[@username]",
				state.user.as_ref().map_or("member", |u| &u.name),
			));
			for row in &config.actions {
				ui.horizontal(|ui| {
					guide_icon(ui, avatars, row, false, state.demo);
					ui.label(format!("☐ {}", row.title));
				});
			}
			for row in &config.resources {
				ui.horizontal(|ui| {
					guide_icon(ui, avatars, row, true, state.demo);
					ui.label(&row.title);
				});
			}
		});
	}
	fn artwork_controls(
		&mut self,
		ui: &mut egui::Ui,
		channel: Option<Id>,
		resource: bool,
		present: bool,
	) {
		ui.horizontal_wrapped(|ui| {
			if ui
				.button(i18n::translate("community-choose-artwork"))
				.clicked()
			{
				self.artwork_pending = true;
				self.artwork_requested = Some((channel, resource));
			}
			if ui
				.add_enabled(
					present,
					egui::Button::new(i18n::translate("community-remove-artwork")),
				)
				.clicked()
			{
				self.artwork_remove = Some((channel, resource));
			}
		});
	}
}
pub(super) fn guide_icon(
	ui: &mut egui::Ui,
	avatars: &mut Avatars,
	row: &m::GuideChannel,
	resource: bool,
	demo: bool,
) {
	let key = row
		.icon
		.as_ref()
		.map(|hash| {
			format!(
				"guide-{}-{}-{hash}",
				if resource { "resource" } else { "action" },
				row.channel
			)
		})
		.or_else(|| row.emoji.id.map(|id| format!("emoji-{id}")));
	if key.is_some() {
		avatars.show_icon(ui, key, 32.0, demo, &row.title);
	} else if let Some(name) = &row.emoji.name {
		ui.label(egui::RichText::new(name).size(26.0));
	}
}
fn input(ui: &mut egui::Ui, key: &str, value: &mut String, max: usize, multiline: bool) -> bool {
	let label = dialog::label(ui, key);
	let edit = if multiline {
		egui::TextEdit::multiline(value).desired_rows(2)
	} else {
		egui::TextEdit::singleline(value)
	};
	ui.add(edit.char_limit(max).desired_width(f32::INFINITY))
		.labelled_by(label.id)
		.changed()
}
fn optional_input(
	ui: &mut egui::Ui,
	key: &str,
	value: &mut Option<String>,
	max: usize,
	multiline: bool,
) {
	let mut text = value.clone().unwrap_or_default();
	if input(ui, key, &mut text, max, multiline) {
		*value = (!text.is_empty()).then_some(text);
	}
}
fn string_rows(ui: &mut egui::Ui, rows: &mut Vec<String>, max: usize, chars: usize, add: &str) {
	let mut remove = None;
	let mut shift = None;
	for (index, row) in rows.iter_mut().enumerate() {
		ui.push_id(index, |ui| {
			ui.horizontal(|ui| {
				ui.add(
					egui::TextEdit::multiline(row)
						.desired_rows(2)
						.char_limit(chars)
						.desired_width((ui.available_width() - 94.0).max(90.0)),
				);
				if ui.small_button("↑").clicked() && index > 0 {
					shift = Some((index, index - 1));
				}
				if ui.small_button("↓").clicked() {
					shift = Some((index, index + 1));
				}
				if ui
					.small_button("×")
					.on_hover_text(i18n::translate("community-remove"))
					.clicked()
				{
					remove = Some(index);
				}
			});
		});
	}
	if let Some((a, b)) = shift
		&& b < rows.len()
	{
		rows.swap(a, b);
	}
	if let Some(index) = remove {
		rows.remove(index);
	}
	if ui
		.add_enabled(rows.len() < max, egui::Button::new(i18n::translate(add)))
		.clicked()
	{
		rows.push(String::new());
	}
}
fn channel_picker(
	ui: &mut egui::Ui,
	state: &State,
	guild: Id,
	chosen: &mut Vec<Id>,
	salt: impl std::hash::Hash + std::fmt::Debug,
	max: usize,
	search: &mut String,
) {
	egui::CollapsingHeader::new(format!(
		"{} ({})",
		i18n::translate("community-select-channels"),
		chosen.len()
	))
	.id_salt(salt)
	.show(ui, |ui| {
		ui.add(
			egui::TextEdit::singleline(search)
				.char_limit(100)
				.hint_text(i18n::translate("community-filter-channels")),
		);
		let filter = search.to_lowercase();
		egui::ScrollArea::vertical()
			.id_salt("channel-choices")
			.max_height(220.0)
			.show(ui, |ui| {
				for channel in &state.channels {
					if channel.guild != Some(guild)
						|| !matches!(channel.kind, 0 | 2 | 5 | 13 | 15 | 16)
						|| (!filter.is_empty() && !channel.name.to_lowercase().contains(&filter))
					{
						continue;
					}
					let mut selected = chosen.contains(&channel.id);
					let available = state.community_channel_allowed(guild, channel.id);
					if ui
						.add_enabled(
							(available && chosen.len() < max) || selected,
							egui::Checkbox::new(&mut selected, format!("#{}", channel.name)),
						)
						.changed()
					{
						if selected {
							chosen.push(channel.id);
						} else {
							chosen.retain(|id| *id != channel.id);
						}
					}
				}
				for id in chosen.clone() {
					if state.channel(id).is_none()
						&& ui
							.button(format!(
								"{} {id} ×",
								i18n::translate("community-unavailable-channel")
							))
							.clicked()
					{
						chosen.retain(|v| *v != id);
					}
				}
			});
	});
}
fn role_picker(ui: &mut egui::Ui, state: &State, guild: Id, chosen: &mut Vec<Id>) {
	egui::CollapsingHeader::new(format!(
		"{} ({})",
		i18n::translate("community-answer-roles"),
		chosen.len()
	))
	.show(ui, |ui| {
		if let Some(snapshot) = &state.server_admin.community {
			egui::ScrollArea::vertical()
				.id_salt("role-choices")
				.max_height(180.0)
				.show(ui, |ui| {
					for role in &snapshot.roles {
						if role.id == guild {
							continue;
						}
						let mut selected = chosen.contains(&role.id);
						if ui
							.add_enabled(
								(state.community_role_allowed(guild, role.id)
									&& chosen.len() < m::MAX_ASSIGNMENTS)
									|| selected,
								egui::Checkbox::new(&mut selected, &role.name),
							)
							.changed()
						{
							if selected {
								chosen.push(role.id);
							} else {
								chosen.retain(|id| *id != role.id);
							}
						}
					}
					for id in chosen.clone() {
						if !snapshot.roles.iter().any(|r| r.id == id)
							&& ui
								.button(format!(
									"{} {id} ×",
									i18n::translate("community-unavailable-role")
								))
								.clicked()
						{
							chosen.retain(|v| *v != id);
						}
					}
				});
		}
	});
}
fn emoji_picker(ui: &mut egui::Ui, state: &State, guild: Id, emoji: &mut m::Emoji) {
	ui.horizontal_wrapped(|ui| {
		ui.label(i18n::translate("community-emoji"));
		let mut name = emoji.name.clone().unwrap_or_default();
		if ui
			.add(
				egui::TextEdit::singleline(&mut name)
					.char_limit(128)
					.desired_width(110.0),
			)
			.changed()
		{
			emoji.name = (!name.is_empty()).then_some(name);
			emoji.id = None;
			emoji.animated = false;
		}
		egui::ComboBox::from_id_salt("custom-emoji")
			.selected_text(i18n::translate("community-server-emoji"))
			.show_ui(ui, |ui| {
				if let Some(emojis) = state.guild(guild).and_then(|g| g.emojis.as_ref()) {
					for e in emojis.iter().take(model::MAX_GUILD_EMOJIS) {
						if ui
							.selectable_label(emoji.id == Some(e.id), format!(":{}:", e.name))
							.clicked()
						{
							*emoji = m::Emoji {
								id: Some(e.id),
								name: Some(e.name.clone()),
								animated: e.animated,
							};
						}
					}
				}
			});
		if ui
			.small_button(i18n::translate("community-clear"))
			.clicked()
		{
			*emoji = m::Emoji::default();
		}
	});
}
fn guide_channels(
	ui: &mut egui::Ui,
	state: &State,
	guild: Id,
	rows: &mut Vec<m::GuideChannel>,
	actions: bool,
) {
	section(
		ui,
		if actions {
			"community-new-member-actions"
		} else {
			"community-resource-channels"
		},
	);
	let mut remove = None;
	let mut shift = None;
	for (index, row) in rows.iter_mut().enumerate() {
		ui.push_id((actions, index), |ui| {
			design::card(ui, |ui| {
				egui::ComboBox::from_id_salt("guide-channel")
					.selected_text(state.channel(row.channel).map_or_else(
						|| i18n::translate("community-choose-channel"),
						|c| format!("#{}", c.name),
					))
					.show_ui(ui, |ui| {
						for c in &state.channels {
							if state.community_channel_allowed(guild, c.id)
								&& matches!(c.kind, 0 | 5 | 15 | 16)
							{
								ui.selectable_value(&mut row.channel, c.id, format!("#{}", c.name));
							}
						}
					});
				input(
					ui,
					"community-guide-channel-title",
					&mut row.title,
					60,
					false,
				);
				input(
					ui,
					"community-guide-channel-description",
					&mut row.description,
					200,
					true,
				);
				if actions {
					egui::ComboBox::from_id_salt("guide-action")
						.selected_text(if row.action == 1 {
							i18n::translate("community-action-chat")
						} else {
							i18n::translate("community-action-view")
						})
						.show_ui(ui, |ui| {
							ui.selectable_value(
								&mut row.action,
								0,
								i18n::translate("community-action-view"),
							);
							ui.selectable_value(
								&mut row.action,
								1,
								i18n::translate("community-action-chat"),
							);
						});
				}
				emoji_picker(ui, state, guild, &mut row.emoji);
				ui.horizontal_wrapped(|ui| {
					if ui
						.small_button("↑")
						.on_hover_text(i18n::translate("community-move-up"))
						.clicked() && index > 0
					{
						shift = Some((index, index - 1));
					}
					if ui
						.small_button("↓")
						.on_hover_text(i18n::translate("community-move-down"))
						.clicked()
					{
						shift = Some((index, index + 1));
					}
					if ui
						.small_button(i18n::translate("community-remove"))
						.clicked()
					{
						remove = Some(index);
					}
				});
			});
		});
	}
	if let Some((a, b)) = shift
		&& b < rows.len()
	{
		rows.swap(a, b);
	}
	if let Some(index) = remove {
		rows.remove(index);
	}
	let max = if actions {
		m::MAX_ACTIONS
	} else {
		m::MAX_RESOURCES
	};
	if ui
		.add_enabled(
			rows.len() < max,
			egui::Button::new(i18n::translate(if actions {
				"community-add-action"
			} else {
				"community-add-resource"
			})),
		)
		.clicked()
	{
		rows.push(m::GuideChannel::default());
	}
}
fn member_preview(ui: &mut egui::Ui, state: &State, data: &Data) {
	match data {
		Data::Onboarding(config) => {
			ui.heading("Channels & Roles");
			for prompt in &config.prompts {
				ui.strong(format!(
					"{}{}",
					prompt.title,
					if prompt.required { " *" } else { "" }
				));
				ui.weak(if prompt.single_select {
					"Choose one answer"
				} else {
					"Choose any answers"
				});
				for option in &prompt.options {
					egui::Frame::new()
						.fill(design::palette(ui).raised)
						.inner_margin(10)
						.corner_radius(6)
						.show(ui, |ui| {
							ui.strong(&option.title);
							if let Some(description) = &option.description {
								ui.label(description);
							}
							let channels: Vec<_> = option
								.channels
								.iter()
								.map(|id| {
									state
										.channel(*id)
										.map(|c| format!("#{}", c.name))
										.unwrap_or_else(|| "Unavailable channel".into())
								})
								.collect();
							if !channels.is_empty() {
								ui.weak(channels.join(", "));
							}
							let roles: Vec<_> = option
								.roles
								.iter()
								.map(|id| {
									state
										.server_admin
										.community
										.as_ref()
										.and_then(|s| s.roles.iter().find(|r| r.id == *id))
										.map(|r| r.name.as_str())
										.unwrap_or("Unavailable role")
								})
								.collect();
							if !roles.is_empty() {
								ui.weak(roles.join(", "));
							}
						});
				}
				ui.add_space(12.0);
			}
			ui.separator();
			ui.strong("Default channels");
			for id in &config.default_channels {
				if let Some(channel) = state.channel(*id) {
					ui.label(format!("#{}", channel.name));
				}
			}
		}
		Data::Screening(config) => {
			ui.heading("Before you can participate");
			if let Some(description) = &config.description {
				ui.label(description);
			}
			for field in &config.fields {
				ui.strong(&field.label);
				if let Some(description) = &field.description {
					ui.label(description);
				}
				for rule in &field.values {
					ui.label(format!("• {rule}"));
				}
				if field.kind == "TERMS" {
					let mut checked = false;
					ui.add_enabled(
						false,
						egui::Checkbox::new(&mut checked, "I have read and agree to the rules"),
					);
				} else if field.kind == "MULTIPLE_CHOICE" {
					for choice in &field.choices {
						ui.add_enabled(false, egui::RadioButton::new(false, choice));
					}
				} else {
					let mut placeholder = field
						.placeholder
						.clone()
						.unwrap_or_else(|| "Your answer".into());
					ui.add_enabled(false, egui::TextEdit::singleline(&mut placeholder));
				}
				ui.add_space(12.0);
			}
			if config.manual_approval {
				ui.weak("Submitted applications require moderator review.");
			}
		}
		Data::Guide(config) => {
			ui.heading("Server Guide");
			ui.label(&config.message);
			ui.separator();
			ui.strong("Get started");
			for row in &config.actions {
				ui.strong(&row.title);
				ui.label(&row.description);
				if let Some(channel) = state.channel(row.channel) {
					ui.weak(format!("#{}", channel.name));
				}
			}
			ui.separator();
			ui.strong("Resources");
			for row in &config.resources {
				ui.strong(&row.title);
				ui.label(&row.description);
				if let Some(channel) = state.channel(row.channel) {
					ui.weak(format!("#{}", channel.name));
				}
			}
		}
	}
}

fn author_name(state: &State, id: Id) -> String {
	state
		.user
		.as_ref()
		.filter(|u| u.id == id)
		.map(|u| u.name.clone())
		.or_else(|| {
			state
				.server_admin
				.members
				.as_ref()
				.and_then(|p| p.items.iter().find(|m| m.user.id == id))
				.map(|m| m.user.name.clone())
		})
		.unwrap_or_else(|| id.to_string())
}

fn section(ui: &mut egui::Ui, key: &str) {
	design::section(ui, key, None);
}
fn artwork_hash(guide: &m::Guide, channel: Option<Id>, resource: bool) -> Option<String> {
	if let Some(channel) = channel {
		(if resource {
			&guide.resources
		} else {
			&guide.actions
		})
		.iter()
		.find(|r| r.channel == channel)
		.and_then(|r| r.icon.clone())
	} else {
		guide.header.clone()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn fixture() -> (State, Id) {
		let mut state = test_support::demo_state();
		let guild = state.guilds[0].id;
		state.permissions.guilds.insert(
			guild,
			model::permissions::Guild {
				id: guild,
				owner: state.user.as_ref().map(|u| u.id),
				roles: Some(vec![]),
				member: Some(model::permissions::Member {
					roles: vec![],
					timeout_until: None,
				}),
			},
		);
		state.permissions.clear_cache();
		state.server_admin.guild = Some(guild);
		state.server_admin.community = Some(m::Snapshot {
			guild,
			roles: vec![],
			data: Data::Screening(m::Screening {
				enabled: true,
				description: Some(String::new()),
				fields: vec![m::Field {
					source: Some(0),
					kind: "TERMS".into(),
					label: "Rules".into(),
					description: Some(String::new()),
					placeholder: Some(String::new()),
					required: true,
					values: vec!["Be kind".into()],
					..Default::default()
				}],
				..Default::default()
			}),
		});
		(state, guild)
	}
	fn render(editor: &mut CommunityUi, state: &mut State, guild: Id) {
		let ctx = egui::Context::default();
		let output = ctx.run_ui(egui::RawInput::default(), |ui| {
			editor.show(
				ui,
				state,
				guild,
				(Section::Screening, false),
				&mut Avatars::default(),
				&mut vec![],
			);
		});
		output.drop_without_applying_deltas();
	}
	#[test]
	fn community_render_does_not_turn_empty_optional_fields_into_unsolicited_edits() {
		let (mut state, guild) = fixture();
		let mut editor = CommunityUi::default();
		render(&mut editor, &mut state, guild);
		assert!(!editor.has_changes());
		assert_eq!(editor.draft, editor.baseline);
	}
	#[test]
	fn community_reload_acknowledges_a_confirmed_draft_without_another_write() {
		let (mut state, guild) = fixture();
		let mut editor = CommunityUi::default();
		render(&mut editor, &mut state, guild);
		let Data::Screening(draft) = editor.draft.as_mut().unwrap() else {
			panic!()
		};
		draft.fields[0].values[0] = "Be friendly".into();
		state.server_admin.community.as_mut().unwrap().data = editor.draft.clone().unwrap();
		state.server_admin.revision += 1;
		render(&mut editor, &mut state, guild);
		assert!(!editor.has_changes());
		assert_eq!(editor.baseline, editor.draft);
		assert!(!state.server_admin.pending);
	}
	#[test]
	fn community_rejected_save_keeps_the_rendered_rules_draft() {
		let (mut state, guild) = fixture();
		let mut editor = CommunityUi::default();
		render(&mut editor, &mut state, guild);
		let before = editor.baseline.clone().unwrap();
		let mut after = before.clone();
		let Data::Screening(s) = &mut after else {
			panic!()
		};
		s.fields[0].values[0] = "Be friendly".into();
		editor.draft = Some(after.clone());
		let mut commands = vec![];
		editor.dispatch(
			&mut state,
			guild,
			Action::Edit {
				before: Box::new(before),
				after: Box::new(after.clone()),
				pending: Default::default(),
			},
			&mut commands,
		);
		let Command::ServerAdmin { request, .. } = commands.pop().unwrap() else {
			panic!()
		};
		state.apply(client_core::Envelope {
			generation: state.generation,
			event: client_core::Event::ServerAdmin(client_core::server_admin::Event {
				guild,
				request,
				result: Err(client_core::auth::Failure::Forbidden),
			}),
		});
		render(&mut editor, &mut state, guild);
		assert_eq!(editor.draft, Some(after));
		assert!(editor.has_changes());
	}
}
