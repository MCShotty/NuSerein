//! History requests, reading positions and pagination.
use super::{Command, Freshness, Id, ReadingCursor, State};

impl State {
	pub fn history(&mut self, before: Option<Id>) -> Command {
		if let Some(parent) = self.selected.filter(|id| self.is_forum(*id)) {
			self.reload_forum_posts(parent);
			return self
				.request_forum_posts(parent, false)
				.unwrap_or(Command::CancelSearch);
		}
		self.history_range(before, None)
	}
	pub(super) fn open_scrolled_window(&mut self, message: Id) -> Option<Command> {
		if message.0 <= 1 || self.timeline.is_deleted(message) {
			return None;
		}
		let after = Id(message.0 - 1);
		if self.timeline.get(message).is_none() {
			self.timeline.clear_window_preserving_deletions();
		}
		self.newer_cursor = None;
		self.newer_may_have_more = false;
		self.revision += 1;
		let command = self.history_range(None, Some(after));
		self.timeline.begin_page(false);
		self.history_targeted = true;
		self.search_target = Some(message);
		self.restore_scroll = true;
		self.enforce_resident_budget();
		Some(command)
	}
	pub fn reading(&self, channel: Id) -> Option<ReadingCursor> {
		self.reading
			.iter()
			.find(|(id, _)| *id == channel)
			.map(|(_, cursor)| *cursor)
	}
	pub fn remember_reading(&mut self, channel: Id, cursor: ReadingCursor) {
		if channel.0 == 0 {
			return;
		}
		self.reading.retain(|(id, _)| *id != channel);
		self.reading.push((channel, cursor));
		if self.reading.len() > Self::MAX_READING_CURSORS {
			self.reading.remove(0);
		}
	}
	pub(super) fn history_range(&mut self, before: Option<Id>, after: Option<Id>) -> Command {
		self.reply_previews.cancel_pending();
		if self.selected.is_some_and(|channel| {
			self.history_navigation
				.target(channel, self.request)
				.is_some()
		}) {
			self.search_target = None;
			self.restore_scroll = false;
		}
		self.history_navigation.clear();
		self.typing.clear();
		if before.is_none() {
			self.newer_cursor = None;
			self.newer_may_have_more = false;
		}
		if before.is_none() && after.is_none() {
			self.history_targeted = false;
		}
		let Some(channel) = self.selected.filter(|id| {
			self.channels
				.iter()
				.any(|channel| channel.id == *id && channel.supports_text())
		}) else {
			self.arrived_home();
			self.search_target = None;
			self.reply = None;
			self.invalidate_members();
			self.timeline.clear();
			self.cancel_history();
			self.freshness = Freshness::Unavailable;
			self.status = "Conversation no longer available in navigation";
			return self.clear_search();
		};
		if !self.can_read_history(channel) {
			self.cancel_history();
			self.timeline.clear();
			self.reply = None;
			self.freshness = if self.can_view(channel) && self.gateway_connected {
				Freshness::Fresh
			} else {
				Freshness::Unavailable
			};
			self.status = "Message history is unavailable with the current permissions";
			return self.clear_search();
		}
		if self.search.as_ref().is_some_and(|s| s.loading && s.pins)
			|| self.archives.as_ref().is_some_and(|s| s.loading)
		{
			self.clear_search();
		}
		if before.is_none() {
			self.search_target = None;
		}
		self.reactions.cancel_read();
		self.request += 1;
		self.history_before = before;
		self.history_after = after;
		self.history_pending = true;
		self.freshness = Freshness::Loading;
		if before.is_none() && after.is_some() {
			self.timeline.begin_append();
		} else {
			self.timeline.begin_page(before.is_some());
		}
		Command::History {
			channel,
			before,
			after,
			request: self.request,
		}
	}
	pub fn can_load_older(&self) -> bool {
		self.selected
			.is_some_and(|channel| self.can_read_history(channel))
			&& self.gateway_connected
			&& self.freshness == Freshness::Fresh
			&& !self.history_pending
			&& !self.older_exhausted
			&& self.timeline.row_count() != 0
	}
	pub fn older_history(&mut self) -> Option<Command> {
		if !self.can_load_older() {
			return None;
		}
		let before = self.timeline.row_ids().next()?;
		Some(self.history(Some(before)))
	}
	pub(super) fn cancel_history(&mut self) {
		self.history_navigation.clear();
		self.typing.clear();
		self.reactions.reset();
		self.polls.reset();
		self.interactions.reset();
		self.search_target = None;
		self.restore_scroll = false;
		self.request += 1;
		self.history_pending = false;
		self.history_after = None;
		self.newer_cursor = None;
		self.newer_may_have_more = false;
		self.timeline.cancel_page();
	}
}
