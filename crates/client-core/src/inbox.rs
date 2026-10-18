//! One bounded, on-demand page of the account's recent mentions.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{Id, Message};

pub const PAGE_SIZE: usize = 25;
pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_WIRE: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
	Saved,
	Dismiss {
		channel: Id,
		message: Id,
	},
	Save {
		channel: Id,
		message: Id,
		due_at: Option<i128>,
	},
	Unsave {
		channel: Id,
		message: Id,
	},
}
impl Action {
	pub fn write(&self) -> bool {
		!matches!(self, Self::Saved)
	}
	pub fn valid(&self) -> bool {
		match self {
			Self::Saved => true,
			Self::Dismiss { channel, message } | Self::Unsave { channel, message } => {
				channel.0 != 0 && message.0 != 0
			}
			Self::Save {
				channel,
				message,
				due_at,
			} => {
				channel.0 != 0
					&& message.0 != 0
					&& due_at.is_none_or(|at| (0..=253_402_300_799_999_999_999).contains(&at))
			}
		}
	}
}
pub enum Outcome {
	Saved(Vec<model::inbox::SavedMessage>),
	SavedOne(Box<model::inbox::SavedMessage>),
	Done,
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Saved(items) => {
				items.capacity() * size_of::<model::inbox::SavedMessage>()
					+ items
						.iter()
						.map(model::inbox::SavedMessage::bytes)
						.sum::<usize>()
			}
			Self::SavedOne(item) => item.bytes(),
			Self::Done => 0,
		}
	}
}

#[derive(Default)]
pub struct View {
	pub messages: Vec<Message>,
	pub loading: bool,
	pub error: Option<&'static str>,
	pub before: Option<Id>,
	pub next: Option<Id>,
	pub loaded: bool,
	pub request: u64,
	pub saved: Vec<model::inbox::SavedMessage>,
	pub saved_loaded: bool,
	pub operation: Option<Action>,
	/// A service echo marks the snapshot stale without retiring an accepted write.
	pub stale: bool,
	/// Closing/navigation retires cached content, but the one pending write can still settle.
	pub detached_write: bool,
}

impl State {
	pub fn can_load_mentions(&self) -> bool {
		self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected)
	}
	pub fn request_mentions(&mut self, before: Option<Id>) -> Option<Command> {
		if !self.can_load_mentions()
			|| self.inbox.loading
			|| before.is_some_and(|before| {
				Some(before) != self.inbox.next
					&& !(self.inbox.error.is_some() && Some(before) == self.inbox.before)
			}) {
			return None;
		}
		self.inbox = View {
			request: self.inbox.request.wrapping_add(1),
			loading: true,
			before,
			..Default::default()
		};
		Some(Command::Mentions {
			before,
			request: self.inbox.request,
		})
	}
	pub fn request_inbox_action(&mut self, action: Action) -> Option<Command> {
		if !self.can_load_mentions() || self.inbox.loading || !self.inbox_action_allowed(&action) {
			return None;
		}
		self.inbox.request = self.inbox.request.wrapping_add(1);
		self.inbox.loading = true;
		self.inbox.error = None;
		self.inbox.detached_write = false;
		if !action.write() {
			self.inbox.stale = false;
		}
		self.inbox.operation = Some(action.clone());
		Some(Command::InboxAction {
			request: self.inbox.request,
			action,
		})
	}
	pub fn inbox_action_allowed(&self, action: &Action) -> bool {
		if !action.valid() || !self.can_load_mentions() {
			return false;
		}
		match action {
			Action::Saved => true,
			Action::Dismiss { channel, message } => {
				self.can_read_history(*channel)
					&& self
						.inbox
						.messages
						.iter()
						.any(|item| item.channel == *channel && item.id == *message)
			}
			Action::Save {
				channel, message, ..
			} => {
				self.can_read_history(*channel)
					&& (self
						.inbox
						.saved
						.iter()
						.any(|item| item.channel == *channel && item.message == *message)
						|| self
							.inbox
							.messages
							.iter()
							.any(|item| item.channel == *channel && item.id == *message)
						|| self.timeline.iter().any(|item| {
							item.channel == *channel && item.id == *message && !item.ephemeral
						}))
			}
			Action::Unsave { channel, message } => self
				.inbox
				.saved
				.iter()
				.any(|item| item.channel == *channel && item.message == *message),
		}
	}
	pub fn apply_inbox_action(&mut self, request: u64, result: Result<Outcome, Failure>) {
		let write = self.inbox.operation.as_ref().is_some_and(Action::write);
		let may_settle =
			self.can_load_mentions() || (write && self.auth == AuthState::Authenticated);
		if !may_settle || !self.inbox.loading || self.inbox.request != request {
			return;
		}
		let Some(action) = self.inbox.operation.take() else {
			return;
		};
		self.inbox.loading = false;
		let detached = std::mem::take(&mut self.inbox.detached_write);
		if let Err(failure) = result {
			self.inbox.error = Some(failure.label());
			self.status = failure.label();
			if failure.ends_session() && failure != Failure::Capacity {
				self.fail(failure);
			}
			return;
		}
		if detached {
			self.status = "Inbox updated";
			return;
		}
		match (action, result.unwrap()) {
			(Action::Saved, Outcome::Saved(mut items))
				if model::inbox::valid_saved(&items)
					&& items.capacity() * size_of::<model::inbox::SavedMessage>()
						+ items
							.iter()
							.map(model::inbox::SavedMessage::bytes)
							.sum::<usize>() <= model::inbox::MAX_BYTES =>
			{
				for item in &mut items {
					if !self.can_read_history(item.channel) {
						item.body = None;
					}
				}
				self.inbox.saved = items;
				self.inbox.saved_loaded = true;
			}
			(
				Action::Save {
					channel,
					message,
					due_at,
				},
				Outcome::SavedOne(mut item),
			) if item.valid()
				&& item.channel == channel
				&& item.message == message
				&& item.due_at == due_at =>
			{
				if !self.can_read_history(channel) {
					item.body = None;
				}
				self.inbox
					.saved
					.retain(|row| !(row.channel == channel && row.message == message));
				if self.inbox.saved.len() < model::inbox::MAX_SAVED
					&& self
						.inbox
						.saved
						.iter()
						.map(model::inbox::SavedMessage::bytes)
						.sum::<usize>() + item.bytes()
						+ (self.inbox.saved.len() + 1) * size_of::<model::inbox::SavedMessage>()
						<= model::inbox::MAX_BYTES
				{
					self.inbox.saved.push(*item);
					self.inbox.saved.shrink_to_fit();
				} else {
					self.inbox.saved.clear();
					self.inbox.saved_loaded = false;
				}
			}
			(Action::Dismiss { channel, message }, Outcome::Done) => self
				.inbox
				.messages
				.retain(|item| !(item.channel == channel && item.id == message)),
			(Action::Unsave { channel, message }, Outcome::Done) => self
				.inbox
				.saved
				.retain(|item| !(item.channel == channel && item.message == message)),
			_ => {
				self.inbox.error = Some("Inbox response was invalid or did not confirm the change")
			}
		}
		if self.inbox.error.is_none() {
			self.status = "Inbox updated";
		}
	}
	pub fn clear_mentions(&mut self) -> Command {
		let operation = self.inbox.operation.take().filter(Action::write);
		let pending = self.inbox.loading && operation.is_some();
		self.inbox = View {
			request: self.inbox.request.wrapping_add(u64::from(!pending)),
			loading: pending,
			detached_write: pending,
			operation,
			..Default::default()
		};
		Command::CancelMentions
	}
	pub fn apply_mentions(&mut self, request: u64, result: Result<Vec<Message>, Failure>) {
		if !self.can_load_mentions() || !self.inbox.loading || self.inbox.request != request {
			return;
		}
		self.inbox.loading = false;
		match result {
			Ok(mut messages)
				if messages.len() <= PAGE_SIZE
					&& messages.capacity() <= PAGE_SIZE
					&& messages.iter().map(Message::bytes).sum::<usize>() <= MAX_BYTES
					&& messages.iter().all(|message| {
						message.id.0 != 0
							&& message.channel.0 != 0
							&& self.inbox.before.is_none_or(|before| message.id < before)
					}) && messages.windows(2).all(|pair| pair[0].id > pair[1].id) =>
			{
				self.inbox.next =
					(messages.len() == PAGE_SIZE).then(|| messages.last().unwrap().id);
				messages.retain(|message| self.can_read_history(message.channel));
				self.inbox.messages = messages;
				self.inbox.loaded = true;
			}
			Ok(_) => self.inbox.error = Some("Mention results were invalid or too large"),
			Err(failure) => {
				self.inbox.error = Some(failure.label());
				if failure.ends_session() && failure != Failure::Capacity {
					self.fail(failure);
				}
			}
		}
	}
}
