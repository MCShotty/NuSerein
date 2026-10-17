//! Session-only saved messages. Summaries never substitute for channel access checks.
use crate::{Id, Message};

pub const MAX_SAVED: usize = 500;
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct SavedMessage {
	pub channel: Id,
	pub message: Id,
	pub guild: Option<Id>,
	/// Unix nanoseconds, matching protocol timestamps.
	pub due_at: Option<i128>,
	pub body: Option<Message>,
}
impl SavedMessage {
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.body.as_ref().map_or(0, Message::bytes)
	}
	pub fn valid(&self) -> bool {
		self.channel.0 != 0
			&& self.message.0 != 0
			&& self.guild.is_none_or(|id| id.0 != 0)
			&& self
				.due_at
				.is_none_or(|value| (0..=253_402_300_799_999_999_999).contains(&value))
			&& self.body.as_ref().is_none_or(|body| {
				body.channel == self.channel && body.id == self.message && !body.ephemeral
			}) && self.bytes() <= MAX_BYTES
	}
}

pub fn valid_saved(items: &[SavedMessage]) -> bool {
	let mut seen = std::collections::BTreeSet::new();
	items.len() <= MAX_SAVED
		&& items.iter().map(SavedMessage::bytes).sum::<usize>() <= MAX_BYTES
		&& items
			.iter()
			.all(|item| item.valid() && seen.insert((item.channel, item.message)))
}
