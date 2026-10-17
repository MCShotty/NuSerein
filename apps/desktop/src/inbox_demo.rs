//! Synthetic remote state for native Inbox previews. Never contacts a service or opens a file.
use client_core::{
	Command, Event, State,
	inbox::{Action, Outcome},
};
use model::{Id, inbox::SavedMessage};
#[derive(Default)]
pub(crate) struct Store {
	generation: Option<u64>,
	saved: Vec<SavedMessage>,
	dismissed: std::collections::BTreeSet<Id>,
}
impl Store {
	pub(crate) fn execute(&mut self, state: &State, command: Command) -> Event {
		if self.generation != Some(state.generation) {
			*self = Self {
				generation: Some(state.generation),
				..Self::default()
			};
		}
		match command {
			Command::Mentions { before, request } => {
				let mut messages: Vec<_> = [
					"A direct mention for you",
					"Your role was mentioned",
					"An @everyone announcement",
				]
				.into_iter()
				.enumerate()
				.map(|(index, content)| {
					let mut message = test_support::message(90000 - index as u64, Id(20));
					message.content = content.into();
					message.attachments.clear();
					message.embeds.clear();
					message
				})
				.filter(|message| {
					before.is_none_or(|before| message.id < before)
						&& !self.dismissed.contains(&message.id)
				})
				.collect();
				messages.shrink_to_fit();
				Event::Mentions {
					request,
					result: Ok(messages),
				}
			}
			Command::InboxAction { request, action } => {
				let result = match action {
					Action::Saved => Outcome::Saved(self.saved.clone()),
					Action::Save {
						channel,
						message,
						due_at,
					} => {
						let body = state
							.timeline
							.iter()
							.chain(&state.inbox.messages)
							.find(|row| row.channel == channel && row.id == message)
							.cloned()
							.or_else(|| {
								self.saved
									.iter()
									.find(|row| row.channel == channel && row.message == message)
									.and_then(|row| row.body.clone())
							});
						let row = SavedMessage {
							channel,
							message,
							due_at,
							guild: state.channel(channel).and_then(|row| row.guild),
							body,
						};
						self.saved
							.retain(|row| !(row.channel == channel && row.message == message));
						if self.saved.len() < model::inbox::MAX_SAVED
							&& self.saved.iter().map(SavedMessage::bytes).sum::<usize>()
								+ row.bytes() + (self.saved.len() + 1) * size_of::<SavedMessage>()
								<= model::inbox::MAX_BYTES
						{
							self.saved.push(row.clone());
							self.saved.shrink_to_fit();
						} else {
							return Event::InboxAction {
								request,
								result: Err(client_core::auth::Failure::Capacity),
							};
						}
						Outcome::SavedOne(Box::new(row))
					}
					Action::Dismiss { message, .. } => {
						if self.dismissed.len() < 512 {
							self.dismissed.insert(message);
						}
						Outcome::Done
					}
					Action::Unsave { channel, message } => {
						self.saved
							.retain(|row| !(row.channel == channel && row.message == message));
						Outcome::Done
					}
				};
				Event::InboxAction {
					request,
					result: Ok(result),
				}
			}
			_ => Event::Failure(client_core::auth::Failure::Protocol),
		}
	}
}
