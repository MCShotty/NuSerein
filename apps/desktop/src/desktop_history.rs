//! Coordinate bounded disk history work and explicit cleanup.
use super::{Desktop, Event, cache, wants_cached_history};

impl Desktop {
	/// Shows the selected conversation's cached window while its recent page loads.
	pub(super) fn load_cached_history(&mut self) {
		let (state, request) = (&self.state, self.state.request);
		if let Some(channel) = state.selected
			&& self.cached_history_request != Some((channel, request))
			&& state.history_before.is_none()
			&& state.history_after.is_none()
			&& !state.history_targeted
			&& wants_cached_history(state, channel, request)
		{
			self.cached_history_request = Some((channel, request));
			self.queue_cache(cache::Operation::LoadChannel { channel, request });
		}
	}
	pub(super) fn queue_cache(&mut self, operation: cache::Operation) -> bool {
		if let Some(user) = &self.state.user {
			if matches!(operation, cache::Operation::ClearHistory) {
				self.request_history_clear(user.id);
				return true;
			}
			self.queue_cache_for(user.id, operation)
		} else {
			false
		}
	}
	pub(super) fn queue_cache_for(
		&mut self,
		account: model::Id,
		operation: cache::Operation,
	) -> bool {
		if self.state.demo || self.fixture_only {
			return false;
		}
		if let Some(cache) = &self.cache {
			if matches!(
				operation,
				cache::Operation::LoadChannel { .. }
					| cache::Operation::SaveChannel { .. }
					| cache::Operation::SaveChanges { .. }
			) && !cache.history.allows(cache.history.epoch())
			{
				return false;
			}
			if cache.queue(self.state.generation, account, operation) {
				self.cache_pending += 1;
				if !self.cache_error {
					self.cache_status = "Saving local changes…";
				}
				return true;
			} else {
				self.cache_error = true;
				self.cache_status = "Local storage queue full; some changes are not saved";
			}
		}
		false
	}
	pub(super) fn request_history_clear(&mut self, account: model::Id) {
		if self.state.demo || self.fixture_only {
			return;
		}
		let Some(cache) = &self.cache else {
			return;
		};
		cache.history.invalidate();
		cache.history.block();
		if !self.cache_clears.request(account) {
			cache.history.fail();
			self.cache_error = true;
			self.cache_status = "Cache cleanup backlog exceeded; history cache disabled until restart; deleted messages may remain on disk";
			return;
		}
		if !self.cache_error {
			self.cache_status =
				"Waiting to clear cached history; cached history temporarily disabled";
		}
		self.retry_history_clears();
	}
	pub(super) fn retry_history_clears(&mut self) {
		let Some(cache) = &self.cache else {
			return;
		};
		for _ in 0..16 {
			let Some(account) = self.cache_clears.next() else {
				break;
			};
			if !cache.queue(
				self.state.generation,
				account,
				cache::Operation::ClearHistory,
			) {
				break;
			}
			self.cache_clears.queued(account);
			self.cache_pending += 1;
		}
	}
	pub(super) fn delete_cached_messages(&mut self, event: &Event) {
		if self.state.demo || self.fixture_only {
			return;
		}
		let Some(account) = self.state.user.as_ref().map(|user| user.id) else {
			return;
		};
		let (channel, ids) = match event {
			Event::Delete { channel, id } => (*channel, vec![*id]),
			Event::DeleteBulk { channel, ids } if !ids.is_empty() && ids.len() <= 100 => {
				(*channel, ids.clone())
			}
			Event::DeleteBulk { ids, .. } if ids.len() > 100 => {
				self.request_history_clear(account);
				return;
			}
			_ => return,
		};
		self.delete_cached_ids(channel, ids);
	}
	pub(super) fn delete_cached_ids(&mut self, channel: model::Id, ids: Vec<model::Id>) {
		if self.state.demo || self.fixture_only || ids.is_empty() {
			return;
		}
		let Some(account) = self.state.user.as_ref().map(|user| user.id) else {
			return;
		};
		let Some(cache) = &self.cache else {
			return;
		};
		if cache.delete_messages(self.state.generation, account, channel, ids) {
			self.cache_pending += 1;
		} else {
			self.request_history_clear(account);
		}
	}
}
