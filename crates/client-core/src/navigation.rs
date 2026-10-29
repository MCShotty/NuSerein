//! Navigation identity, selection and Back/Forward transitions.
use super::{
	Apply, Channel, Command, Freshness, Guild, Id, MAX_NAV, MAX_VIEWED_SERVERS, NavStep, Place,
	State, navigable, voice,
};

impl State {
	pub(crate) fn navigation_bytes(&self) -> usize {
		if let Some((channels, guilds, bytes)) = self.navigation_index.bytes.get()
			&& channels == self.channels.len()
			&& guilds == self.guilds.len()
		{
			return bytes;
		}
		let bytes = self.channels.iter().map(Channel::bytes).sum::<usize>()
			+ self.guilds.iter().map(Guild::bytes).sum::<usize>()
			+ self.expression_hints.bytes()
			+ self.server_display.bytes()
			+ (self.channels.capacity() - self.channels.len()) * size_of::<Channel>()
			+ (self.guilds.capacity() - self.guilds.len()) * size_of::<Guild>();
		self.set_navigation_bytes(bytes);
		bytes
	}
	pub(super) fn set_navigation_bytes(&self, bytes: usize) {
		self.navigation_index
			.bytes
			.set(Some((self.channels.len(), self.guilds.len(), bytes)));
	}
	pub(super) fn channel_stamp(&self) -> (usize, usize) {
		(self.channels.as_ptr() as usize, self.channels.len())
	}
	/// Index into the current channel slice; invalid after a navigation mutation.
	pub fn channel_index(&self, id: Id) -> Option<usize> {
		let stamp = self.channel_stamp();
		if self.navigation_index.channel_stamp.get() == Some(stamp) {
			// Message processing asks several subsystems about the same channel. Reuse
			// that lookup, while still checking the current slice and exact identity.
			if let Some((last, index)) = self.navigation_index.last_channel.get()
				&& last == id
				&& self
					.channels
					.get(index)
					.is_some_and(|channel| channel.id == id)
			{
				return Some(index);
			}
			let cached = self.navigation_index.channels.borrow().get(&id).copied();
			if cached.is_none_or(|index| {
				self.channels
					.get(index)
					.is_some_and(|channel| channel.id == id)
			}) {
				self.navigation_index
					.last_channel
					.set(cached.map(|index| (id, index)));
				return cached;
			}
		}
		let mut index = self.navigation_index.channels.borrow_mut();
		*index = self
			.channels
			.iter()
			.take(MAX_NAV)
			.enumerate()
			.map(|(index, channel)| (channel.id, index))
			.collect();
		self.navigation_index.channel_stamp.set(Some(stamp));
		let found = index.get(&id).copied();
		self.navigation_index
			.last_channel
			.set(found.map(|index| (id, index)));
		found
	}
	pub fn channel(&self, id: Id) -> Option<&Channel> {
		self.channel_index(id)
			.and_then(|index| self.channels.get(index))
	}
	/// Call after replacing IDs or payloads directly in synthetic navigation vectors.
	pub fn invalidate_navigation(&self) {
		self.navigation_index
			.invalidations
			.set(self.navigation_index.invalidations.get().wrapping_add(1));
		self.navigation_index.channel_stamp.set(None);
		self.navigation_index.last_channel.set(None);
		self.navigation_index.guild_stamp.set(None);
		self.navigation_index.guilds.borrow_mut().clear();
		self.navigation_index.bytes.set(None);
	}
	pub fn guild(&self, id: Id) -> Option<&Guild> {
		let stamp = (self.guilds.as_ptr() as usize, self.guilds.len());
		if self.navigation_index.guild_stamp.get() == Some(stamp) {
			let cached = self.navigation_index.guilds.borrow().get(&id).copied();
			if cached.is_none_or(|index| self.guilds.get(index).is_some_and(|guild| guild.id == id))
			{
				return cached.and_then(|index| self.guilds.get(index));
			}
		}
		let mut index = self.navigation_index.guilds.borrow_mut();
		*index = self
			.guilds
			.iter()
			.take(MAX_NAV)
			.enumerate()
			.map(|(index, guild)| (guild.id, index))
			.collect();
		self.navigation_index.guild_stamp.set(Some(stamp));
		index.get(&id).and_then(|index| self.guilds.get(*index))
	}
	pub(super) fn remember_channel(&mut self, channel: Id) {
		if self
			.channel(channel)
			.is_some_and(|c| c.guild.is_none() && matches!(c.kind, 1 | 3))
		{
			self.last_viewed_dm = Some(channel);
			return;
		}
		let Some(guild) = self.channel(channel).and_then(|c| c.guild) else {
			return;
		};
		self.last_viewed_channels.retain(|(id, _)| *id != guild);
		if self.last_viewed_channels.len() == MAX_VIEWED_SERVERS {
			self.last_viewed_channels.remove(0);
		}
		self.last_viewed_channels.push((guild, channel));
	}
	/// Restores saved server visits (oldest first) behind the ones made this session.
	pub fn seed_viewed_channels(&mut self, saved: &[(Id, Id)]) {
		let room = MAX_VIEWED_SERVERS.saturating_sub(self.last_viewed_channels.len());
		let mut fresh: Vec<_> = saved
			.iter()
			.rev()
			.filter(|(guild, _)| !self.last_viewed_channels.iter().any(|(id, _)| id == guild))
			.take(room)
			.copied()
			.collect();
		fresh.reverse();
		self.last_viewed_channels.splice(0..0, fresh);
	}
	pub fn select_guild(&mut self, guild: Id) -> Option<Command> {
		let channel = self.guild_target(guild)?;
		self.select(channel)
	}
	/// The channel a server icon opens: the current one, the last viewed, or the first text one.
	pub fn guild_target(&self, guild: Id) -> Option<Id> {
		self.guild(guild)?;
		let available = |id| {
			self.channel(id).is_some_and(|channel| {
				channel.guild == Some(guild) && navigable(channel) && self.can_view(id)
			})
		};
		self.selected
			.filter(|id| available(*id))
			.or_else(|| {
				self.last_viewed_channels
					.iter()
					.find(|(id, channel)| *id == guild && available(*channel))
					.map(|(_, id)| *id)
			})
			.or_else(|| {
				self.channels
					.iter()
					.filter(|c| c.guild == Some(guild) && available(c.id))
					// Prefer ordinary text/forum channels over threads or voice on first visit.
					.min_by_key(|c| (matches!(c.kind, 2 | 10..=12), c.position, c.id))
					.map(|c| c.id)
			})
	}
	pub fn select(&mut self, channel: Id) -> Option<Command> {
		self.cancel_invite_navigation(channel);
		// Keep the current conversation intact, but allow a restored channel to load again.
		if self.selected == Some(channel) && self.freshness != Freshness::Unavailable {
			if self.browse_channels.take().is_some() {
				self.flush_channel_preferences();
				self.record(Place::Channel(channel));
				self.revision += 1;
			}
			return None;
		}
		match self.apply_channel(channel) {
			Apply::Opened(command) => {
				self.record(Place::Channel(channel));
				command
			}
			Apply::AlreadyHere => None,
			Apply::Rejected(status) => {
				self.status = status;
				None
			}
		}
	}
	pub(super) fn cancel_invite_navigation(&mut self, channel: Id) {
		// Choosing another server's channel while a join is pending cancels its navigation.
		if let Some((_, guild)) = self.invite_join.navigate
			&& self
				.channel(channel)
				.is_some_and(|c| c.guild != Some(guild))
		{
			self.invite_join.navigate = None;
		}
	}
	pub(super) fn select_chat_link_channel(
		&mut self,
		channel: Id,
		target: Id,
	) -> Result<(), &'static str> {
		self.cancel_invite_navigation(channel);
		let browsing = self.browse_channels.is_some();
		match self.apply_channel_with_target(channel, Some(target)) {
			Apply::Opened(_) => {
				self.record(Place::Channel(channel));
				Ok(())
			}
			Apply::AlreadyHere => {
				if browsing {
					self.browse_channels = None;
					self.flush_channel_preferences();
					self.record(Place::Channel(channel));
					self.revision += 1;
				}
				Ok(())
			}
			Apply::Rejected(status) => Err(status),
		}
	}
	pub(super) fn record(&mut self, place: Place) {
		if self.trail.is_empty() && !matches!(place, Place::Home) {
			self.trail.visit(Place::Home);
		}
		self.trail.visit(place);
	}
	pub(super) fn apply_channel(&mut self, channel: Id) -> Apply {
		self.apply_channel_with_target(channel, None)
	}
	pub(super) fn apply_channel_with_target(&mut self, channel: Id, target: Option<Id>) -> Apply {
		if self.selected == Some(channel) && self.freshness != Freshness::Unavailable {
			return Apply::AlreadyHere;
		}
		if !self.channel(channel).is_some_and(navigable) {
			return Apply::Rejected("This channel kind is unsupported");
		}
		if !self.can_view(channel) {
			return Apply::Rejected("Channel permissions are unavailable or access was revoked");
		}
		if self.browse_channels.take().is_some() {
			self.flush_channel_preferences();
		}
		self.history_navigation.clear_back();
		if let Some(previous) = self.selected {
			self.remember_channel(previous);
		}
		let parked_reply = self.swap_parked_reply(channel);
		self.remember_channel(channel);
		if self
			.channel(channel)
			.is_some_and(|c| matches!(c.kind, 10..=12))
		{
			self.last_viewed_threads.retain(|id| *id != channel);
			self.last_viewed_threads.truncate(1023);
			self.last_viewed_threads.insert(0, channel);
		}
		self.retire_archived_thread(Some(channel));
		self.typing.clear();
		let target_current = self.resident_target_current(channel);
		let trusted = self.select_resident(channel);
		self.history_targeted = false;
		self.history_navigation.clear();
		if self.shared_member_list_id(channel).is_none() {
			self.members = None;
		}
		self.member_search = Default::default();

		self.selected = Some(channel);
		self.clear_search();
		self.reset_thread_starter();
		self.search_target = None;
		self.restore_scroll = false;
		self.reactions.reset();
		self.polls.reset();
		self.interactions.reset();
		let scope = self.application_command_scope(channel);
		self.application_commands.retain(scope);
		self.older_exhausted = false;
		self.restore_reply(channel, parked_reply);
		self.revision += 1;
		if self.channel(channel).is_some_and(|c| !c.supports_text()) {
			self.cancel_history();
			self.freshness = Freshness::Fresh;
			return Apply::Opened(None);
		}
		if let Some(target) = target {
			self.cancel_history();
			self.history_before = None;
			// A restored resident is a session-local, previously fresh window;
			// live mutations mark it stale and access changes retire it. Only a
			// window holding the live target is presented as loaded: a deletion-only or
			// unrelated window stays Stale, and the caller requests the target's page.
			self.freshness = if target_current
				&& self
					.timeline
					.get(target)
					.is_some_and(|message| message.channel == channel)
				&& !self.timeline.is_deleted(target)
			{
				Freshness::Fresh
			} else {
				Freshness::Stale
			};
			return Apply::Opened(None);
		}
		if let Some(message) = self.reading(channel).and_then(|cursor| cursor.message) {
			if target_current
				&& self.timeline.get(message).is_some()
				&& !self.timeline.is_deleted(message)
			{
				self.cancel_history();
				self.history_before = None;
				self.history_targeted = false;
				self.search_target = Some(message);
				self.restore_scroll = true;
				self.revision += 1;
				if self.gateway_connected {
					self.freshness = Freshness::Fresh;
				}
				return Apply::Opened(None);
			}
			if let Some(command) = self.open_scrolled_window(message) {
				return Apply::Opened(Some(command));
			}
		}
		if let Some(trusted) = trusted {
			self.cancel_history();
			self.history_before = None;
			self.older_exhausted = trusted.older_exhausted;
			self.freshness = Freshness::Fresh;
			return Apply::Opened(self.call_sync(channel));
		}
		let command = self.history(None);
		if let Command::History { request, .. } = command
			&& self
				.preload
				.conversations
				.adopt(channel, request, std::time::Instant::now())
		{
			// The page this click needs is already on its way.
			return Apply::Opened(self.call_sync(channel));
		}
		Apply::Opened(Some(command))
	}
	/// Opening without a page request still synchronizes a direct call's state, as the host
	/// does alongside a recent-page request.
	pub(super) fn call_sync(&self, channel: Id) -> Option<Command> {
		(self.channel(channel).is_some_and(|c| c.guild.is_none()) && self.can_call(channel))
			.then_some(Command::Voice(voice::Command::Sync { channel }))
	}
	/// Return to the last available direct message, or Friends when none remains.
	pub fn open_messages(&mut self) -> Option<Command> {
		if let Some(channel) = self.last_viewed_dm.filter(|id| {
			self.channel(*id)
				.is_some_and(|c| c.guild.is_none() && matches!(c.kind, 1 | 3))
				&& self.can_view(*id)
		}) {
			return self.select(channel);
		}
		self.open_home();
		None
	}
	/// Open Friends / Home. Does not clear the timeline or emit a command.
	pub fn open_home(&mut self) {
		self.browse_channels = None;
		self.flush_channel_preferences();
		self.history_navigation.clear_back();
		self.park_reply();
		self.reset_thread_starter();
		self.last_viewed_dm = None;
		self.application_commands.clear();
		self.selected = None;
		self.history_navigation.clear();
		self.search_target = None;
		self.restore_scroll = false;
		self.record(Place::Home);
	}
	/// Land on Home because the open channel is gone.
	pub fn arrived_home(&mut self) {
		self.browse_channels = None;
		self.flush_channel_preferences();
		self.history_navigation.clear_back();
		self.reset_thread_starter();
		self.application_commands.clear();
		if let Some(Place::Channel(id)) = self.trail.current()
			&& self.selected == Some(id)
		{
			self.trail.drop_current();
		}
		self.selected = None;
		self.history_navigation.clear();
		self.search_target = None;
		self.restore_scroll = false;
		self.record(Place::Home);
	}
	pub fn navigate_back(&mut self) -> Option<NavStep> {
		if self.browse_channels.is_none() && self.can_return_from_reply() {
			return Some(NavStep {
				command: self.return_from_reply(),
			});
		}
		self.navigate(true)
	}
	pub fn navigate_forward(&mut self) -> Option<NavStep> {
		self.navigate(false)
	}
	pub(super) fn navigate(&mut self, back: bool) -> Option<NavStep> {
		let mut blocked = None;
		loop {
			let Some(place) = (if back {
				self.trail.peek_back()
			} else {
				self.trail.peek_forward()
			}) else {
				if let Some(status) = blocked {
					self.status = status;
				}
				return None;
			};
			let apply = match place {
				Place::Home => {
					self.browse_channels = None;
					self.flush_channel_preferences();
					self.history_navigation.clear_back();
					self.park_reply();
					self.application_commands.clear();
					self.selected = None;
					self.history_navigation.clear();
					self.search_target = None;
					self.restore_scroll = false;
					Apply::Opened(None)
				}
				Place::Channel(channel) => {
					let result = self.apply_channel(channel);
					if matches!(result, Apply::Opened(_) | Apply::AlreadyHere) {
						self.browse_channels = None;
						self.flush_channel_preferences();
					}
					result
				}
				Place::BrowseChannels(guild) => {
					if self.guild(guild).is_some() {
						self.browse_channels = Some(guild);
						self.revision += 1;
						Apply::Opened(None)
					} else {
						Apply::Rejected("This server is no longer available")
					}
				}
			};
			match apply {
				Apply::Opened(command) => {
					self.commit_nav(back);
					return Some(NavStep { command });
				}
				Apply::AlreadyHere => {
					self.commit_nav(back);
					return Some(NavStep { command: None });
				}
				Apply::Rejected(status) => {
					blocked = Some(status);
					if back {
						self.trail.drop_back();
					} else {
						self.trail.drop_forward();
					}
				}
			}
		}
	}

	/// A personal channel-list page shares the existing bounded navigation trail.
	/// The selected conversation and its reading anchor remain resident and unread.
	pub fn open_channel_browser(&mut self, guild: Id) -> bool {
		if self.guild(guild).is_none() {
			return false;
		}
		if self.browse_channels == Some(guild) {
			return true;
		}
		if self.trail.is_empty() {
			self.record(self.selected.map_or(Place::Home, Place::Channel));
		}
		self.browse_channels = Some(guild);
		self.record(Place::BrowseChannels(guild));
		self.revision += 1;
		true
	}
	pub(super) fn commit_nav(&mut self, back: bool) {
		if back {
			self.trail.commit_back();
		} else {
			self.trail.commit_forward();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ReadingCursor;

	#[test]
	fn browser_back_forward_preserves_conversation_and_reading_anchor() {
		let mut state = crate::personal_channels::tests::state();
		state.select(Id(3));
		state.freshness = Freshness::Fresh;
		state.remember_reading(
			Id(3),
			ReadingCursor {
				message: Some(Id(100)),
				inset: 27.5,
			},
		);
		let request = state.request;
		let pending = state.history_pending;
		assert!(state.open_channel_browser(Id(2)));
		assert_eq!(state.selected, Some(Id(3)));
		assert_eq!(state.request, request);
		assert_eq!(state.history_pending, pending);
		assert!(state.navigate_back().unwrap().command.is_none());
		assert_eq!(state.browse_channels, None);
		assert_eq!(state.reading(Id(3)).unwrap().message, Some(Id(100)));
		assert_eq!(state.reading(Id(3)).unwrap().inset, 27.5);
		assert!(state.navigate_forward().unwrap().command.is_none());
		assert_eq!(state.browse_channels, Some(Id(2)));
		assert_eq!(
			state.request, request,
			"browser navigation issues no history request or acknowledgement"
		);
		assert!(state.select(Id(3)).is_none());
		assert_eq!(state.browse_channels, None);
		assert!(state.navigate_back().unwrap().command.is_none());
		assert_eq!(state.browse_channels, Some(Id(2)));
	}

	#[test]
	fn browser_rejects_removed_servers_and_channel_loss_returns_home() {
		let mut state = crate::personal_channels::tests::state();
		assert!(!state.open_channel_browser(Id(999)));
		state.select(Id(3));
		state.freshness = Freshness::Fresh;
		assert!(state.open_channel_browser(Id(2)));
		assert!(state.navigate_back().unwrap().command.is_none());
		state.guilds.clear();
		assert!(state.navigate_forward().is_none());
		assert_eq!(state.browse_channels, None);
		state.guilds = crate::personal_channels::tests::state().guilds;
		assert!(state.open_channel_browser(Id(2)));
		state.arrived_home();
		assert_eq!(state.browse_channels, None);
		assert_eq!(state.selected, None);
	}
	#[test]
	fn leaving_browser_flushes_authorized_saves_without_touching_history() {
		let mut state = crate::personal_channels::tests::state();
		state.select(Id(3));
		state.freshness = Freshness::Fresh;
		let history_request = state.request;
		state.open_channel_browser(Id(2));
		let change = state.personal_visibility_change(Id(4), false).unwrap();
		state.request_personal_channel_action(Id(2), change);
		assert_eq!(state.channel_in_custom_list(Id(4)), Some(false));
		assert!(state.navigate_back().unwrap().command.is_none());
		assert_eq!(state.browse_channels, None);
		assert_eq!(state.request, history_request);
		assert!(
			matches!(state.poll_channel_preferences(std::time::Instant::now()),Some(Command::ChannelPreferences(request)) if request.guild==Id(2))
		);
	}
}
