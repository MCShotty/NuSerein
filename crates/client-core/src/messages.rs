//! Incoming message reconciliation, independent of event admission.
use super::{Freshness, Message, State};

impl State {
	pub(super) fn apply_message(&mut self, mut m: Message) -> Result<(), &'static str> {
		self.ingest_reply_preview(&mut m);
		if self.message_actions.edit_pending(m.channel, m.id)
			&& self.timeline.get(m.id).is_some_and(|old| {
				old.edited_at
					.is_none_or(|at| m.edited_at.is_some_and(|new| new >= at))
			}) {
			self.message_actions.observe_content(m.channel, m.id);
		}
		self.typing_message(&m);
		if let Some(post) = self
			.channel_index(m.channel)
			.and_then(|index| self.channels.get_mut(index))
			.filter(|c| matches!(c.kind, 10..=12))
		{
			post.message_count = post.message_count.map(|n| n.saturating_add(1));
		}
		let deletion = if m.reply_deleted && self.accepts_reply_source(&m) {
			self.record_reply_deletion(&m);
			if self.selected == Some(m.channel) {
				self.timeline.observe_deleted_reference(&m)
			} else {
				Ok(())
			}
		} else {
			Ok(())
		};
		self.observe_notification(&m);
		self.observe_last_message(m.channel, m.id);
		if self.selected == Some(m.channel)
			&& self.reactions.invalidated(m.id)
			&& m.reactions.is_some()
		{
			self.queue_reaction_read(m.id);
		}
		if self.reactions.invalidated(m.id) {
			m.reactions = self
				.timeline
				.get(m.id)
				.and_then(|old| old.reactions.clone());
		}
		self.confirm(&m);
		if deletion.is_err() {
			deletion
		} else if self.selected == Some(m.channel)
                    && self.can_view(m.channel)
                    && self.freshness != Freshness::Unavailable
                    // Do not splice a new live tail into a deliberately browsed history page.
                    // Loaded edits still reconcile; navigation/read state continues to observe it.
                    && (!self.history_targeted || self.timeline.get(m.id).is_some())
		{
			self.timeline.insert(m, true, false)
		} else {
			Ok(())
		}
	}
}
