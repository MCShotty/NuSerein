//! Visible-only, bounded reply summary reads. Preview bodies never become actionable timeline rows.
use crate::{
	Command, Event, State,
	auth::{AuthState, Failure},
};
use model::{Freshness, Id, Message, ReplyPreview};
use std::collections::VecDeque;

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_VISIBLE_REPLIES: usize = session_cache::MAX_MESSAGES;
const MAX_QUEUED: usize = 32;
const MAX_INFLIGHT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
	Unrequested,
	Loading,
	Ready,
	Deleted,
	Unavailable,
	Forbidden,
	Failed,
}

pub struct ResultEvent {
	pub channel: Id,
	pub message: Id,
	pub request: u64,
	pub result: Result<Option<Box<ReplyPreview>>, Failure>,
}

enum Value {
	Ready(Box<ReplyPreview>),
	Unavailable,
	Forbidden,
	Failed,
}
struct Entry {
	channel: Id,
	id: Id,
	value: Value,
}
#[derive(Default)]
pub struct Previews {
	entries: VecDeque<Entry>,
	visible: Vec<Id>,
	// Completed reads must not restart on repaint merely because their bodies were evicted.
	attempted: Vec<Id>,
	queued: Vec<Id>,
	inflight: Vec<(Id, Id, u64)>,
	cancel_requested: bool,
	channel: Option<Id>,
	next: u64,
}
impl Previews {
	pub(crate) fn cancel_pending(&mut self) {
		self.cancel_requested |= !self.inflight.is_empty();
		self.attempted
			.retain(|id| !self.inflight.iter().any(|(_, pending, _)| pending == id));
		self.inflight.clear();
	}
	pub(crate) fn clear(&mut self) {
		self.cancel_pending();
		self.entries = VecDeque::new();
		self.visible = Vec::new();
		self.attempted = Vec::new();
		self.queued = Vec::new();
		self.channel = None;
		// Preserve next: queued responses from before clearing must never match a new read.
	}
	fn metadata_bytes(&self) -> usize {
		std::mem::size_of::<Self>()
			+ self.entries.capacity() * std::mem::size_of::<Entry>()
			+ self.visible.capacity() * std::mem::size_of::<Id>()
			+ self.attempted.capacity() * std::mem::size_of::<Id>()
			+ self.queued.capacity() * std::mem::size_of::<Id>()
			+ self.inflight.capacity() * std::mem::size_of::<(Id, Id, u64)>()
	}
	pub(crate) fn bytes(&self) -> usize {
		self.metadata_bytes()
			+ self
				.entries
				.iter()
				.map(|entry| match &entry.value {
					Value::Ready(preview) => preview.bytes(),
					_ => 0,
				})
				.sum::<usize>()
	}
	fn remember_attempt(&mut self, channel: Id, id: Id) {
		if self.channel == Some(channel)
			&& self.visible.contains(&id)
			&& !self.attempted.contains(&id)
			&& self.attempted.len() < MAX_VISIBLE_REPLIES
		{
			self.attempted.push(id);
		}
	}
	fn insert(&mut self, channel: Id, id: Id, value: Value) {
		self.entries
			.retain(|entry| entry.channel != channel || entry.id != id);
		self.entries.push_back(Entry { channel, id, value });
		while self.entries.len() > MAX_ENTRIES || self.bytes() > MAX_BYTES {
			self.entries.pop_front();
		}
	}
	pub(crate) fn evict(&mut self) -> bool {
		self.entries.pop_front().is_some()
	}
}

impl State {
	/// Display-only lookup; callers must never offer edit/delete/reaction actions for this body.
	pub fn reply_original(&self, id: Id) -> Option<&Message> {
		let channel = self.selected?;
		if !self.can_read_history(channel) || self.timeline.is_deleted(id) {
			return None;
		}
		if let Some(message) = self
			.timeline
			.get(id)
			.filter(|message| message.channel == channel)
		{
			return Some(message);
		}
		if let Some(entry) = self
			.reply_previews
			.entries
			.iter()
			.find(|entry| entry.channel == channel && entry.id == id)
		{
			match &entry.value {
				Value::Ready(preview) => return Some(preview.message()),
				Value::Unavailable | Value::Forbidden => return None,
				Value::Failed => {}
			}
		}
		self.history_navigation.back_original(channel, id)
	}
	pub fn reply_preview_status(&self, id: Id) -> Status {
		let Some(channel) = self.selected else {
			return Status::Unavailable;
		};
		if !self.can_read_history(channel) {
			return Status::Forbidden;
		}
		if self.timeline.is_deleted(id)
			|| self
				.timeline
				.iter()
				.any(|m| m.reply_to == Some(id) && m.reply_deleted)
		{
			return Status::Deleted;
		}
		if self.reply_original(id).is_some() {
			return Status::Ready;
		}
		if self
			.reply_previews
			.inflight
			.iter()
			.any(|(c, target, _)| *c == channel && *target == id)
		{
			return Status::Loading;
		}
		self.reply_previews
			.entries
			.iter()
			.find(|entry| entry.channel == channel && entry.id == id)
			.map_or_else(
				|| {
					if self.reply_previews.channel == Some(channel)
						&& self.reply_previews.attempted.contains(&id)
					{
						Status::Failed
					} else {
						Status::Unrequested
					}
				},
				|entry| match &entry.value {
					Value::Ready(_) => Status::Ready,
					Value::Unavailable => Status::Unavailable,
					Value::Forbidden => Status::Forbidden,
					Value::Failed => Status::Failed,
				},
			)
	}
	/// Called once after painting with visible and overscan reply target IDs, never all history.
	pub fn sync_visible_reply_previews(&mut self, targets: &[Id]) -> Vec<Command> {
		let previous_metadata = self.reply_previews.metadata_bytes();
		let mut commands = Vec::new();
		let channel = self
			.selected
			.filter(|channel| self.can_read_history(*channel));
		let visible: Vec<_> = targets
			.iter()
			.take(MAX_VISIBLE_REPLIES)
			.copied()
			.filter(|id| id.0 != 0)
			.fold(Vec::new(), |mut ids, id| {
				if ids.len() < MAX_VISIBLE_REPLIES && !ids.contains(&id) {
					ids.push(id);
				}
				ids
			});
		let channel_changed = self.reply_previews.channel != channel;
		if channel_changed
			|| self
				.reply_previews
				.inflight
				.iter()
				.any(|(_, id, _)| !visible.contains(id))
		{
			self.reply_previews.cancel_pending();
		}
		if std::mem::take(&mut self.reply_previews.cancel_requested) {
			commands.push(Command::CancelReplyPreviews);
		}
		if channel_changed {
			self.reply_previews.attempted.clear();
		} else {
			self.reply_previews
				.attempted
				.retain(|id| visible.contains(id));
		}
		self.reply_previews.channel = channel;
		self.reply_previews.visible = visible;
		self.reply_previews.queued.clear();
		if self.auth != AuthState::Authenticated
			|| !self.gateway_connected
			|| self.freshness != Freshness::Fresh
			|| self.history_pending
		{
			return self.finish_reply_preview_sync(previous_metadata, commands);
		}
		let Some(channel) = channel else {
			return self.finish_reply_preview_sync(previous_metadata, commands);
		};
		let visible = self.reply_previews.visible.clone();
		for id in visible {
			if self.reply_previews.queued.len() == MAX_QUEUED {
				break;
			}
			if self.reply_preview_status(id) != Status::Unrequested {
				continue;
			}
			// Only genuine loaded reply references can initiate service reads.
			if !self
				.timeline
				.iter()
				.any(|m| m.channel == channel && m.reply_to == Some(id))
			{
				continue;
			}
			self.reply_previews.queued.push(id);
		}
		while self.reply_previews.inflight.len() < MAX_INFLIGHT
			&& !self.reply_previews.queued.is_empty()
		{
			let id = self.reply_previews.queued.remove(0);
			self.reply_previews.next = self.reply_previews.next.wrapping_add(1);
			let request = self.reply_previews.next;
			self.reply_previews.remember_attempt(channel, id);
			self.reply_previews.inflight.push((channel, id, request));
			commands.push(Command::ReplyPreview {
				channel,
				message: id,
				request,
			});
		}
		self.finish_reply_preview_sync(previous_metadata, commands)
	}
	fn finish_reply_preview_sync(
		&mut self,
		previous_metadata: usize,
		commands: Vec<Command>,
	) -> Vec<Command> {
		// Painting allocates this metadata outside apply(), which normally enforces the
		// shared budget. The constant-time charge avoids scanning it again on every repaint.
		if self.reply_previews.metadata_bytes() > previous_metadata {
			self.enforce_resident_budget();
		}
		commands
	}
	pub fn retry_reply_preview(&mut self, id: Id) -> Vec<Command> {
		let Some(channel) = self.selected else {
			return Vec::new();
		};
		if self.reply_preview_status(id) == Status::Deleted {
			return Vec::new();
		}
		if self.reply_previews.channel == Some(channel) {
			self.reply_previews.attempted.retain(|target| *target != id);
		}
		self.reply_previews
			.entries
			.retain(|entry| entry.channel != channel || entry.id != id);
		let mut visible = self.reply_previews.visible.clone();
		if visible.contains(&id) {
			visible.retain(|target| *target != id);
			visible.insert(0, id);
			self.reply_previews.cancel_pending();
		}
		self.sync_visible_reply_previews(&visible)
	}
	pub(crate) fn ingest_reply_preview(&mut self, message: &mut Message) {
		if message.reply_deleted
			&& let Some(id) = message.reply_to
		{
			self.reply_previews
				.entries
				.retain(|entry| entry.channel != message.channel || entry.id != id);
		}
		if let Some(preview) = message.reply_preview.take()
			&& message.reply_to == Some(preview.message().id)
			&& message.channel == preview.message().channel
			&& self.can_read_history(message.channel)
		{
			if self
				.reply_previews
				.inflight
				.iter()
				.any(|(channel, id, _)| *channel == message.channel && *id == preview.message().id)
			{
				self.reply_previews.cancel_pending();
			}
			self.reply_previews
				.remember_attempt(message.channel, preview.message().id);
			self.reply_previews.insert(
				message.channel,
				preview.message().id,
				Value::Ready(preview),
			);
		}
	}
	pub(crate) fn apply_reply_preview(&mut self, event: ResultEvent) {
		let Some(index) = self
			.reply_previews
			.inflight
			.iter()
			.position(|scope| *scope == (event.channel, event.message, event.request))
		else {
			return;
		};
		self.reply_previews.inflight.remove(index);
		if self.selected != Some(event.channel)
			|| !self.can_read_history(event.channel)
			|| self.timeline.is_deleted(event.message)
		{
			return;
		}
		let value = match event.result {
			Ok(Some(preview))
				if preview.message().id == event.message
					&& preview.message().channel == event.channel
					&& preview.bytes() <= model::MAX_REPLY_PREVIEW_BYTES =>
			{
				Value::Ready(preview)
			}
			Ok(None) => Value::Unavailable,
			Err(Failure::Forbidden) => Value::Forbidden,
			Err(failure) if failure.ends_session() => {
				self.fail(failure);
				return;
			}
			_ => Value::Failed,
		};
		self.reply_previews
			.insert(event.channel, event.message, value);
	}
	pub(crate) fn invalidate_reply_previews(&mut self, event: &Event) {
		let key = match event {
			Event::Delete { channel, id } => Some((*channel, *id)),
			Event::Patch(patch) => Some((patch.channel, patch.id)),
			Event::Message(message) => Some((message.channel, message.id)),
			Event::Edited {
				channel,
				message,
				result: Ok(_),
				..
			} => Some((*channel, *message)),
			_ => None,
		};
		if let Some((channel, id)) = key {
			if self
				.reply_previews
				.inflight
				.iter()
				.any(|(pending_channel, pending_id, _)| {
					*pending_channel == channel && *pending_id == id
				}) {
				self.reply_previews.cancel_pending();
			}
			if self.reply_previews.channel == Some(channel) {
				self.reply_previews.attempted.retain(|target| *target != id);
			}
			self.reply_previews
				.entries
				.retain(|entry| entry.channel != channel || entry.id != id);
		}
		if let Event::DeleteBulk { channel, ids } = event {
			if self
				.reply_previews
				.inflight
				.iter()
				.any(|(pending_channel, pending_id, _)| {
					*pending_channel == *channel && ids.contains(pending_id)
				}) {
				self.reply_previews.cancel_pending();
			}
			if self.reply_previews.channel == Some(*channel) {
				self.reply_previews
					.attempted
					.retain(|target| !ids.contains(target));
			}
			self.reply_previews
				.entries
				.retain(|entry| entry.channel != *channel || !ids.contains(&entry.id));
		}
		if event.changes_access() || matches!(event, Event::Disconnected | Event::Resumed) {
			self.reply_previews.entries.clear();
			self.reply_previews.cancel_pending();
			self.reply_previews.attempted.clear();
		}
		// A return window is only a cache: changes revalidate it instead of retaining stale content.
		if let Some((channel, _)) = key {
			self.history_navigation.invalidate_back(channel);
		}
		if let Event::DeleteBulk { channel, .. } = event {
			self.history_navigation.invalidate_back(*channel);
		}
		if event.changes_access() || matches!(event, Event::Disconnected | Event::Resumed) {
			self.history_navigation.evict_back();
		}
	}
}
