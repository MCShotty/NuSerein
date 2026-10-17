//! Native file selection scoped to an application form, separate from message attachments.
use crate::{Desktop, uploads::Uploads};
use client_core::{Command, State, auth::AuthState, interactions::Data};
use discord_api::upload::Source;
use model::Id;

#[derive(Clone, PartialEq, Eq)]
enum Form {
	Modal(Id),
	Command(Id, Id, Vec<String>),
}
#[derive(Clone, PartialEq, Eq)]
struct Scope {
	generation: u64,
	channel: Id,
	form: Form,
}
impl Scope {
	fn current(state: &State, messaging: &ui::MessagingUi) -> Option<Self> {
		let channel = state.selected?;
		let form = if let Some(modal) = &state.interactions.modal {
			Form::Modal(modal.id)
		} else {
			let (id, version, path) = messaging.command_file_scope(state)?;
			Form::Command(id, version, path)
		};
		Some(Self {
			generation: state.generation,
			channel,
			form,
		})
	}
	fn allowed(&self, state: &State) -> bool {
		self.generation == state.generation
			&& state.selected == Some(self.channel)
			&& state.auth == AuthState::Authenticated
			&& state.gateway_connected
			&& state.can_view(self.channel)
			&& match &self.form {
				Form::Modal(id) => state
					.interactions
					.modal
					.as_ref()
					.is_some_and(|m| m.id == *id),
				Form::Command(id, version, path) => {
					state.application_commands_cover(self.channel)
						&& !state.application_commands.loading
						&& state.application_commands.error.is_none()
						&& state.application_commands.commands.iter().any(|command| {
							command.id == *id
								&& command.version == *version
								&& state.can_use_application_command(self.channel, command)
								&& command.options_at(path).is_ok()
						})
				}
			}
	}
}
struct Retained {
	nonce: String,
	fields: Vec<(String, Vec<Source>)>,
}
#[derive(Default)]
pub(crate) struct Files {
	scope: Option<Scope>,
	fields: Vec<(String, Uploads)>,
	retained: Option<Retained>,
	upload_nonce: Option<String>,
}
impl Desktop {
	pub(crate) fn poll_interaction_files(&mut self, ctx: &eframe::egui::Context) {
		let scope = Scope::current(&self.state, &self.messaging);
		if self.interaction_files.scope != scope {
			self.interaction_files = Files {
				scope,
				..Default::default()
			};
			self.messaging.set_command_files(vec![], false);
		}
		let allowed = self
			.interaction_files
			.scope
			.as_ref()
			.is_some_and(|s| s.allowed(&self.state));
		let command_form = self
			.interaction_files
			.scope
			.as_ref()
			.is_some_and(|s| matches!(s.form, Form::Command(..)));
		let mut offset = 0;
		let mut bytes = 0u64;
		let mut command_files = vec![];
		let mut busy = false;
		let was_uploading = self
			.interaction_files
			.fields
			.iter()
			.any(|(_, uploads)| uploads.transfer_active());
		for (field, uploads) in &mut self.interaction_files.fields {
			uploads.poll(self.state.generation, self.state.selected, allowed, ctx);
			let selected = uploads.files();
			let selected_bytes = selected.iter().map(|(_, size)| size).sum::<u64>();
			if command_form
				&& (selected.len() > 1
					|| bytes.saturating_add(selected_bytes) > discord_api::upload::MAX_TOTAL_BYTES)
			{
				uploads.remove();
				self.state.interactions.error =
					Some("Choose one file per option, totaling at most 500 MB");
			} else {
				bytes += selected_bytes;
			}
			let files = uploads.files();
			let count = files.len();
			if command_form {
				command_files.extend(
					files
						.into_iter()
						.enumerate()
						.map(|(i, (name, _))| (field.clone(), offset + i, name)),
				);
			} else {
				self.messaging.set_interaction_files(
					field,
					files
						.into_iter()
						.enumerate()
						.map(|(i, (name, _))| (offset + i, name))
						.collect(),
				);
			}
			offset += count;
			busy |= uploads.busy();
			if let Some(error) = uploads.take_notice() {
				self.state.interactions.error = Some(error);
			}
		}
		if !allowed {
			self.interaction_files.retained = None;
		}
		if allowed
			&& was_uploading
			&& let Some(nonce) = &self.interaction_files.upload_nonce
		{
			// Renew even when this poll consumes the terminal status after a long UI
			// gap: the HTTP completion can still be queued behind other host events.
			self.state
				.renew_interaction_upload(nonce, std::time::Instant::now());
		}
		if !self
			.interaction_files
			.fields
			.iter()
			.any(|(_, uploads)| uploads.transfer_active())
		{
			self.interaction_files.upload_nonce = None;
		}
		if command_form {
			if let Some(retained) = &self.interaction_files.retained {
				command_files.clear();
				let mut index = 0;
				for (field, files) in &retained.fields {
					for file in files {
						command_files.push((field.clone(), index, file.filename().to_owned()));
						index += 1;
					}
				}
			}
			// A failed explicit submission restores selections; upload revalidates the source
			// descriptor before any retry. No worker is automatically restarted.
			let restore = !busy
				&& self.interaction_files.retained.as_ref().is_some_and(|r| {
					self.state
						.interactions
						.submission_result(&r.nonce)
						.is_some_and(|result| result.is_err())
				});
			if restore && let Some(retained) = self.interaction_files.retained.take() {
				for (field, files) in retained.fields {
					if let Some((_, uploads)) = self
						.interaction_files
						.fields
						.iter_mut()
						.find(|(name, _)| *name == field)
						&& let Some(channel) = self.state.selected
						&& let Err(error) = uploads.select_pasted(
							self.state.generation,
							channel,
							files,
							self.runtime.handle(),
							ctx,
						) {
						self.state.interactions.error = Some(error);
					}
				}
			}
			self.messaging.set_command_files(command_files, busy);
		}
	}
	pub(crate) fn choose_interaction_files(&mut self, ctx: &eframe::egui::Context) {
		if let Some(field) = self.messaging.take_command_file_remove()
			&& !self.state.interactions.busy()
			&& let Some((_, uploads)) = self
				.interaction_files
				.fields
				.iter_mut()
				.find(|(name, _)| *name == field)
		{
			uploads.remove();
		}
		let Some(field) = self.messaging.interaction_file_request.take() else {
			return;
		};
		if self.state.demo || self.fixture_only || self.state.interactions.busy() {
			return;
		}
		let Some(scope) = &self.interaction_files.scope else {
			return;
		};
		if Some(scope) != Scope::current(&self.state, &self.messaging).as_ref()
			|| !scope.allowed(&self.state)
		{
			return;
		}
		let command_form = matches!(scope.form, Form::Command(..));
		let valid_field = match &scope.form {
			Form::Modal(_) => self.state.interactions.modal.as_ref().is_some_and(|modal| {
				modal
					.components
					.iter()
					.any(|c| c.find(&field).is_some_and(|c| c.kind == 19))
			}),
			Form::Command(id, _, path) => self
				.state
				.application_commands
				.commands
				.iter()
				.find(|c| c.id == *id)
				.and_then(|c| c.options_at(path).ok())
				.is_some_and(|options| options.iter().any(|o| o.kind == 11 && o.name == field)),
		};
		if !valid_field {
			return;
		}
		let index = if let Some(index) = self
			.interaction_files
			.fields
			.iter()
			.position(|(name, _)| *name == field)
		{
			index
		} else {
			if self.interaction_files.fields.len() >= if command_form { 10 } else { 5 } {
				return;
			}
			self.interaction_files
				.fields
				.push((field.clone(), Uploads::default()));
			self.interaction_files.fields.len() - 1
		};
		let uploads = &mut self.interaction_files.fields[index].1;
		if uploads.busy() {
			return;
		}
		uploads.remove();
		self.messaging.set_interaction_files(&field, vec![]);
		if let Err(error) = uploads.start_choose(
			scope.generation,
			scope.channel,
			self.runtime.handle(),
			ctx,
			self.window.clone(),
		) {
			self.state.interactions.error = Some(error);
		}
	}
	pub(crate) fn interaction_upload(&mut self, command: Command) {
		let channel = self.state.selected;
		let command_form = matches!(&command, Command::Interaction(request) if matches!(request.data, Data::ApplicationCommand {..}));
		let mut sources = vec![];
		let mut retained = vec![];
		if let Some(channel) = channel {
			for (field, uploads) in &mut self.interaction_files.fields {
				if let Some(mut selected) = uploads.take_source(self.state.generation, channel) {
					if command_form {
						retained.push((field.clone(), selected.clone()));
					}
					sources.append(&mut selected);
				}
			}
		}
		let allowed = !self.fixture_only
			&& !self.state.demo
			&& self
				.interaction_files
				.scope
				.as_ref()
				.is_some_and(|scope| scope.allowed(&self.state));
		if !allowed
			|| sources.is_empty()
			|| sources.len() > 10
			|| sources.iter().map(Source::size).sum::<u64>() > discord_api::upload::MAX_TOTAL_BYTES
			|| self.connection.is_none()
		{
			self.state.command_rejected(command);
			self.state.interactions.error =
				Some("Files were not sent; select up to 10 files totaling at most 500 MB");
			return;
		}
		if command_form && let Command::Interaction(request) = &command {
			self.interaction_files.retained = Some(Retained {
				nonce: request.nonce.clone(),
				fields: retained,
			});
		}
		let (progress, receive) =
			tokio::sync::watch::channel(discord_api::upload::Status::Preparing);
		let (cancel, _) = tokio::sync::watch::channel(false);
		if self
			.interaction_files
			.fields
			.first_mut()
			.is_none_or(|(_, uploads)| uploads.begin_upload(receive, cancel.clone()).is_err())
		{
			self.state.command_rejected(command);
			return;
		}
		if let Command::Interaction(request) = &command {
			self.interaction_files.upload_nonce = Some(request.nonce.clone());
		}
		let request = crate::uploads::UploadRequest {
			command,
			source: sources,
			voice_message: None,
			progress,
			cancel,
		};
		if let Err(error) = self.connection.as_ref().unwrap().uploads.try_send(request) {
			self.state.command_rejected(error.into_inner().command);
		}
	}
}
pub(crate) fn request_has_files(request: &client_core::interactions::Request) -> bool {
	match &request.data {
		Data::ApplicationCommand { invocation } => {
			invocation.attachment_count().is_some_and(|count| count > 0)
		}
		Data::Modal { components, .. } => has_files(components),
		_ => false,
	}
}
pub(crate) fn has_files(components: &[model::Component]) -> bool {
	components.iter().any(|c| {
		c.kind == 19 && !c.values.is_empty()
			|| has_files(&c.components)
			|| c.component
				.as_deref()
				.is_some_and(|c| has_files(std::slice::from_ref(c)))
	})
}
