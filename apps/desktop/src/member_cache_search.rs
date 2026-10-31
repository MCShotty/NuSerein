//! A small local hint lookup runs alongside, and never completes, a live member query.
use super::{Desktop, cache};
use client_core::{auth::AuthState, member_search::Request};
use model::Id;

#[derive(Default)]
pub(super) struct Search {
	scope: Option<(u64, Id)>,
	pending: [Option<Request>; 2],
	loads: [cache::HistoryLoad; 2],
}

impl Desktop {
	pub(super) fn queue_member_cache_search(&mut self, request: &Request) {
		if !self.state.demo && !self.fixture_only && request.valid() {
			self.member_cache_search.pending[request.slot] = Some(request.clone());
			self.poll_member_cache_search();
		}
	}

	pub(super) fn poll_member_cache_search(&mut self) {
		let Some(account) = self.state.user.as_ref().map(|user| user.id) else {
			self.member_cache_search = Search::default();
			return;
		};
		if self.state.demo || self.fixture_only || self.state.auth != AuthState::Authenticated {
			return;
		}
		let scope = (self.state.generation, account);
		if self.member_cache_search.scope != Some(scope) {
			// A command can be the first request of a new account. Keep only requests
			// that exactly match the newly authenticated state's current slots.
			let pending = std::mem::take(&mut self.member_cache_search.pending);
			self.member_cache_search = Search {
				scope: Some(scope),
				pending,
				..Default::default()
			};
		}
		let Some(cache) = &self.cache else {
			return;
		};
		let epoch = cache.history.epoch();
		if !cache.history.allows(epoch) {
			return;
		}
		for slot in 0..2 {
			let Some(request) = self.member_cache_search.pending[slot].as_ref().cloned() else {
				continue;
			};
			if !request.valid()
				|| self.state.selected != Some(request.channel)
				|| !self.state.can_view(request.channel)
				|| self.state.member_search[slot].request.as_ref() != Some(&request)
			{
				self.member_cache_search.pending[slot] = None;
				continue;
			}
			let key = (scope.0, request.channel, request.nonce, epoch);
			if self.member_cache_search.loads[slot].attempt(key) {
				let revision = self.state.member_display_revision();
				if self.queue_cache(cache::Operation::FindMemberDisplays { request, revision }) {
					self.member_cache_search.loads[slot].accepted(key);
					self.member_cache_search.pending[slot] = None;
				}
			}
		}
	}

	pub(super) fn member_cache_search_capacity_changed(&mut self) {
		for load in &mut self.member_cache_search.loads {
			load.capacity_changed();
		}
	}

	pub(super) fn accept_member_cache_search(
		&mut self,
		generation: u64,
		request: Request,
		revision: u64,
		epoch: u64,
		result: Result<Vec<model::MemberDisplay>, local_store::StoreError>,
	) {
		if generation == self.state.generation
			&& self
				.cache
				.as_ref()
				.is_some_and(|cache| cache.history.allows(epoch))
			&& let Ok(rows) = result
		{
			self.state
				.hydrate_member_search_displays(&request, revision, rows);
		}
	}
}
