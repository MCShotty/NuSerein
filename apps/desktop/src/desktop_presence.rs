//! Reconcile account presence and local activity.
#[cfg(feature = "demo")]
use super::game_activity;
use super::{Desktop, cache, egui};

impl Desktop {
	pub(super) fn presence_snapshot(&self) -> model::OwnPresence {
		model::OwnPresence {
			expires_at_ms: self.messaging.own_presence_expires,
			..self.messaging.own_presence.clone()
		}
	}
	pub(super) fn adopt_remote_presence(&mut self, apply: bool) {
		let Some(connection) = &mut self.connection else {
			return;
		};
		if !connection.account_presence.has_changed().unwrap_or(false) {
			return;
		}
		let remote = connection.account_presence.borrow_and_update().clone();
		let Some(remote) = remote else {
			return;
		};
		if !apply {
			return;
		}
		self.messaging.adopt_account_presence(remote);
		self.presence_authoritative = true;
	}
	pub(super) fn persist_account_presence(&mut self) {
		if !self.presence_authoritative || self.state.demo || self.fixture_only {
			return;
		}
		let Some(account) = self.state.user.as_ref().map(|user| user.id) else {
			return;
		};
		let snapshot = self.presence_snapshot();
		if !snapshot.valid() || self.presence_saved.as_ref() == Some(&(account, snapshot.clone())) {
			return;
		}
		if self.queue_cache(cache::Operation::SaveAccountPresence(snapshot.clone())) {
			self.account_presences.insert(account, snapshot.clone());
			self.presence_saved = Some((account, snapshot));
		}
	}
	pub(super) fn flush_deferred_connect(&mut self, ctx: &egui::Context) {
		if self.presence_load_pending {
			return;
		}
		if let Some((secret, save)) = self.deferred_connect.take() {
			self.connect(secret, save, ctx);
		}
	}
	pub(super) fn sync_own_presence(&mut self, ctx: &egui::Context) {
		self.expire_own_status(ctx);
		let changed = std::mem::take(&mut self.messaging.own_presence_changed);
		self.adopt_remote_presence(!changed);
		if changed {
			self.presence_authoritative = true;
		}
		let published = self.presence_snapshot();
		let previous_status = self.messaging.own_presence_status;
		self.messaging.own_presence_status = if !published.valid() {
			"Status must be at most 128 characters without line breaks or surrounding spaces."
		} else if self.state.demo || self.fixture_only {
			"Offline preview: not shared or saved."
		} else if let Some(connection) = &self.connection {
			let save_error = *connection.presence_error.borrow();
			let rejected = connection.own_presence.is_closed()
				|| (changed && connection.own_presence.send(published.clone()).is_err());
			if changed && !rejected {
				connection.presence_edits.send_replace(Some(published));
			}
			if rejected {
				"Could not update status: connection unavailable."
			} else if !self.state.gateway_connected {
				"Waiting for connection."
			} else {
				save_error.unwrap_or("")
			}
		} else {
			"Not connected; status is not shared."
		};
		self.persist_account_presence();
		if changed || previous_status != self.messaging.own_presence_status {
			ctx.request_repaint();
		}
	}
	/// A status with a "Clear after" deadline clears itself, then republishes like any edit.
	pub(super) fn expire_own_status(&mut self, ctx: &egui::Context) {
		let Some(expires) = self.messaging.own_presence_expires else {
			return;
		};
		if self.messaging.own_presence.custom_status.is_empty() {
			self.messaging.own_presence_expires = None;
			return;
		}
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_millis()
			.min(u128::from(u64::MAX)) as u64;
		if now < expires {
			// Wake once at the deadline; egui otherwise sleeps through it on an idle window.
			ctx.request_repaint_after(std::time::Duration::from_millis(
				(expires - now).min(60_000),
			));
			return;
		}
		self.messaging.own_presence_expires = None;
		self.messaging.own_presence.expires_at_ms = None;
		self.messaging.own_presence.custom_status.clear();
		self.messaging.own_presence_changed = true;
		self.messaging
			.toasts
			.push(ui::design::Level::Info, "Custom status cleared");
	}
	pub(super) fn sync_game_activity(&mut self, ctx: &egui::Context) {
		let previous_sharing = (
			self.messaging.discord_activity_sharing,
			self.messaging.discord_activity_sharing_busy,
			self.messaging.discord_activity_sharing_retry,
		);
		let previous = (
			self.messaging.own_game.clone(),
			self.messaging.game_activity_status,
		);
		#[cfg(feature = "demo")]
		if self.state.demo {
			let activity = self
				.messaging
				.share_game_activity
				.then(game_activity::demo_activity);
			self.messaging.own_game = activity.as_ref().map(model::RichActivity::summary);
			self.messaging.running_game = self
				.messaging
				.share_game_activity
				.then(|| {
					let renamed = self
						.messaging
						.registered_games
						.iter()
						.find(|game| game.executable == "osu!.exe");
					model::registered_games::RunningGame {
						executable: "osu!.exe".into(),
						name: renamed.map_or_else(|| "osu!".into(), |game| game.name.clone()),
						application: Some(model::Id(367_827_983_903_490_050)),
						renamed: renamed.is_some(),
					}
				})
				.filter(|_| {
					!self
						.messaging
						.registered_games
						.iter()
						.any(|game| game.executable == "osu!.exe" && game.hidden)
				});
			let changed = self.state.set_local_game_activity(activity);
			self.messaging.game_activity_status = "settings-activity-status-offline-preview";
			if changed
				|| previous
					!= (
						self.messaging.own_game.clone(),
						self.messaging.game_activity_status,
					) {
				ctx.request_repaint();
			}
			return;
		}
		if self.fixture_only {
			return;
		}
		self.registered_games.sync(&mut self.messaging, ctx);
		self.messaging.running_game = None;
		self.game_activity
			.observe(self.messaging.share_game_activity);
		if self.game_activity.dirty && !self.game_activity.saving {
			let accepted = self.cache.as_ref().is_some_and(|cache| {
				cache.queue(
					self.state.generation,
					model::Id(0),
					cache::Operation::SaveGameActivity(self.game_activity.enabled),
				)
			});
			self.game_activity.dirty = false;
			self.game_activity.saving = accepted;
			self.game_activity.failed = !accepted;
			self.cache_pending += usize::from(accepted);
		}
		self.messaging.own_game = None;
		self.messaging.discord_activity_sharing = None;
		self.messaging.discord_activity_sharing_busy = false;
		self.messaging.discord_activity_sharing_retry = false;
		let sharing_request = self.messaging.discord_activity_sharing_request.take();
		let mut own_activity = None;
		self.messaging.game_activity_status = self.game_activity.status();
		if let Some(connection) = &self.connection {
			let custom_changed = self.extensions.take_rich_presence_change();
			let custom = self.extensions.rich_presence();
			connection.custom_rich_presence.send_if_modified(|current| {
				if !custom_changed && current.as_ref() == custom {
					return false;
				}
				*current = custom.cloned();
				true
			});
			let registered = self.registered_games.games();
			connection.registered_games.send_if_modified(|current| {
				if current.as_slice() == registered {
					return false;
				}
				*current = registered.to_vec();
				true
			});
			if self.game_activity.enabled && self.state.gateway_connected {
				self.messaging.running_game = connection.running_game.borrow().clone();
			}
			connection.share_activity.send_if_modified(|enabled| {
				if *enabled == self.game_activity.enabled {
					return false;
				}
				*enabled = self.game_activity.enabled;
				true
			});
			// A local client may ask for an invite once; the dialog then waits for the user.
			if let Some((count, code)) = connection.rpc_invite.borrow().clone()
				&& self.rpc_invite_seen < count
			{
				self.rpc_invite_seen = count;
				self.messaging
					.open_rpc_invite(self.state.generation, code.clone());
			}
			if self.game_activity.enabled && self.state.gateway_connected {
				match &*connection.game_activity.borrow() {
					Ok(game) => {
						self.messaging.own_game = game.as_ref().map(model::RichActivity::summary);
						own_activity = game.clone();
						if game.is_some() && !self.game_activity.needs_attention() {
							use discord_gateway::ActivityObservation as Observation;
							self.messaging.game_activity_status = match *connection
								.activity_observation
								.borrow()
							{
								Observation::Unconfirmed => "settings-activity-status-unconfirmed",
								Observation::ServerReceived => "settings-activity-status-received",
								Observation::ServerListed => "settings-activity-status-listed",
								Observation::ServerHidden => {
									self.messaging.discord_activity_sharing_retry = true;
									"settings-activity-status-hidden"
								}
								Observation::ServerMissing => "settings-activity-status-missing",
							};
						}
					}
					Err(error) => self.messaging.game_activity_status = error,
				}
			}
			if self.game_activity.enabled && self.state.gateway_connected {
				match *connection.activity_sharing.borrow() {
					Ok(value) => {
						self.messaging.discord_activity_sharing = value;
						self.messaging.discord_activity_sharing_busy = value.is_none();
						if !self.game_activity.needs_attention() {
							match value {
								Some(false) => {
									self.messaging.game_activity_status =
										"settings-activity-status-sharing-off"
								}
								None => {
									self.messaging.game_activity_status =
										"settings-activity-status-checking"
								}
								Some(true) => {}
							}
						}
					}
					Err(_) => {
						self.messaging.discord_activity_sharing_retry = true;
						if !self.game_activity.needs_attention() {
							self.messaging.game_activity_status =
								"settings-activity-status-check-failed";
						}
					}
				}
				if let Some(enable) = sharing_request {
					if connection.activity_sharing_request.try_send(enable).is_ok() {
						self.messaging.discord_activity_sharing_busy = true;
						self.messaging.game_activity_status = "settings-activity-status-updating";
					} else {
						self.messaging.discord_activity_sharing_retry = true;
						self.messaging.game_activity_status =
							"settings-activity-status-request-failed";
					}
				}
			}
		}
		// Detected games join Added Games so a wrong detection can be hidden later.
		let now_ms = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_millis()
			.min(u128::from(u64::MAX)) as u64;
		self.registered_games.record(&mut self.messaging, now_ms);
		// Keep the existing game preview; Spotify fills the activity card while no game is active.
		if own_activity.is_none() && self.state.gateway_connected {
			own_activity = self.connection.as_ref().and_then(|connection| {
				connection
					.spotify_activity
					.borrow()
					.as_ref()
					.map(|activity| activity.display())
			});
		}
		let changed = self.state.set_local_game_activity(own_activity);
		if changed
			|| previous_sharing
				!= (
					self.messaging.discord_activity_sharing,
					self.messaging.discord_activity_sharing_busy,
					self.messaging.discord_activity_sharing_retry,
				) || previous
			!= (
				self.messaging.own_game.clone(),
				self.messaging.game_activity_status,
			) {
			ctx.request_repaint();
		}
	}
}
