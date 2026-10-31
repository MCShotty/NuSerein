//! Provisional disk pages reuse the normal request and live reconciliation fences.
use crate::State;
use model::{Freshness, Id, Message};

impl State {
	pub fn pending_cached_history_target(&self) -> Option<Id> {
		self.selected
			.and_then(|channel| self.history_navigation.target(channel, self.request))
	}
	/// Hydrate a bounded cached range without completing or acknowledging its REST read.
	pub fn hydrate_cached_history_page(
		&mut self,
		channel: Id,
		request: u64,
		mut messages: Vec<Message>,
	) -> bool {
		if self.selected != Some(channel)
			|| self.request != request
			|| !self.history_pending
			|| self.freshness == Freshness::Unavailable
			|| !self.can_read_history(channel)
			|| messages.is_empty()
			|| messages.len() > 50
			|| messages.iter().any(|message| {
				message.channel != channel || !session_cache::Timeline::valid_message(message)
			}) || messages.iter().map(Message::bytes).sum::<usize>() > session_cache::MAX_BYTES
			|| messages
				.iter()
				.enumerate()
				.any(|(index, message)| messages[..index].iter().any(|old| old.id == message.id))
		{
			return false;
		}
		let target = self.pending_cached_history_target();
		if let Some(target) = target {
			if self.timeline.is_deleted(target)
				|| !messages.iter().any(|message| message.id == target)
			{
				return false;
			}
		} else if let Some(before) = self.history_before {
			if messages.iter().any(|message| message.id >= before) {
				return false;
			}
		} else if let Some(after) = self.history_after {
			messages.retain(|message| message.id > after);
			if messages.is_empty() {
				return false;
			}
		} else {
			return false;
		}
		if target.is_some()
			&& self
				.prepare_target_history(channel, request, &messages)
				.is_err()
		{
			return false;
		}
		if self
			.timeline
			.seed_cached_page(messages, target.is_some())
			.is_err()
		{
			return false;
		}
		self.revision = self.revision.wrapping_add(1);
		self.enforce_resident_budget();
		true
	}
}
