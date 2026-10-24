//! Host manifest, invocation and response validation; SDK wire contracts stay at the crate root.
use super::{
	API_VERSION, AppEventKind, BTreeSet, Capability, Element, Error, ExtensionKind, Invocation,
	MAX_CAPABILITIES, MAX_IO_BYTES, MAX_PANEL_ELEMENTS, MAX_STORAGE_BYTES, Manifest, Output,
	RichPresenceUpdate, Surface, app, valid_https_url, valid_id,
};

impl Manifest {
	pub fn validate(&self) -> Result<(), Error> {
		if self.api_version != API_VERSION {
			return Err(Error::Version);
		}
		if !valid_id(&self.id)
			|| !(valid_https_url(&self.source)
				|| self.kind == ExtensionKind::Theme && self.source.is_empty())
		{
			return Err(Error::Invalid);
		}
		for value in [&self.name, &self.version, &self.author, &self.license] {
			if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
				return Err(Error::Invalid);
			}
		}
		if self.capabilities.len() > MAX_CAPABILITIES || self.actions.len() > 16 {
			return Err(Error::Limit);
		}
		if self.capabilities.contains(&Capability::ApiProxy)
			&& (self.kind != ExtensionKind::Plugin
				|| self
					.capabilities
					.iter()
					.any(|cap| !matches!(cap, Capability::ApiProxy | Capability::Storage))
				|| self
					.actions
					.iter()
					.any(|action| !matches!(action.surface, Surface::Activation | Surface::Panel)))
		{
			return Err(Error::Capability);
		}
		let mut capabilities = BTreeSet::new();
		if self.capabilities.iter().any(|c| !capabilities.insert(*c)) {
			return Err(Error::Invalid);
		}
		if capabilities.contains(&Capability::DataEvents)
			&& !capabilities.contains(&Capability::AppEvents)
		{
			return Err(Error::Capability);
		}
		if capabilities.contains(&Capability::ActionFeedback)
			&& (!capabilities.contains(&Capability::AppEvents)
				|| !self
					.actions
					.iter()
					.any(|action| action.surface == Surface::AppEvent))
		{
			return Err(Error::Capability);
		}
		if capabilities.contains(&Capability::DataQueries)
			&& (!capabilities.contains(&Capability::AppEvents)
				|| !self
					.actions
					.iter()
					.any(|action| action.surface == Surface::AppEvent))
		{
			return Err(Error::Capability);
		}
		for surface in [
			Surface::Activation,
			Surface::MessageEvent,
			Surface::AppEvent,
			Surface::Tick,
		] {
			if self
				.actions
				.iter()
				.filter(|action| action.surface == surface)
				.count() > 1
			{
				return Err(Error::Invalid);
			}
		}
		let mut ids = BTreeSet::new();
		for action in &self.actions {
			if !valid_id(&action.id)
				|| !ids.insert(&action.id)
				|| action.label.is_empty()
				|| action.label.len() > 128
				|| action.label.chars().any(char::is_control)
			{
				return Err(Error::Invalid);
			}
			let required = match action.surface {
				Surface::Message => Some(Capability::SelectedMessage),
				Surface::Composer => Some(Capability::Composer),
				Surface::Panel => None,
				Surface::Activation => None,
				Surface::MessageEvent => Some(Capability::MessageEvents),
				Surface::AppEvent => Some(Capability::AppEvents),
				Surface::Tick => Some(Capability::Appearance),
			};
			if required.is_some_and(|c| !capabilities.contains(&c)) {
				return Err(Error::Capability);
			}
		}
		if self.kind == ExtensionKind::Theme
			&& (!self.actions.is_empty() || !self.capabilities.is_empty())
		{
			return Err(Error::Invalid);
		}
		if self.kind == ExtensionKind::Plugin && self.actions.is_empty() {
			return Err(Error::Invalid);
		}
		Ok(())
	}
}

impl Invocation {
	pub fn validate(&self, manifest: &Manifest) -> Result<(), Error> {
		let action = manifest
			.actions
			.iter()
			.find(|a| a.id == self.action)
			.ok_or(Error::Invalid)?;
		if let Some(snapshot) = &self.app {
			snapshot.validate(manifest)?;
		}
		if action.surface == Surface::AppEvent {
			if !manifest.capabilities.contains(&Capability::AppEvents)
				|| self.selected_message.is_some()
				|| self.composer.is_some()
				|| !self.values.is_empty()
			{
				return Err(Error::Capability);
			}
			self.app_event.ok_or(Error::Invalid)?.validate(manifest)?;
		} else if self.app_event.is_some() {
			return Err(Error::Capability);
		}
		if let Some(result) = &self.action_result {
			if action.surface != Surface::AppEvent
				|| self.app_event != Some(AppEventKind::Context)
				|| !manifest.capabilities.contains(&Capability::ActionFeedback)
			{
				return Err(Error::Capability);
			}
			result.validate()?;
		}
		if let Some(queries) = &self.queries {
			if !manifest.capabilities.contains(&Capability::DataQueries) {
				return Err(Error::Capability);
			}
			queries.validate()?;
		}
		if let Some(settings) = &self.messaging_settings {
			if !manifest
				.capabilities
				.contains(&Capability::MessagingSettings)
			{
				return Err(Error::Capability);
			}
			settings.validate()?;
		}
		if let Some(folders) = &self.guild_folders {
			if !manifest.capabilities.contains(&Capability::GuildFolders) {
				return Err(Error::Capability);
			}
			folders.validate()?;
		}
		if action.surface == Surface::MessageEvent {
			if !manifest.capabilities.contains(&Capability::MessageEvents)
				|| self.selected_message.is_some()
				|| self.composer.is_some()
				|| !self.values.is_empty()
			{
				return Err(Error::Capability);
			}
			self.message_event
				.as_ref()
				.ok_or(Error::Invalid)?
				.validate()?;
		} else if self.message_event.is_some() {
			return Err(Error::Capability);
		}
		for (data, capability) in [
			(&self.selected_message, Capability::SelectedMessage),
			(&self.composer, Capability::Composer),
			(&self.storage, Capability::Storage),
		] {
			if let Some(data) = data {
				if !manifest.capabilities.contains(&capability) {
					return Err(Error::Capability);
				}
				if data.len() > MAX_IO_BYTES {
					return Err(Error::Limit);
				}
			}
		}
		if self.selected_message.is_some() && action.surface != Surface::Message
			|| self.composer.is_some() && action.surface != Surface::Composer
			|| self.tick_ms.is_some() && action.surface != Surface::Tick
			|| self.tick_ms.is_none() && action.surface == Surface::Tick
		{
			return Err(Error::Capability);
		}
		if action.surface == Surface::Tick
			&& (!self.values.is_empty()
				|| self.message_event.is_some()
				|| self.app.is_some()
				|| self.app_event.is_some()
				|| self.action_result.is_some()
				|| self.queries.is_some()
				|| self.messaging_settings.is_some()
				|| self.guild_folders.is_some())
		{
			return Err(Error::Capability);
		}
		if self.values.len() > MAX_PANEL_ELEMENTS
			|| self
				.values
				.iter()
				.any(|(id, value)| !valid_id(id) || value.len() > 4096)
		{
			return Err(Error::Limit);
		}
		Ok(())
	}
}

impl Output {
	pub fn validate(&self, manifest: &Manifest, input: &Invocation) -> Result<(), Error> {
		let surface = manifest
			.actions
			.iter()
			.find(|action| action.id == input.action)
			.ok_or(Error::Invalid)?
			.surface;
		if surface == Surface::Tick
			&& (self.rich_presence.is_some()
				|| self.voice_messages.is_some()
				|| self.image_sharing
				|| self.preserve_deleted_messages
				|| self.replacement.is_some()
				|| !self.panel.is_empty()
				|| self.storage.is_some()
				|| !self.effects.is_empty())
		{
			return Err(Error::Capability);
		}
		if let Some(config) = &self.api_proxy {
			if !manifest.capabilities.contains(&Capability::ApiProxy)
				|| !matches!(surface, Surface::Activation | Surface::Panel)
			{
				return Err(Error::Capability);
			}
			config.validate()?;
		}
		if let Some(update) = &self.rich_presence {
			if !manifest.capabilities.contains(&Capability::RichPresence)
				|| !matches!(surface, Surface::Activation | Surface::Panel)
			{
				return Err(Error::Capability);
			}
			if let RichPresenceUpdate::Set { presence } = update {
				presence.validate()?;
			}
		}
		if let Some(config) = &self.voice_messages {
			if !manifest.capabilities.contains(&Capability::VoiceMessages)
				|| !matches!(surface, Surface::Activation | Surface::Panel)
			{
				return Err(Error::Capability);
			}
			config.validate()?;
		}
		if !self.panel.is_empty() && matches!(surface, Surface::MessageEvent | Surface::AppEvent) {
			return Err(Error::Capability);
		}
		if !self.effects.is_empty() {
			if self.replacement.is_some()
				|| matches!(
					surface,
					Surface::Activation | Surface::MessageEvent | Surface::AppEvent
				) {
				return Err(Error::Capability);
			}
			app::validate_effects(&self.effects, manifest)?;
		}
		if let Some(appearance) = &self.appearance {
			if !manifest.capabilities.contains(&Capability::Appearance) {
				return Err(Error::Capability);
			}
			appearance.validate()?;
		}
		if self.image_sharing
			&& (!manifest.capabilities.contains(&Capability::ImageSharing)
				|| !manifest
					.actions
					.iter()
					.any(|a| a.id == input.action && a.surface == Surface::Activation))
		{
			return Err(Error::Capability);
		}
		if self.preserve_deleted_messages
			&& (!manifest.capabilities.contains(&Capability::DeletedMessages)
				|| !manifest
					.actions
					.iter()
					.any(|a| a.id == input.action && a.surface == Surface::Activation))
		{
			return Err(Error::Capability);
		}
		if self.replacement.is_some()
			&& (input.composer.is_none() || !manifest.capabilities.contains(&Capability::Composer))
		{
			return Err(Error::Capability);
		}
		if self.storage.is_some() && !manifest.capabilities.contains(&Capability::Storage) {
			return Err(Error::Capability);
		}
		if self
			.replacement
			.as_ref()
			.is_some_and(|s| s.len() > MAX_IO_BYTES)
			|| self
				.storage
				.as_ref()
				.is_some_and(|s| s.len() > MAX_STORAGE_BYTES)
		{
			return Err(Error::Limit);
		}
		let mut count = 0;
		let mut ids = BTreeSet::new();
		validate_elements(&self.panel, 0, &mut count, &mut ids, manifest)
	}
}

fn validate_elements(
	elements: &[Element],
	depth: usize,
	count: &mut usize,
	ids: &mut BTreeSet<String>,
	manifest: &Manifest,
) -> Result<(), Error> {
	if depth > 8 || elements.len() > MAX_PANEL_ELEMENTS {
		return Err(Error::Limit);
	}
	for element in elements {
		*count += 1;
		if *count > MAX_PANEL_ELEMENTS {
			return Err(Error::Limit);
		}
		let (id, label) = match element {
			Element::ActivityPreview { presence } => {
				presence.validate()?;
				continue;
			}
			Element::Text { text } => {
				if text.len() > 4096 {
					return Err(Error::Limit);
				}
				continue;
			}
			Element::Heading { text } => {
				if text.is_empty() || text.len() > 128 || text.chars().any(char::is_control) {
					return Err(Error::Invalid);
				}
				continue;
			}
			Element::Separator => continue,
			Element::Row { children } => {
				validate_elements(children, depth + 1, count, ids, manifest)?;
				continue;
			}
			Element::Button { id, label } => {
				if !manifest
					.actions
					.iter()
					.any(|a| a.id == *id && a.surface == Surface::Panel)
				{
					return Err(Error::Invalid);
				}
				(id, label)
			}
			Element::TextInput { id, label, value } => {
				if value.len() > 4096 {
					return Err(Error::Limit);
				}
				(id, label)
			}
			Element::Checkbox { id, label, .. } => (id, label),
			Element::Select {
				id,
				label,
				options,
				value,
			} => {
				if options.len() > 32 {
					return Err(Error::Limit);
				}
				let mut unique = BTreeSet::new();
				if options.is_empty()
					|| !options.contains(value)
					|| options.iter().any(|option| {
						option.is_empty()
							|| option.len() > 128 || option.chars().any(char::is_control)
							|| !unique.insert(option)
					}) {
					return Err(Error::Invalid);
				}
				(id, label)
			}
			Element::Slider {
				id,
				label,
				min,
				max,
				value,
			} => {
				if min >= max || !(*min..=*max).contains(value) {
					return Err(Error::Invalid);
				}
				(id, label)
			}
		};
		if !valid_id(id)
			|| !ids.insert(id.clone())
			|| label.is_empty()
			|| label.len() > 128
			|| label.chars().any(char::is_control)
		{
			return Err(Error::Invalid);
		}
	}
	Ok(())
}
