//! Serialized account presence and activity reconciliation.
use super::{Arc, DiscordApi, Duration, Failure, egui, mpsc, watch};

pub(super) async fn resolve_account_presence(
	api: &DiscordApi,
	presence: &watch::Sender<model::OwnPresence>,
	cached: Option<model::OwnPresence>,
) -> Option<model::OwnPresence> {
	let baseline = presence.borrow().clone();
	let remote = tokio::time::timeout(Duration::from_secs(8), api.account_presence())
		.await
		.ok()
		.and_then(Result::ok);
	let edited = presence.borrow().clone() != baseline;
	if !edited && (remote.is_some() || cached.is_some()) {
		let chosen = remote.clone().or(cached.clone()).unwrap_or_default();
		let _ = presence.send_if_modified(|slot| {
			if *slot == baseline {
				*slot = chosen;
				true
			} else {
				false
			}
		});
	}
	let current = presence.borrow().clone();
	(remote.is_some() || cached.is_some() || current != baseline).then_some(current)
}

pub(super) struct PresenceSync {
	pub(super) api: Arc<DiscordApi>,
	pub(super) edits: watch::Receiver<Option<model::OwnPresence>>,
	pub(super) remote_changed: Arc<tokio::sync::Notify>,
	pub(super) presence: watch::Sender<model::OwnPresence>,
	pub(super) account: Arc<watch::Sender<Option<model::OwnPresence>>>,
	pub(super) note: watch::Sender<Option<&'static str>>,
	pub(super) finished: watch::Sender<Option<Failure>>,
	pub(super) wake: egui::Context,
}

/// Saves local status edits and adopts status changed on other devices, one request at a
/// time so a remote echo never races a newer local edit. Unsaved edits retry with backoff.
pub(super) async fn run_presence_sync(sync: PresenceSync) {
	const RETRY: [u64; 5] = [2, 5, 15, 30, 60];
	let PresenceSync {
		api,
		mut edits,
		remote_changed,
		presence,
		account,
		note,
		finished,
		wake,
	} = sync;
	let mut pending: Option<model::OwnPresence> = None;
	let mut failures = 0;
	let stop = |failure: Failure| {
		api.stop();
		let _ = finished.send(Some(failure));
		wake.request_repaint();
	};
	loop {
		let retry = pending
			.as_ref()
			.map(|_| Duration::from_secs(RETRY[failures.min(RETRY.len() - 1)]));
		let mut refresh = false;
		tokio::select! {
			changed = edits.changed() => {
				if changed.is_err() {
					return;
				}
				if let Some(next) = edits.borrow_and_update().clone().filter(model::OwnPresence::valid) {
					// Always write: the account API compares against a fresh read, while a
					// remembered "last saved" value goes stale once another device edits.
					pending = Some(next);
					failures = 0;
				}
			}
			() = remote_changed.notified() => refresh = pending.is_none(),
			() = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {}
		}
		if let Some(next) = pending.clone() {
			match api.set_account_presence(&next).await {
				Ok(()) => {
					pending = None;
					failures = 0;
					if note.send_replace(None).is_some() {
						wake.request_repaint();
					}
				}
				Err(failure) if failure.ends_session() => return stop(failure),
				Err(_) => {
					failures += 1;
					if failures >= RETRY.len() {
						// Give up until the next edit or a settings change from Discord.
						pending = None;
					}
					let _ = note.send_replace(Some(
						"Could not save status to Discord. Retrying; it stays on this device until Discord accepts it.",
					));
					wake.request_repaint();
				}
			}
			continue;
		}
		if !refresh {
			continue;
		}
		let remote =
			match tokio::time::timeout(Duration::from_secs(8), api.account_presence()).await {
				Ok(Ok(remote)) if remote.valid() => remote,
				Ok(Err(failure)) if failure.ends_session() => return stop(failure),
				// Keep the current status; the next change or reconnect reads again.
				_ => continue,
			};
		// An edit made while reading is newer than what Discord returned.
		if edits.has_changed().unwrap_or(true) {
			continue;
		}
		let modified = presence.send_if_modified(|slot| {
			let modified = *slot != remote;
			if modified {
				slot.clone_from(&remote);
			}
			modified
		});
		let adopted = account.send_if_modified(|slot| {
			let modified = slot.as_ref() != Some(&remote);
			if modified {
				*slot = Some(remote);
			}
			modified
		});
		// Discord's value replaces any edit that was given up on.
		let cleared = note.send_replace(None).is_some();
		if modified || adopted || cleared {
			wake.request_repaint();
		}
	}
}

pub(super) async fn run_activity_sharing(
	api: Arc<DiscordApi>,
	mut enabled: watch::Receiver<bool>,
	mut requests: mpsc::Receiver<bool>,
	report: watch::Sender<Result<Option<bool>, Failure>>,
	finished: watch::Sender<Option<Failure>>,
	wake: egui::Context,
) {
	let mut refresh = true;
	let mut request = None;
	loop {
		refresh |= enabled.has_changed().unwrap_or(true);
		let current = *enabled.borrow_and_update();
		if refresh {
			refresh = false;
			// An action queued before disabling sharing must not enable it on a later cycle.
			let _ = requests.try_recv();
			request = current.then_some(false);
			let _ = report.send_replace(Ok(None));
			wake.request_repaint();
		}
		if let Some(enable) = request.take() {
			let _ = report.send_replace(Ok(None));
			wake.request_repaint();
			let operation = async {
				if enable {
					api.set_activity_sharing(true).await
				} else {
					api.activity_sharing().await
				}
			};
			let result = tokio::select! {
				biased;
				changed = enabled.changed() => {
					if changed.is_err() { return; }
					refresh = true;
					continue;
				}
				result = operation => result,
			};
			let _ = report.send_replace(result.map(Some));
			wake.request_repaint();
			if let Err(failure) = result
				&& failure.ends_session()
			{
				api.stop();
				let _ = finished.send(Some(failure));
				wake.request_repaint();
				return;
			}
		}
		tokio::select! {
			biased;
			changed = enabled.changed() => {
				if changed.is_err() { return; }
				refresh = true;
			},
			next = requests.recv() => match next {
				Some(next) if *enabled.borrow() => request = Some(next),
				Some(_) => {},
				None => return,
			}
		}
	}
}
