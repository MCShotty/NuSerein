mod events;
mod lanes;
mod presence;
mod session;
mod voice;
use client_core::{
	COMMAND_SLOTS, Command, EVENT_SLOTS, Envelope, Event, MAX_EVENT_BYTES,
	auth::{Failure, SessionSecret},
	server_admin::{ReadKey, ReadScopes},
};
use discord_api::DiscordApi;
use eframe::egui;
pub use events::ReliableEvents;
use events::*;
use lanes::*;
use presence::*;
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicU64, Ordering},
	},
	time::{Duration, Instant},
};
use tokio::{
	runtime::Handle,
	sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
	task::JoinHandle,
};
use voice::*;

pub struct Connection {
	pub commands: mpsc::Sender<Command>,
	admin_scopes: watch::Sender<ReadScopes>,
	preference_scope: watch::Sender<Option<(u64, model::Id, u64)>>,
	pub uploads: mpsc::Sender<crate::uploads::UploadRequest>,
	pub events: ReliableEvents,
	pub typing: mpsc::Receiver<Envelope>,
	pub terminal: watch::Receiver<Option<Failure>>,
	/// One fixed-size candidate failure, independent of the account event queue.
	pub confirmation_failure: watch::Receiver<Option<ConfirmationFailure>>,
	pub share_activity: watch::Sender<bool>,
	pub custom_rich_presence: watch::Sender<Option<extensions::CustomRichPresence>>,
	pub own_presence: watch::Sender<model::OwnPresence>,
	/// Local edits only. Seeding from Discord does not publish through this watch.
	pub presence_edits: watch::Sender<Option<model::OwnPresence>>,
	/// The status chosen for this connection, once Discord or the local fallback is known.
	pub account_presence: watch::Receiver<Option<model::OwnPresence>>,
	pub presence_error: watch::Receiver<Option<&'static str>>,
	pub game_activity: watch::Receiver<crate::game_activity::Detection>,
	pub registered_games: watch::Sender<Vec<model::registered_games::RegisteredGame>>,
	pub running_game: watch::Receiver<Option<model::registered_games::RunningGame>>,
	pub spotify_activity: watch::Receiver<Option<discord_protocol::spotify::Activity>>,
	/// A local Rich Presence client asked the client to show an invite: counter and code.
	pub rpc_invite: watch::Receiver<Option<(u64, String)>>,
	pub activity_observation: watch::Receiver<discord_gateway::ActivityObservation>,
	pub activity_sharing: watch::Receiver<Result<Option<bool>, Failure>>,
	pub activity_sharing_request: mpsc::Sender<bool>,
	typing_channel: Arc<AtomicU64>,
	reconnect: Arc<tokio::sync::Notify>,
	diagnostics: Arc<Mutex<discord_gateway::ConnectionDiagnostics>>,
	diagnostic_revision: Arc<AtomicU64>,
	diagnostic_copied: std::cell::Cell<u64>,
	foreground: std::cell::Cell<ForegroundRecovery>,
	send_recovery_pending: std::cell::Cell<bool>,
	task: JoinHandle<()>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmationFailure {
	generation: u64,
	channel: model::Id,
	request: u64,
	revision: u64,
	message: &'static str,
}
impl ConfirmationFailure {
	fn envelope(self) -> Envelope {
		Envelope {
			generation: self.generation,
			event: Event::Voice(client_core::voice::Event::SessionConfirmationFailed {
				channel: self.channel,
				request: self.request,
				revision: self.revision,
				message: self.message,
			}),
		}
	}
}
pub fn take_confirmation_failure(
	receiver: &mut watch::Receiver<Option<ConfirmationFailure>>,
	events: &ReliableEvents,
) -> Option<Envelope> {
	// A replacement candidate/ACK may still be behind this frame's reliable batch.
	// Keep the report unseen until preceding signaling is consumed; the original
	// negotiation deadline still bounds failure if sustained events never drain.
	if !events.receive.is_empty() {
		return None;
	}
	// Ref::has_changed preserves an unread final version after publisher shutdown.
	// Release its read lock before handing the report to the event consumer.
	let failure = {
		let report = receiver.borrow_and_update();
		report.has_changed().then_some(*report).flatten()
	};
	failure.map(ConfirmationFailure::envelope)
}
impl Drop for Connection {
	fn drop(&mut self) {
		self.task.abort();
	}
}
/// Preload tasks kept at once: one per preload kind, plus room for one whose result was lost.
const PREFETCHES: usize = 4;
struct AbortTask(JoinHandle<()>);

impl Drop for AbortTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

impl Connection {
	/// A suspended UI must not replace a healthy Gateway merely because focus changed.
	pub fn observe_foreground(&self, focused: bool, now: Instant) {
		let mut foreground = self.foreground.get();
		if foreground.observe(focused, now) {
			self.reconnect();
		}
		self.foreground.set(foreground);
	}
	pub fn copy_diagnostics_to(&self, messaging: &mut ui::MessagingUi) {
		let revision = self.diagnostic_revision.load(Ordering::Acquire);
		if revision == self.diagnostic_copied.get() {
			return;
		}
		if let Ok(diagnostics) = self.diagnostics.lock() {
			messaging.connection_diagnostics = diagnostics.summary();
			messaging.connection_reason = diagnostics.last_reason();
			self.diagnostic_copied.set(revision);
		}
	}
	/// Coalesce explicit recovery requests without restarting REST writes or authentication.
	pub fn reconnect(&self) {
		self.reconnect.notify_one();
	}
	/// Automatic send recovery wakes one attempt per outage, so rapid sends cannot
	/// repeatedly abandon a dial already in progress. Manual recovery remains explicit.
	pub fn recover_send(&self) {
		if !self.send_recovery_pending.replace(true) {
			self.reconnect();
		}
	}
	pub fn gateway_recovered(&self) {
		self.send_recovery_pending.set(false);
	}
	pub fn set_typing_channel(&self, channel: Option<model::Id>) {
		self.typing_channel
			.store(channel.map_or(0, |id| id.0), Ordering::Relaxed);
	}
	pub fn sync_admin_reads(&self, state: &client_core::State) {
		let preference = state.channel_preference_scope();
		self.preference_scope.send_if_modified(|current| {
			if *current == preference {
				false
			} else {
				*current = preference;
				true
			}
		});
		let latest = state.admin_read_scopes();
		self.admin_scopes.send_if_modified(|current| {
			if *current == latest {
				false
			} else {
				*current = latest;
				true
			}
		});
	}
}

#[derive(Clone, Copy)]
struct ForegroundRecovery {
	last_frame: Instant,
	last_wall_frame: std::time::SystemTime,
	interrupted: bool,
}
impl ForegroundRecovery {
	fn new(now: Instant) -> Self {
		Self {
			last_frame: now,
			last_wall_frame: std::time::SystemTime::now(),
			interrupted: false,
		}
	}
	fn observe(&mut self, focused: bool, now: Instant) -> bool {
		self.observe_at(focused, now, std::time::SystemTime::now())
	}
	fn observe_at(&mut self, focused: bool, now: Instant, wall_now: std::time::SystemTime) -> bool {
		// Some monotonic clocks pause during system sleep. Wall time is used only to
		// request an advisory probe; actual transport deadlines remain monotonic.
		self.interrupted |= now.saturating_duration_since(self.last_frame)
			>= Duration::from_secs(30)
			|| wall_now
				.duration_since(self.last_wall_frame)
				.is_ok_and(|gap| gap >= Duration::from_secs(30));
		self.last_frame = now;
		self.last_wall_frame = wall_now;
		focused && std::mem::take(&mut self.interrupted)
	}
}

fn stream_preview_session_failure(event: &Event) -> Option<Failure> {
	match event {
		// An oversized optional still must not disconnect the account.
		Event::StreamPreview { result: Err(f), .. }
			if f.ends_session() && *f != Failure::Capacity =>
		{
			Some(*f)
		}
		_ => None,
	}
}

// Reads can finish after navigation/cancellation. Only a session-ending failure is global.
fn scope_history_failure(event: Event, channel: model::Id, request: u64) -> Event {
	let failure = match event {
		Event::Failure(failure) if !failure.ends_session() => failure,
		Event::Unavailable(_) => Failure::Forbidden,
		event => return event,
	};
	Event::HistoryFailed {
		channel,
		request,
		failure,
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn sleep_is_detected_when_the_monotonic_clock_pauses_without_using_wall_time_for_deadlines() {
		let mono = Instant::now();
		let wall = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
		let mut recovery = ForegroundRecovery {
			last_frame: mono,
			last_wall_frame: wall,
			interrupted: false,
		};
		assert!(recovery.observe_at(
			true,
			mono + Duration::from_millis(1),
			wall + Duration::from_secs(3600)
		));
		assert!(!recovery.observe_at(
			true,
			mono + Duration::from_millis(2),
			wall + Duration::from_secs(3600)
		));
		// Clock corrections may cause one advisory probe; moving backwards never loops.
		assert!(!recovery.observe_at(true, mono + Duration::from_millis(3), wall));
		assert!(!recovery.observe_at(true, mono + Duration::from_millis(4), wall));
	}
	#[test]
	fn foreground_recovery_probes_one_long_gap_and_keeps_ordinary_focus_changes_quiet() {
		let now = Instant::now();
		let mut recovery = ForegroundRecovery::new(now);
		assert!(!recovery.observe(true, now + Duration::from_secs(1)));
		assert!(!recovery.observe(false, now + Duration::from_secs(2)));
		assert!(!recovery.observe(true, now + Duration::from_secs(3)));
		assert!(recovery.observe(true, now + Duration::from_secs(33)));
		assert!(!recovery.observe(true, now + Duration::from_secs(34)));
		assert!(!recovery.observe(false, now + Duration::from_secs(100)));
		assert!(!recovery.observe(false, now + Duration::from_secs(101)));
		assert!(recovery.observe(true, now + Duration::from_secs(102)));
		assert!(!recovery.observe(true, now + Duration::from_secs(103)));
		// A new account gets a fresh observation window, never the previous account's gap.
		let mut next_account = ForegroundRecovery::new(now + Duration::from_secs(200));
		assert!(!next_account.observe(true, now + Duration::from_secs(201)));
	}
	#[tokio::test]
	async fn administrative_and_community_mutations_cannot_overlap() {
		let (release, held) = tokio::sync::oneshot::channel::<()>();
		let mut administrative = Some(AbortTask(tokio::spawn(async move {
			let _ = held.await;
		})));
		let mut community = None;
		assert!(
			admin_mutation_busy(&administrative, &community),
			"an active server mutation rejects a Community write"
		);
		drop(administrative.take());
		drop(release);
		assert!(!admin_mutation_busy(&administrative, &community));
		let (release, held) = tokio::sync::oneshot::channel::<()>();
		community = Some(AbortTask(tokio::spawn(async move {
			let _ = held.await;
		})));
		assert!(
			admin_mutation_busy(&administrative, &community),
			"an active Community write rejects a server mutation"
		);
		drop(community.take());
		drop(release);
		assert!(!admin_mutation_busy(&administrative, &community));
	}
	#[tokio::test]
	async fn verified_resume_handoff_does_not_wait_for_worker_exit() {
		use client_core::mfa::{Action, Request};
		let (published, verified) = tokio::sync::oneshot::channel();
		let (release, held) = tokio::sync::oneshot::channel::<()>();
		let mut job = Some(AbortTask(tokio::spawn(async move {
			published.send(()).unwrap();
			let _ = held.await;
		})));
		verified.await.unwrap();
		assert!(!job.as_ref().unwrap().0.is_finished());
		let resume = Request {
			id: 7,
			action: Action::Resume,
		};
		assert!(!mfa_job_available(&job, &resume, false));
		assert!(
			mfa_job_available(&job, &resume, true),
			"published broker proof permits immediate resume"
		);
		assert!(!mfa_job_available(
			&job,
			&Request {
				id: 7,
				action: Action::SendSms
			},
			true
		));
		assert!(mfa_job_available(
			&job,
			&Request {
				id: 7,
				action: Action::Cancel
			},
			false
		));
		// Production hands the single bounded worker slot over at this point.
		drop(job.take());
		assert!(mfa_job_available(&job, &resume, false));
		drop(release);
	}
	#[tokio::test]
	async fn channel_admin_read_replacement_cancels_held_http_and_leaves_message_work_running() {
		use tokio::{
			io::{AsyncReadExt, AsyncWriteExt},
			net::{TcpListener, TcpStream},
		};
		tokio::time::timeout(Duration::from_secs(5), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let address = listener.local_addr().unwrap();
			let (entered, waiting) = tokio::sync::oneshot::channel();
			let server = tokio::spawn(async move {
				let (mut held, _) = listener.accept().await.unwrap();
				let mut request = [0; 22];
				held.read_exact(&mut request).await.unwrap();
				assert_eq!(&request, b"GET /held HTTP/1.1\r\n\r\n");
				entered.send(()).unwrap();
				assert_eq!(
					held.read(&mut [0]).await.unwrap(),
					0,
					"retired read releases its HTTP stream"
				);
				let (mut latest, _) = listener.accept().await.unwrap();
				let mut request = [0; 24];
				latest.read_exact(&mut request).await.unwrap();
				assert_eq!(&request, b"GET /latest HTTP/1.1\r\n\r\n");
				latest
					.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
					.await
					.unwrap();
			});
			let key = ReadKey {
				generation: 3,
				kind: client_core::server_admin::ReadKind::Channel(model::Id(3)),
				guild: model::Id(2),
				request: 7,
			};
			let latest = ReadKey { request: 8, ..key };
			let (scope, updates) = watch::channel(ReadScopes([Some(key), None, None, None]));
			let old_scope = updates.clone();
			let old = tokio::spawn(scoped_admin_read(
				async move {
					let mut stream = TcpStream::connect(address).await.unwrap();
					stream
						.write_all(b"GET /held HTTP/1.1\r\n\r\n")
						.await
						.unwrap();
					let mut response = Vec::new();
					stream.read_to_end(&mut response).await.unwrap();
					Event::Failure(Failure::Forbidden)
				},
				old_scope,
				key,
			));
			waiting.await.unwrap();
			// The ordinary message worker is independent of the held administration read.
			let (send, mut receive) = mpsc::channel(1);
			let message = tokio::spawn(async move { receive.recv().await.unwrap() });
			send.send(19u64).await.unwrap();
			assert_eq!(message.await.unwrap(), 19);
			assert!(!old.is_finished());
			scope.send_replace(ReadScopes([Some(latest), None, None, None]));
			assert!(old.await.unwrap().is_none());
			let event = scoped_admin_read(
				async move {
					let mut stream = TcpStream::connect(address).await.unwrap();
					stream
						.write_all(b"GET /latest HTTP/1.1\r\n\r\n")
						.await
						.unwrap();
					let mut response = Vec::new();
					stream.read_to_end(&mut response).await.unwrap();
					assert!(response.starts_with(b"HTTP/1.1 200 OK"));
					Event::Failure(Failure::Forbidden)
				},
				updates,
				latest,
			)
			.await;
			assert!(matches!(event, Some(Event::Failure(Failure::Forbidden))));
			server.await.unwrap();
		})
		.await
		.unwrap();
	}

	#[tokio::test]
	async fn administration_read_slots_are_bounded_and_cannot_cancel_a_mutation() {
		let key = ReadKey {
			generation: 1,
			kind: client_core::server_admin::ReadKind::Panel,
			guild: model::Id(2),
			request: 1,
		};
		let settings = ReadKey {
			kind: client_core::server_admin::ReadKind::Settings,
			..key
		};
		let third = ReadKey {
			kind: client_core::server_admin::ReadKind::Channel(model::Id(3)),
			..key
		};
		let mut reads = AdminReads::default();
		for current in [key, settings] {
			assert!(reads.replace(current));
			reads.track(current, tokio::spawn(std::future::pending()));
		}
		assert!(!reads.replace(third));
		assert!(!reads.replace(ReadKey {
			kind: client_core::server_admin::ReadKind::Community,
			..key
		}));
		let latest = ReadKey { request: 2, ..key };
		assert!(reads.replace(latest));
		reads.track(latest, tokio::spawn(std::future::pending()));
		assert_eq!(reads.slots.iter().flatten().count(), 2);
		let (finish, wait) = tokio::sync::oneshot::channel();
		let mutation = tokio::spawn(async move { wait.await.unwrap() });
		reads.retire(ReadScopes::default());
		assert!(reads.slots.iter().all(Option::is_none));
		assert!(!mutation.is_finished());
		finish.send(11u64).unwrap();
		assert_eq!(mutation.await.unwrap(), 11);
	}

	#[tokio::test]
	async fn soundboard_scope_cancels_unpolled_and_inflight_operations() {
		use std::sync::{
			Arc,
			atomic::{AtomicBool, Ordering},
		};
		let (owner, updates) = watch::channel(None);
		let (lifecycle, epochs) = watch::channel(VoiceLifecycle::default());
		let contacted = Arc::new(AtomicBool::new(false));
		let started = contacted.clone();
		let result = scoped_soundboard_event(
			async move {
				started.store(true, Ordering::SeqCst);
				Event::Failure(Failure::Forbidden)
			},
			updates.clone(),
			(model::Id(2), 7),
			epochs.clone(),
			0,
		)
		.await;
		assert!(result.is_none() && !contacted.load(Ordering::SeqCst));
		owner.send_replace(Some((model::Id(2), 7)));
		lifecycle.send_modify(|state| {
			state.observe(&Event::Disconnected);
			state.observe(&Event::Resumed);
		});
		let started = contacted.clone();
		assert!(
			scoped_soundboard_event(
				async move {
					started.store(true, Ordering::SeqCst);
					Event::Failure(Failure::Forbidden)
				},
				updates.clone(),
				(model::Id(2), 7),
				epochs.clone(),
				0
			)
			.await
			.is_none()
		);
		assert!(
			!contacted.load(Ordering::SeqCst),
			"a fast resume must not revive an old operation"
		);
		let revision = lifecycle.borrow().interruption;
		struct Released(Arc<AtomicBool>);
		impl Drop for Released {
			fn drop(&mut self) {
				self.0.store(true, Ordering::SeqCst);
			}
		}
		let released = Arc::new(AtomicBool::new(false));
		let signal = released.clone();
		let (entered, waiting) = tokio::sync::oneshot::channel();
		let task = tokio::spawn(scoped_soundboard_event(
			async move {
				let _resource = Released(signal);
				let _ = entered.send(());
				std::future::pending::<Event>().await
			},
			updates,
			(model::Id(2), 7),
			epochs,
			revision,
		));
		tokio::time::timeout(Duration::from_secs(1), waiting)
			.await
			.unwrap()
			.unwrap();
		owner.send_replace(Some((model::Id(2), 8)));
		assert!(
			tokio::time::timeout(Duration::from_secs(1), task)
				.await
				.unwrap()
				.unwrap()
				.is_none()
		);
		assert!(
			released.load(Ordering::SeqCst),
			"cancelled operations must release their resources"
		);
	}

	#[tokio::test]
	async fn inbox_cancellation_retires_reads_but_preserves_accepted_writes() {
		let mut tasks = InboxTasks::default();
		let (read, mut read_cancelled) = tokio::sync::oneshot::channel::<()>();
		tasks.track(
			false,
			tokio::spawn(async move {
				let _read = read;
				std::future::pending::<()>().await;
			}),
		);
		let (accepted, wait) = tokio::sync::oneshot::channel();
		let (result, received) = tokio::sync::oneshot::channel();
		tasks.track(
			true,
			tokio::spawn(async move {
				wait.await.unwrap();
				let _ = result.send(Event::InboxAction {
					request: 7,
					result: Ok(client_core::inbox::Outcome::Done),
				});
			}),
		);
		assert!(!tasks.can_start(true));
		tasks.cancel_read();
		tokio::time::timeout(Duration::from_secs(1), &mut read_cancelled)
			.await
			.unwrap()
			.unwrap_err();
		accepted.send(()).unwrap();
		assert!(matches!(
			tokio::time::timeout(Duration::from_secs(1), received)
				.await
				.unwrap()
				.unwrap(),
			Event::InboxAction {
				request: 7,
				result: Ok(_)
			}
		));
		(&mut tasks.write.as_mut().unwrap().0).await.unwrap();
		assert!(tasks.can_start(true));
	}
	#[tokio::test]
	async fn full_prefetch_pool_rejects_new_reads_without_aborting_accepted_work() {
		let mut tasks: Vec<_> = (0..PREFETCHES)
			.map(|_| AbortTask(tokio::spawn(std::future::pending())))
			.collect();
		let history = Command::History {
			channel: model::Id(1),
			before: None,
			after: None,
			request: 99,
		};
		assert!(matches!(
			reject_prefetch_at_capacity(&mut tasks, &history),
			Some(Event::HistoryFailed {
				channel: model::Id(1),
				request: 99,
				failure: Failure::Capacity
			})
		));
		let profile = Command::Profile {
			user: model::Id(2),
			guild: None,
			request: 100,
			with_mutuals: true,
			cosmetics: Default::default(),
		};
		assert!(matches!(
			reject_prefetch_at_capacity(&mut tasks, &profile),
			Some(Event::Profile {
				user: model::Id(2),
				request: 100,
				result: Err(Failure::Capacity),
				..
			})
		));
		assert_eq!(tasks.len(), PREFETCHES);
		tokio::task::yield_now().await;
		assert!(tasks.iter().all(|task| !task.0.is_finished()));
		tasks[0].0.abort();
		let _ = (&mut tasks[0].0).await;
		assert!(reject_prefetch_at_capacity(&mut tasks, &history).is_none());
		assert_eq!(tasks.len(), PREFETCHES - 1);
	}

	#[test]
	fn confirmed_dm_ownership_survives_resume_without_repeating_initial_ring() {
		use client_core::voice::Command as V;
		use model::Id;
		let (control, mut calls, channels) = recipient_worker_fixture(false);
		calls
			.observed
			.push(calls.as_ref().map(RecipientCall::snapshot));
		let mut active = Some((Id(2), 7, true));
		let mut lifecycle = VoiceLifecycle::default();
		let mut applied = lifecycle;
		assert_eq!(
			calls.observe_scoped(&Event::Disconnected, Id(1), &channels),
			(true, None)
		);
		assert!(calls.as_ref().unwrap().confirmed);
		assert!(calls.observed.is_empty());
		assert!(lifecycle.observe(&Event::Disconnected));
		assert!(apply_voice_lifecycle(
			&mut applied,
			lifecycle,
			&mut active,
			&calls,
			&channels
		));
		assert_eq!(active, Some((Id(2), 7, true)));
		assert_eq!(
			calls.observe_scoped(&Event::Resumed, Id(1), &channels),
			(false, None)
		);
		assert!(!lifecycle.observe(&Event::Resumed));
		assert!(recipient_action(
			Id(2),
			7,
			Id(3),
			Id(1),
			active,
			channels.get(&Id(2)).map(Vec::as_slice)
		));
		assert!(recipient_write_allowed(control, &calls, &channels));
		assert_eq!(
			ring_action(
				V::Ring {
					channel: Id(2),
					request: 7
				},
				Id(1),
				&mut active,
				true
			),
			Err(())
		);
		assert_eq!(
			ring_action(
				V::Leave {
					channel: Id(2),
					request: 7
				},
				Id(1),
				&mut active,
				true
			),
			Ok(Some((None, true)))
		);
		assert!(active.is_none());
	}

	#[test]
	fn dm_ownership_is_retired_on_fresh_session_departure_and_access_loss() {
		use client_core::voice::Event as V;
		use model::Id;
		let ready = || Event::Ready {
			permissions: model::permissions::Snapshot::default(),
			user: model::User {
				username: None,
				id: Id(1),
				name: "Synthetic".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
			},
			guilds: vec![],
			channels: vec![],
		};
		for event in [
			Event::Resync,
			ready(),
			Event::Voice(V::Departed {
				channel: Id(2),
				request: 7,
			}),
			Event::Unavailable(Id(2)),
			Event::RecipientRemoved {
				channel: Id(2),
				user: Id(1),
			},
		] {
			let (_, mut calls, channels) = recipient_worker_fixture(false);
			let mut active = Some((Id(2), 7, true));
			let (invalidates, released) = calls.observe_scoped(&event, Id(1), &channels);
			assert!(invalidates);
			assert_eq!(released, Some((Id(2), 7)));
			assert!(calls.active.is_none());
			assert!(release_taken_over(&mut active, released));
			assert!(active.is_none());
			let mut lifecycle = VoiceLifecycle::default();
			let mut applied = lifecycle;
			if lifecycle.observe(&event) {
				active = Some((Id(2), 7, true));
				assert!(apply_voice_lifecycle(
					&mut applied,
					lifecycle,
					&mut active,
					&calls,
					&channels
				));
				assert!(active.is_none());
			}
		}
		let (_, mut calls, channels) = recipient_worker_fixture(false);
		calls.active.as_mut().unwrap().confirmed = false;
		let mut active = Some((Id(2), 7, false));
		let mut lifecycle = VoiceLifecycle::default();
		let mut applied = lifecycle;
		assert_eq!(
			calls.observe_scoped(&Event::Disconnected, Id(1), &channels),
			(true, None)
		);
		assert!(lifecycle.observe(&Event::Disconnected));
		assert!(apply_voice_lifecycle(
			&mut applied,
			lifecycle,
			&mut active,
			&calls,
			&channels
		));
		assert!(active.is_none());
	}

	#[tokio::test]
	async fn ring_http_is_cancelled_even_when_disconnect_and_resume_are_coalesced() {
		let (lifecycle, updates) = watch::channel(VoiceLifecycle::default());
		let (online, availability) = watch::channel(true);
		let pending = wait_for_voice_interruption(updates.clone(), 0);
		tokio::pin!(pending);
		assert!(
			tokio::time::timeout(Duration::from_millis(1), &mut pending)
				.await
				.is_err()
		);
		lifecycle.send_modify(|state| {
			assert!(state.observe(&Event::Disconnected));
		});
		online.send_replace(false);
		online.send_replace(true);
		lifecycle.send_if_modified(|state| state.observe(&Event::Resumed));
		assert!(
			*availability.borrow(),
			"availability alone lost the interruption"
		);
		tokio::time::timeout(Duration::from_secs(1), pending)
			.await
			.unwrap();
		let contacted = std::cell::Cell::new(false);
		tokio::select! {
			biased;
			_=wait_for_voice_interruption(updates.clone(),0)=>{},
			_=async {contacted.set(true);std::future::pending::<()>().await}=>unreachable!(),
		}
		assert!(
			!contacted.get(),
			"retired work must not dispatch after Resume"
		);
		assert!(
			tokio::time::timeout(
				Duration::from_millis(1),
				wait_for_voice_interruption(updates, 1)
			)
			.await
			.is_err(),
			"a new explicit action uses the resumed revision"
		);
	}

	#[test]
	fn takeover_rejects_queued_initial_ringing_but_preserves_new_call_ownership() {
		use client_core::voice::Command as V;
		let channel = model::Id(22);
		let mut active = Some((channel, 5, false));
		let (send, receive) = watch::channel(None);
		let _ = send.send_replace(Some((channel, 5)));
		// The command is ready before the changed-watch select branch executes.
		assert!(release_taken_over(&mut active, *receive.borrow()));
		assert_eq!(active, None);
		let old_join = V::Join {
			channel,
			request: 5,
			ring: true,
			mute: false,
			deaf: false,
		};
		assert!(join_taken_over(old_join, *receive.borrow()));
		assert!(
			active.is_none(),
			"rejected duplicate Join cannot repopulate ownership"
		);
		let new_join = V::Join {
			channel,
			request: 6,
			ring: true,
			mute: false,
			deaf: false,
		};
		assert!(!join_taken_over(new_join, *receive.borrow()));
		assert!(!join_taken_over(
			V::Join {
				channel: model::Id(23),
				request: 5,
				ring: true,
				mute: false,
				deaf: false
			},
			*receive.borrow()
		));
		assert_eq!(
			ring_action(
				V::Ring {
					channel,
					request: 5
				},
				model::Id(1),
				&mut active,
				true
			),
			Err(())
		);
		assert_eq!(
			ring_action(
				V::Join {
					channel,
					request: 6,
					ring: true,
					mute: false,
					deaf: false
				},
				model::Id(1),
				&mut active,
				true
			),
			Ok(None)
		);
		assert!(!release_taken_over(&mut active, *receive.borrow()));
		assert_eq!(
			ring_action(
				V::Ring {
					channel,
					request: 6
				},
				model::Id(1),
				&mut active,
				true
			),
			Ok(Some((None, false)))
		);
		assert!(!release_taken_over(&mut active, Some((model::Id(23), 6))));
		assert_eq!(active, Some((channel, 6, true)));
		// Moving from an unringed DM attempt to a guild replaces dispatcher ownership too.
		active = Some((channel, 7, false));
		let guild_channel = model::Id(30);
		assert_eq!(
			ring_action(
				V::Join {
					channel: guild_channel,
					request: 8,
					ring: false,
					mute: false,
					deaf: false
				},
				model::Id(1),
				&mut active,
				false
			),
			Ok(None)
		);
		assert_eq!(active, Some((guild_channel, 8, true)));
		assert_eq!(
			ring_action(
				V::Ring {
					channel,
					request: 7
				},
				model::Id(1),
				&mut active,
				true
			),
			Err(())
		);
		assert!(release_taken_over(&mut active, Some((guild_channel, 8))));
	}
	#[test]
	fn initial_ringing_wait_cancels_only_matching_takeover_or_connection_teardown() {
		let runtime = tokio::runtime::Runtime::new().unwrap();
		runtime.block_on(async {
			let scope = (model::Id(22), 5);
			let (send, receive) = watch::channel(Some((model::Id(22), 4)));
			let task = tokio::spawn(wait_for_takeover(receive.clone(), scope));
			tokio::task::yield_now().await;
			assert!(!task.is_finished());
			let _ = send.send_replace(Some(scope));
			tokio::time::timeout(Duration::from_secs(1), task)
				.await
				.unwrap()
				.unwrap();
			// Match production biased selection with a synthetic HTTP future side effect.
			let contacted = std::cell::Cell::new(false);
			tokio::select! {
				biased;
				_=wait_for_takeover(receive.clone(), scope)=>{},
				_=async {contacted.set(true);std::future::pending::<()>().await}=>unreachable!(),
			}
			assert!(
				!contacted.get(),
				"already invalidated ownership must never poll the HTTP write"
			);
			// A task spawned after the notification also cancels before its HTTP future.
			let task = tokio::spawn(wait_for_takeover(receive.clone(), scope));
			tokio::time::timeout(Duration::from_secs(1), task)
				.await
				.unwrap()
				.unwrap();
			let task = tokio::spawn(wait_for_takeover(receive, (model::Id(22), 6)));
			drop(send);
			tokio::time::timeout(Duration::from_secs(1), task)
				.await
				.unwrap()
				.unwrap();
		});
	}
	#[test]
	fn stream_preview_capacity_is_local_but_auth_failures_end_the_session() {
		for (failure, expected) in [
			(Failure::Capacity, None),
			(Failure::Forbidden, None),
			(Failure::Expired, Some(Failure::Expired)),
			(Failure::Challenged, Some(Failure::Challenged)),
			(Failure::InvalidCredential, Some(Failure::InvalidCredential)),
		] {
			assert_eq!(
				stream_preview_session_failure(&Event::StreamPreview {
					guild: model::Id(1),
					channel: model::Id(2),
					user: model::Id(3),
					request: 4,
					result: Err(failure),
				}),
				expected
			);
		}
	}

	#[tokio::test]
	async fn confirmation_report_waits_for_replacement_signaling_beyond_one_frame_batch() {
		use client_core::voice::Event as E;
		let ctx = egui::Context::default();
		let (reliable, mut events) = reliable_events(ctx.clone());
		let (typing, _) = mpsc::channel(8);
		for id in 1..=EVENT_SLOTS as u64 {
			emit_event(&reliable, &typing, queued_channel(id), &ctx).unwrap();
		}
		for event in [
			E::Server {
				channel: model::Id(20),
				request: 5,
				negotiation_revision: Some(3),
				token: Some(client_core::voice::Secret::new("synthetic-token".into()).unwrap()),
				endpoint: Some("synthetic.discord.media".into()),
			},
			E::SessionConfirmed {
				channel: model::Id(20),
				request: 5,
				revision: 3,
			},
		] {
			emit_event(
				&reliable,
				&typing,
				Envelope {
					generation: 7,
					event: Event::Voice(event),
				},
				&ctx,
			)
			.unwrap();
		}
		let (report, mut failure) = watch::channel(None);
		report_confirmation_failure(
			&report,
			7,
			E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 5,
				revision: 2,
				message: "old candidate queue full",
			},
			Some((model::Id(20), 5, false)),
			&ctx,
		);
		drop(report);
		for _ in 0..EVENT_SLOTS {
			assert!(matches!(
				events.try_recv().unwrap().event,
				Event::ChannelCreated(_)
			));
		}
		assert!(take_confirmation_failure(&mut failure, &events).is_none());
		assert!(failure.borrow().has_changed());
		assert!(matches!(
			events.try_recv().unwrap().event,
			Event::Voice(E::Server {
				negotiation_revision: Some(3),
				..
			})
		));
		assert!(take_confirmation_failure(&mut failure, &events).is_none());
		assert!(failure.borrow().has_changed());
		assert!(matches!(
			events.try_recv().unwrap().event,
			Event::Voice(E::SessionConfirmed { revision: 3, .. })
		));
		let error = take_confirmation_failure(&mut failure, &events).unwrap();
		assert!(matches!(
			error.event,
			Event::Voice(E::SessionConfirmationFailed { revision: 2, .. })
		));
		assert!(take_confirmation_failure(&mut failure, &events).is_none());
	}

	#[tokio::test]
	async fn closed_confirmation_report_is_delivered_once_without_repeating_each_frame() {
		use client_core::voice::Event as E;
		let ctx = egui::Context::default();
		let (report, mut receiver) = watch::channel(None);
		let (_, events) = reliable_events(ctx.clone());
		assert!(take_confirmation_failure(&mut receiver, &events).is_none());
		report_confirmation_failure(
			&report,
			7,
			E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 5,
				revision: 3,
				message: "confirmation queue full",
			},
			Some((model::Id(20), 5, false)),
			&ctx,
		);
		drop(report);
		assert!(receiver.has_changed().is_err());
		let envelope = take_confirmation_failure(&mut receiver, &events)
			.expect("final unseen report survives publisher shutdown");
		assert_eq!(envelope.generation, 7);
		assert!(matches!(
			envelope.event,
			Event::Voice(E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 5,
				revision: 3,
				..
			})
		));
		assert!(take_confirmation_failure(&mut receiver, &events).is_none());
	}

	#[tokio::test]
	async fn confirmation_failure_bypasses_full_account_events_and_keeps_latest_candidate() {
		use client_core::voice::{Command as V, Event as E};
		let ctx = egui::Context::default();
		let (reliable, mut events) = reliable_events(ctx.clone());
		let (typing, _) = mpsc::channel(8);
		for id in 1..=RELIABLE_ITEMS as u64 {
			emit_event(&reliable, &typing, queued_channel(id), &ctx).unwrap();
		}
		// Both the item and byte budgets are exhausted; candidate delivery uses neither.
		let remaining = reliable.bytes.available_permits() as u32;
		let held = reliable
			.bytes
			.clone()
			.try_acquire_many_owned(remaining)
			.unwrap();
		let (controls, mut control_receive) = mpsc::channel(8);
		for _ in 0..8 {
			controls
				.try_send(V::Sync {
					channel: model::Id(20),
				})
				.unwrap();
		}
		let owner = Some((model::Id(20), 5, false));
		let control = V::ConfirmSession {
			channel: model::Id(20),
			request: 5,
			revision: 3,
		};
		let (report, mut failure) = watch::channel(None);
		let error = queue_confirmation(&controls, control, owner, true).unwrap();
		report_confirmation_failure(&report, 7, error, owner, &ctx);
		assert!(failure.has_changed().unwrap());
		let current = (*failure.borrow()).unwrap();
		assert!(take_confirmation_failure(&mut failure, &events).is_none());
		assert!(failure.has_changed().unwrap());
		let envelope = current.envelope();
		assert_eq!(envelope.generation, 7);
		assert!(matches!(
			envelope.event,
			Event::Voice(E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 5,
				revision: 3,
				..
			})
		));
		assert_eq!(reliable.send.capacity(), 0);
		assert_eq!(reliable.bytes.available_permits(), 0);
		assert_eq!(control_receive.len(), 8);
		for (request, revision) in [(5, 2), (5, 3), (4, 9)] {
			report_confirmation_failure(
				&report,
				7,
				E::SessionConfirmationFailed {
					channel: model::Id(20),
					request,
					revision,
					message: "stale",
				},
				owner,
				&ctx,
			);
			assert!(failure.has_changed().unwrap());
			assert_eq!(*failure.borrow(), Some(current));
		}
		drop(held);
		for id in 1..=RELIABLE_ITEMS as u64 {
			let Event::ChannelCreated(channel) = events.try_recv().unwrap().event else {
				panic!("queued account event lost");
			};
			assert_eq!(channel.id, model::Id(id));
		}
		assert!(events.try_recv().is_err());
		let envelope = take_confirmation_failure(&mut failure, &events).unwrap();
		assert!(take_confirmation_failure(&mut failure, &events).is_none());
		let mut state = client_core::State {
			generation: 7,
			auth: client_core::auth::AuthState::Authenticated,
			..Default::default()
		};
		state.apply(envelope);
		assert_eq!(state.auth, client_core::auth::AuthState::Authenticated);
		for _ in 0..8 {
			assert!(matches!(control_receive.try_recv(), Ok(V::Sync { .. })));
		}
		report_confirmation_failure(
			&report,
			7,
			E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 6,
				revision: 1,
				message: "new attempt",
			},
			Some((model::Id(20), 6, false)),
			&ctx,
		);
		assert!(failure.has_changed().unwrap());
		assert_eq!(failure.borrow_and_update().unwrap().request, 6);
		assert!(size_of::<Option<ConfirmationFailure>>() <= 64);
	}

	#[tokio::test]
	async fn confirmation_queue_pressure_is_revision_scoped_and_preserves_queued_controls() {
		use client_core::voice::{Command as V, Event as E};
		let (sender, mut receiver) = mpsc::channel(8);
		for _ in 0..8 {
			sender
				.try_send(V::Sync {
					channel: model::Id(20),
				})
				.unwrap();
		}
		let confirmation = V::ConfirmSession {
			channel: model::Id(20),
			request: 5,
			revision: 2,
		};
		let owner = Some((model::Id(20), 5, false));
		assert!(matches!(
			queue_confirmation(&sender, confirmation, owner, true),
			Some(E::SessionConfirmationFailed {
				channel: model::Id(20),
				request: 5,
				revision: 2,
				message: "Call confirmation was not sent; the voice queue is full"
			})
		));
		assert_eq!(receiver.len(), 8);
		for _ in 0..8 {
			assert!(matches!(
				receiver.recv().await,
				Some(V::Sync {
					channel: model::Id(20)
				})
			));
		}
		for (active, online) in [
			(None, true),
			(Some((model::Id(20), 6, false)), true),
			(owner, false),
		] {
			assert!(matches!(
				queue_confirmation(&sender, confirmation, active, online),
				Some(E::SessionConfirmationFailed {
					request: 5,
					revision: 2,
					..
				})
			));
			assert!(receiver.try_recv().is_err());
		}
		assert!(queue_confirmation(&sender, confirmation, owner, true).is_none());
		assert!(matches!(
			receiver.recv().await,
			Some(V::ConfirmSession {
				request: 5,
				revision: 2,
				..
			})
		));
		drop(receiver);
		assert!(matches!(
			queue_confirmation(&sender, confirmation, owner, true),
			Some(E::SessionConfirmationFailed {
				request: 5,
				revision: 2,
				message: "Call confirmation was not sent; voice signaling is disconnected",
				..
			})
		));
	}

	#[tokio::test]
	async fn abandonment_survives_a_full_queue_and_precedes_a_fresh_join() {
		use client_core::voice::Command as V;
		let (sender, mut receiver) = mpsc::channel(8);
		for _ in 0..8 {
			sender
				.try_send(V::Sync {
					channel: model::Id(20),
				})
				.unwrap();
		}
		let release = V::AbandonSession {
			channel: model::Id(20),
			request: 5,
		};
		let mut pending = None;
		queue_abandonment(&sender, &mut pending, release, Some((model::Id(21), 6)));
		assert!(pending.is_none()); // An older scope cannot displace the current owner’s later release.
		queue_abandonment(&sender, &mut pending, release, Some((model::Id(20), 5)));
		assert!(matches!(
			pending,
			Some(V::AbandonSession {
				channel: model::Id(20),
				request: 5
			})
		));
		// A delayed stale release must not discard the cleanup that actually releases the old scope.
		queue_abandonment(
			&sender,
			&mut pending,
			V::AbandonSession {
				channel: model::Id(20),
				request: 4,
			},
			None,
		);
		assert!(matches!(
			pending,
			Some(V::AbandonSession {
				channel: model::Id(20),
				request: 5
			})
		));
		let join = V::Join {
			channel: model::Id(21),
			request: 6,
			ring: false,
			mute: false,
			deaf: false,
		};
		assert!(abandonment_blocks_join(pending, join));
		assert!(receiver.recv().await.is_some());
		sender
			.reserve()
			.await
			.unwrap()
			.send(pending.take().unwrap());
		assert!(!abandonment_blocks_join(pending, join));
		// The release used the only free slot: an immediately queued Join must fail locally.
		let mut owner = Some((model::Id(21), 6, true));
		assert!(matches!(
			queue_join(&sender, join, (model::Id(21), 6), &mut owner),
			Some(client_core::voice::Event::Failed {
				channel: model::Id(21),
				request: 6,
				..
			})
		));
		assert!(owner.is_none());
		for _ in 0..7 {
			assert!(matches!(receiver.recv().await, Some(V::Sync { .. })));
		}
		assert!(queue_join(&sender, join, (model::Id(21), 6), &mut owner).is_none());
		assert!(matches!(
			receiver.recv().await,
			Some(V::AbandonSession {
				channel: model::Id(20),
				request: 5
			})
		));
		assert!(matches!(
			receiver.recv().await,
			Some(V::Join {
				channel: model::Id(21),
				request: 6,
				..
			})
		));
		drop(receiver);
		queue_abandonment(&sender, &mut pending, release, None);
		assert!(pending.is_none()); // Closed queues are handled by the existing Gateway task.
	}

	#[test]
	fn abandoning_an_unconfirmed_attempt_clears_dispatch_without_an_http_hangup() {
		use client_core::voice::Command as V;
		let mut active = Some((model::Id(20), 5, false));
		assert_eq!(
			ring_action(
				V::AbandonSession {
					channel: model::Id(20),
					request: 4
				},
				model::Id(1),
				&mut active,
				true
			),
			Ok(None)
		);
		assert_eq!(active, Some((model::Id(20), 5, false)));
		assert_eq!(
			ring_action(
				V::AbandonSession {
					channel: model::Id(20),
					request: 5
				},
				model::Id(1),
				&mut active,
				true
			),
			Ok(None)
		);
		assert!(active.is_none());
		assert_eq!(
			ring_action(
				V::Ring {
					channel: model::Id(20),
					request: 5
				},
				model::Id(1),
				&mut active,
				true
			),
			Err(())
		);
	}
	#[tokio::test]
	async fn activity_privacy_waits_for_opt_in_and_propagates_expired_session() {
		let api = Arc::new(
			DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_ACTIVITY_PRIVACY_TOKEN".into()).unwrap(),
			))
			.unwrap(),
		);
		// Stop before the worker starts: this test can never send an HTTP request.
		api.stop();
		let (enabled, receive_enabled) = watch::channel(false);
		let (requests, receive_requests) = mpsc::channel(1);
		let (send_report, mut report) = watch::channel(Ok(None));
		let (finished, mut terminal) = watch::channel(None);
		let worker = tokio::spawn(run_activity_sharing(
			api,
			receive_enabled,
			receive_requests,
			send_report,
			finished,
			egui::Context::default(),
		));
		report.changed().await.unwrap();
		assert_eq!(*report.borrow_and_update(), Ok(None));
		for request in [false, true] {
			requests.send(request).await.unwrap();
			drop(requests.reserve().await.unwrap());
		}
		assert!(
			tokio::time::timeout(Duration::from_millis(25), report.changed())
				.await
				.is_err()
		);
		assert_eq!(*terminal.borrow(), None);
		enabled.send(true).unwrap();
		tokio::time::timeout(Duration::from_secs(1), terminal.changed())
			.await
			.unwrap()
			.unwrap();
		assert_eq!(*terminal.borrow(), Some(Failure::Expired));
		worker.await.unwrap();
		assert_eq!(*report.borrow(), Err(Failure::Expired));
	}

	fn queued_channel(id: u64) -> client_core::Envelope {
		client_core::Envelope {
			generation: 1,
			event: client_core::Event::ChannelCreated(model::Channel {
				id: model::Id(id),
				guild: Some(model::Id(1)),
				parent_id: None,
				position: 0,
				name: "Synthetic channel".into(),
				icon: None,
				kind: 0,
				recipients: vec![],
				member_list_id: None,
				tags: None,
				message_count: None,
				last_message: None,
			}),
		}
	}

	fn large_startup() -> client_core::Startup {
		use serde_json::json;
		let guilds: Vec<_> = (0..200_u64).map(|g| {
			let id = 10 + g;
			json!({"id":id.to_string(),"name":"Synthetic large account","owner_id":"1",
				"roles":[{"id":id.to_string(),"permissions":"1024"}],
				"channels":(0..100).map(|c| json!({"id":(1000+g*100+c).to_string(),
					"name":"synthetic-channel".repeat(6),"type":0,"permission_overwrites":[]})).collect::<Vec<_>>()})
		}).collect();
		let bytes = serde_json::to_vec(&json!({"user":{"id":"1","username":"Synthetic"},
			"session_id":"synthetic","resume_gateway_url":"wss://gateway.discord.gg/","guilds":guilds}))
		.unwrap();
		let envelope = discord_protocol::ready::decode(&bytes).unwrap();
		let permissions = envelope.permissions().unwrap();
		let (mut ready, warnings) = envelope.navigation().unwrap();
		let (guilds, channels) = ready.navigation().unwrap();
		client_core::Startup {
			premium_type: 0,
			user: ready.user.into_model(),
			guilds,
			channels,
			permissions,
			read_state: client_core::read_state::Event::Snapshot {
				entries: None,
				version: None,
				partial: false,
			},
			notifications: None,
			session_dnd: None,
			warnings,
		}
	}

	#[test]
	fn large_startup_crosses_the_queue_atomically_and_keeps_ordinary_budgets() {
		let ctx = egui::Context::default();
		let (send, mut events) = reliable_events(ctx.clone());
		let (typing, _) = mpsc::channel(8);
		let startup = large_startup();
		assert!(startup.bytes() > MAX_EVENT_BYTES);
		let mut state = client_core::State::default();
		let generation = state.generation;
		emit_event(
			&send,
			&typing,
			Envelope {
				generation,
				event: Event::Startup(Box::new(startup.prepare().unwrap())),
			},
			&ctx,
		)
		.unwrap();
		assert_eq!(send.startup.available_permits(), 0);
		assert_eq!(send.bytes.available_permits(), RELIABLE_BYTES);
		// A second startup cannot multiply the reserved 128 MiB capacity.
		assert_eq!(
			emit_event(
				&send,
				&typing,
				Envelope {
					generation,
					event: Event::Startup(Box::new(large_startup().prepare().unwrap()))
				},
				&ctx
			),
			Err(Failure::Busy)
		);
		let mut later = queued_channel(25_000);
		later.generation = generation;
		assert!(events.take_pressure());
		assert_eq!(emit_event(&send, &typing, later, &ctx), Err(Failure::Busy));
		state.apply(events.try_recv().unwrap());
		let mut later = queued_channel(25_000);
		later.generation = generation;
		emit_event(&send, &typing, later, &ctx).unwrap();
		assert_eq!(state.auth, client_core::auth::AuthState::Authenticated);
		assert_eq!((state.guilds.len(), state.channels.len()), (200, 20_000));
		assert!(state.can_read_history(model::Id(1000)));
		assert!(state.can_read_history(model::Id(20_999)));
		assert_eq!(send.startup.available_permits(), 1);
		let next = events.try_recv().unwrap();
		assert!(matches!(next.event, Event::ChannelCreated(_)));
		state.apply(next);
		assert_eq!(send.bytes.available_permits(), RELIABLE_BYTES);
		state.logout();
		assert!(state.channels.is_empty() && state.guilds.is_empty());
		// Dropping the consumer also releases startup capacity.
		emit_event(
			&send,
			&typing,
			Envelope {
				generation,
				event: Event::Startup(Box::new(large_startup().prepare().unwrap())),
			},
			&ctx,
		)
		.unwrap();
		drop(events);
		assert_eq!(send.startup.available_permits(), 1);
	}

	#[test]
	fn reliable_navigation_burst_is_fifo_and_item_bounded() {
		let ctx = egui::Context::default();
		let (send, mut events) = reliable_events(ctx.clone());
		let (typing, _) = mpsc::channel(8);
		// One entire navigation fanout plus ordinary event slots fits without a UI drain.
		for id in 1..=RELIABLE_ITEMS as u64 {
			emit_event(&send, &typing, queued_channel(id), &ctx).unwrap();
		}
		let retained = send.bytes.available_permits();
		assert_eq!(
			emit_event(&send, &typing, queued_channel(0), &ctx),
			Err(Failure::Busy)
		);
		assert_eq!(send.bytes.available_permits(), retained);
		assert!(events.take_pressure());
		assert!(send.delivery_blocked());
		for id in 1..=RELIABLE_ITEMS as u64 {
			let envelope = events.try_recv().unwrap();
			assert_eq!(envelope.generation, 1);
			let Event::ChannelCreated(channel) = envelope.event else {
				panic!("Reliable events must be retained in order");
			};
			assert_eq!(channel.id, model::Id(id));
		}
		assert!(events.try_recv().is_err());
		assert_eq!(send.bytes.available_permits(), RELIABLE_BYTES);
	}

	#[test]
	fn reliable_byte_budget_releases_on_receive_rejection_and_drop() {
		let ctx = egui::Context::default();
		let (send, mut events) = reliable_events(ctx.clone());
		let (typing, _) = mpsc::channel(8);
		let envelope = queued_channel(2);
		let charge = envelope.event.bytes() + size_of::<(Envelope, OwnedSemaphorePermit)>()
			- size_of::<Event>();
		let held = send
			.bytes
			.clone()
			.try_acquire_many_owned((RELIABLE_BYTES - charge) as u32)
			.unwrap();
		emit_event(&send, &typing, envelope, &ctx).unwrap();
		assert_eq!(send.bytes.available_permits(), 0);
		assert_eq!(
			emit_event(&send, &typing, queued_channel(3), &ctx),
			Err(Failure::Busy)
		);
		assert_eq!(events.receive.len(), 1);
		events.try_recv().unwrap();
		assert_eq!(send.bytes.available_permits(), charge);
		assert!(events.take_pressure());
		assert_eq!(
			emit_event(&send, &typing, queued_channel(3), &ctx),
			Err(Failure::Busy)
		);
		drop(held);
		assert!(!events.take_pressure());
		emit_event(&send, &typing, queued_channel(3), &ctx).unwrap();
		let before = send.bytes.available_permits();
		let mut oversized = queued_channel(4);
		if let Event::ChannelCreated(channel) = &mut oversized.event {
			channel.name = "x".repeat(MAX_EVENT_BYTES);
		}
		assert_eq!(
			emit_event(&send, &typing, oversized, &ctx),
			Err(Failure::CapacityAt(
				"Account synchronization event exceeds 4 MiB; connection stopped"
			))
		);
		assert_eq!(send.bytes.available_permits(), before);
		drop(events);
		assert_eq!(send.bytes.available_permits(), RELIABLE_BYTES);
		assert_eq!(
			emit_event(&send, &typing, queued_channel(5), &ctx),
			Err(Failure::Network)
		);
		assert_eq!(send.bytes.available_permits(), RELIABLE_BYTES);
	}

	#[test]
	fn typing_burst_cannot_consume_reliable_message_slots() {
		let ctx = eframe::egui::Context::default();
		let (send, mut events) = super::reliable_events(ctx.clone());
		let (typing_send, mut typing) = tokio::sync::mpsc::channel(8);
		for user in 1..=100 {
			super::emit_event(
				&send,
				&typing_send,
				client_core::Envelope {
					generation: 1,
					event: client_core::Event::Typing(client_core::typing::Signal {
						channel: model::Id(10),
						user: model::Id(user),
						timestamp: 1,
					}),
				},
				&ctx,
			)
			.unwrap();
		}
		assert_eq!(typing.len(), 8);
		assert!(events.receive.is_empty());
		super::emit_event(
			&send,
			&typing_send,
			client_core::Envelope {
				generation: 1,
				event: client_core::Event::Delete {
					channel: model::Id(10),
					id: model::Id(20),
				},
			},
			&ctx,
		)
		.unwrap();
		assert!(matches!(
			events.try_recv().unwrap().event,
			client_core::Event::Delete { .. }
		));
		for _ in 0..8 {
			typing.try_recv().unwrap();
		}
	}
	#[test]
	fn typing_wakeups_are_selected_bounded_and_coalesced() {
		use client_core::typing::Signal;
		use model::Id;
		use std::time::{Duration, Instant};
		let mut gate = super::TypingGate::default();
		let now = Instant::now();
		let signal = Signal {
			channel: Id(10),
			user: Id(1),
			timestamp: 1,
		};
		assert!(!gate.accept(signal, 0, now));
		assert!(!gate.accept(signal, 11, now));
		for user in 1..=8 {
			let signal = Signal {
				user: Id(user),
				..signal
			};
			assert!(gate.accept(signal, 10, now));
			assert!(!gate.accept(signal, 10, now));
		}
		for user in 9..=1_000 {
			assert!(!gate.accept(
				Signal {
					user: Id(user),
					..signal
				},
				10,
				now
			));
		}
		assert!(!gate.accept(signal, 10, now + Duration::from_millis(1_999)));
		assert!(gate.accept(signal, 10, now + Duration::from_secs(2)));
		assert!(gate.accept(
			Signal {
				channel: Id(11),
				..signal
			},
			11,
			now
		));
		assert!(!gate.accept(signal, 11, now));
	}
	use super::*;
	#[test]
	fn recipient_writes_reject_stale_self_and_removed_members() {
		use model::Id;
		let scope = Some((Id(2), 7, true));
		let peers = [Id(3), Id(4)];
		assert!(recipient_action(
			Id(2),
			7,
			Id(3),
			Id(1),
			scope,
			Some(&peers)
		));
		for (channel, request, recipient, active, members) in [
			(Id(2), 6, Id(3), scope, Some(peers.as_slice())),
			(Id(9), 7, Id(3), scope, Some(peers.as_slice())),
			(Id(2), 7, Id(1), scope, Some(peers.as_slice())),
			(Id(2), 7, Id(8), scope, Some(peers.as_slice())),
			(Id(2), 7, Id(3), None, Some(peers.as_slice())),
			(Id(2), 7, Id(3), scope, None),
		] {
			assert!(!recipient_action(
				channel,
				request,
				recipient,
				Id(1),
				active,
				members
			));
		}
		let oversized = [Id(3); client_core::voice::MAX_PARTICIPANTS];
		assert!(!recipient_action(
			Id(2),
			7,
			Id(3),
			Id(1),
			scope,
			Some(&oversized)
		));
	}
	#[tokio::test]
	async fn multiple_prejoin_calls_preserve_target_and_unrelated_events_do_not_cancel_write() {
		use client_core::voice::Event as V;
		use model::Id;
		let (owner, first, second, recipient, other) = (Id(1), Id(2), Id(4), Id(3), Id(5));
		let channels = BTreeMap::from([(first, vec![recipient]), (second, vec![other])]);
		let call = |channel, ringing| {
			Event::Voice(V::Call {
				channel,
				ringing: Some(ringing),
				participants: Some(vec![]),
				unavailable: false,
			})
		};
		let calls = Arc::new(Mutex::new(RecipientCalls::default()));
		calls
			.lock()
			.unwrap()
			.observe(&call(first, vec![]), owner, &channels);
		calls
			.lock()
			.unwrap()
			.observe(&call(second, vec![other]), owner, &channels);
		assert_eq!(calls.lock().unwrap().observed.len(), 2);
		assert!(calls.lock().unwrap().active.is_none());
		calls.lock().unwrap().join(first, 7, true);
		calls.lock().unwrap().observe(
			&Event::Voice(V::SessionConfirmed {
				channel: first,
				request: 7,
				revision: 1,
			}),
			owner,
			&channels,
		);
		assert!(
			calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(first, 7, recipient, false)
		);
		let (changes, updates) = watch::channel(0u64);
		let (_online_send, online) = watch::channel(true);
		let control = client_core::voice::Command::RingRecipient {
			channel: first,
			request: 7,
			recipient,
			stop: false,
		};
		let cancelled = wait_for_recipient_invalidation(
			updates,
			online,
			control,
			calls.clone(),
			Arc::new(Mutex::new(channels.clone())),
		);
		tokio::pin!(cancelled);
		let unrelated_state = Event::Voice(V::State {
			guild: None,
			channel: Some(second),
			user: other,
			request: None,
			member: None,
			session: None,
			negotiation_revision: None,
			server_muted: false,
			server_deafened: false,
			stage: Default::default(),
			muted: false,
			deafened: false,
			video: false,
			streaming: false,
		});
		for event in [
			unrelated_state,
			call(second, vec![]),
			Event::RecipientRemoved {
				channel: second,
				user: other,
			},
		] {
			if calls.lock().unwrap().observe(&event, owner, &channels) {
				changes.send_modify(|revision| *revision += 1);
			}
		}
		assert_eq!(*changes.borrow(), 0);
		assert!(
			calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(first, 7, recipient, false)
		);
		// A pending request survives unrelated updates; relevant updates still cancel it.
		assert!(
			tokio::time::timeout(Duration::from_millis(1), &mut cancelled)
				.await
				.is_err()
		);
		assert!(
			calls
				.lock()
				.unwrap()
				.observe(&call(first, vec![recipient]), owner, &channels)
		);
		changes.send_modify(|revision| *revision += 1);
		tokio::time::timeout(Duration::from_secs(1), cancelled)
			.await
			.unwrap();
		assert!(
			!calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(first, 7, recipient, false)
		);
		assert!(
			calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(first, 7, recipient, true)
		);
	}

	#[tokio::test]
	async fn recipient_worker_survives_owner_controls_but_departure_rejects_queued_actions() {
		use client_core::voice::Event as V;
		use model::Id;
		let (owner, channel, recipient) = (Id(1), Id(2), Id(3));
		let channels = BTreeMap::from([(channel, vec![recipient])]);
		let state = |user, target, guild, muted, video| {
			Event::Voice(V::State {
				guild,
				channel: target,
				user,
				request: None,
				member: None,
				session: None,
				negotiation_revision: None,
				server_muted: false,
				server_deafened: false,
				stage: Default::default(),
				muted,
				deafened: muted,
				video,
				streaming: false,
			})
		};
		let calls = Arc::new(Mutex::new(RecipientCalls::default()));
		calls.lock().unwrap().observe(
			&Event::Voice(V::Call {
				channel,
				ringing: Some(vec![]),
				participants: Some(vec![]),
				unavailable: false,
			}),
			owner,
			&channels,
		);
		calls.lock().unwrap().join(channel, 7, true);
		calls.lock().unwrap().observe(
			&state(owner, Some(channel), None, false, false),
			owner,
			&channels,
		);
		calls.lock().unwrap().observe(
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 7,
				revision: 1,
			}),
			owner,
			&channels,
		);
		assert!(
			calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		let (changes, updates) = watch::channel(0u64);
		let (_online, available) = watch::channel(true);
		let control = client_core::voice::Command::RingRecipient {
			channel,
			request: 7,
			recipient,
			stop: false,
		};
		let pending = wait_for_recipient_invalidation(
			updates.clone(),
			available.clone(),
			control,
			calls.clone(),
			Arc::new(Mutex::new(channels.clone())),
		);
		tokio::pin!(pending);
		for event in [
			state(owner, Some(channel), None, true, false),
			state(owner, Some(channel), None, false, true),
		] {
			assert!(!calls.lock().unwrap().observe(&event, owner, &channels));
			assert!(
				calls
					.lock()
					.unwrap()
					.as_ref()
					.unwrap()
					.allows(channel, 7, recipient, false)
			);
		}
		assert!(
			tokio::time::timeout(Duration::from_millis(1), &mut pending)
				.await
				.is_err()
		);
		// Owner departure invalidates before any TakenOver/Departed event reaches dispatch.
		assert!(calls.lock().unwrap().observe(
			&state(owner, None, None, false, false),
			owner,
			&channels
		));
		changes.send_replace(1);
		tokio::time::timeout(Duration::from_secs(1), pending)
			.await
			.unwrap();
		assert!(
			!calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		assert!(
			!calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, true)
		);
		// A newer explicit join requires its own confirmation, then can ring again.
		calls.lock().unwrap().join(channel, 8, true);
		calls.lock().unwrap().observe(
			&state(owner, Some(channel), None, false, false),
			owner,
			&channels,
		);
		assert!(
			!calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, 8, recipient, false)
		);
		calls.lock().unwrap().observe(
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 8,
				revision: 2,
			}),
			owner,
			&channels,
		);
		assert!(
			calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, 8, recipient, false)
		);
		let mut attempt = 8;
		for (target, guild) in [(Some(Id(4)), None), (Some(channel), Some(Id(5)))] {
			assert!(calls.lock().unwrap().observe(
				&state(owner, target, guild, false, false),
				owner,
				&channels
			));
			assert!(
				!calls
					.lock()
					.unwrap()
					.as_ref()
					.unwrap()
					.allows(channel, attempt, recipient, false)
			);
			attempt += 1;
			calls.lock().unwrap().join(channel, attempt, true);
			calls.lock().unwrap().observe(
				&state(owner, Some(channel), None, false, false),
				owner,
				&channels,
			);
			calls.lock().unwrap().observe(
				&Event::Voice(V::SessionConfirmed {
					channel,
					request: attempt,
					revision: attempt,
				}),
				owner,
				&channels,
			);
		}
		let control = client_core::voice::Command::RingRecipient {
			channel,
			request: attempt,
			recipient,
			stop: false,
		};
		let pending = wait_for_recipient_invalidation(
			updates,
			available,
			control,
			calls.clone(),
			Arc::new(Mutex::new(channels.clone())),
		);
		tokio::pin!(pending);
		assert!(calls.lock().unwrap().observe(
			&state(recipient, Some(channel), None, false, false),
			owner,
			&channels
		));
		changes.send_replace(2);
		tokio::time::timeout(Duration::from_secs(1), pending)
			.await
			.unwrap();
		assert!(
			!calls
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.allows(channel, attempt, recipient, false)
		);
	}

	#[test]
	fn discovered_recipient_metadata_has_fixed_item_and_allocated_id_byte_limits() {
		use client_core::voice::{Event as V, MAX_DM_CALLS, MAX_PARTICIPANTS};
		use model::Id;
		let mut calls = RecipientCalls::default();
		let mut channels = BTreeMap::new();
		for index in 0..MAX_DM_CALLS + 1 {
			let channel = Id(index as u64 + 10);
			channels.insert(channel, vec![Id(2)]);
			calls.observe(
				&Event::Voice(V::Call {
					channel,
					ringing: Some(vec![Id(2)]),
					participants: Some(vec![]),
					unavailable: false,
				}),
				Id(1),
				&channels,
			);
		}
		assert_eq!(calls.observed.len(), MAX_DM_CALLS);
		assert_eq!(calls.observed.capacity(), MAX_DM_CALLS);
		assert!(
			calls
				.observed
				.iter()
				.flatten()
				.all(|call| call.channel != Id(10))
		);
		calls.join(Id(11), 7, true);
		let bytes = calls
			.observed
			.iter()
			.flatten()
			.chain(calls.active.iter())
			.map(|call| {
				(call.joined.capacity() + call.ringing.as_ref().map_or(0, Vec::capacity))
					* size_of::<Id>()
			})
			.sum::<usize>();
		assert_eq!(
			bytes,
			(MAX_DM_CALLS + 1) * MAX_PARTICIPANTS * 2 * size_of::<Id>()
		);
		assert!(calls.observe(&Event::Disconnected, Id(1), &channels));
		assert!(calls.active.is_none());
		assert!(calls.observed.is_empty());
	}

	#[test]
	fn existing_call_metadata_survives_join_with_only_state_and_confirmation_afterward() {
		use client_core::voice::Event as V;
		use model::Id;
		let (channel, owner, recipient) = (Id(2), Id(1), Id(3));
		let channels = BTreeMap::from([(channel, vec![recipient])]);
		let mut active = None;
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::Call {
				channel,
				ringing: Some(vec![]),
				participants: Some(vec![]),
				unavailable: false,
			}),
			owner,
			&channels,
		);
		assert_eq!(active.as_ref().unwrap().request, None);
		assert!(
			!active
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		RecipientCall::join(&mut active, channel, 7, true);
		assert!(
			!active
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::State {
				guild: None,
				channel: Some(channel),
				user: owner,
				request: Some(7),
				member: None,
				session: None,
				negotiation_revision: Some(1),
				server_muted: false,
				server_deafened: false,
				stage: Default::default(),
				muted: false,
				deafened: false,
				video: false,
				streaming: false,
			}),
			owner,
			&channels,
		);
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 7,
				revision: 1,
			}),
			owner,
			&channels,
		);
		assert!(
			active
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		// Starting a new attempt keeps service metadata but waits for its own confirmation.
		RecipientCall::join(&mut active, channel, 8, true);
		assert!(
			!active
				.as_ref()
				.unwrap()
				.allows(channel, 7, recipient, false)
		);
		assert!(
			!active
				.as_ref()
				.unwrap()
				.allows(channel, 8, recipient, false)
		);
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 7,
				revision: 1,
			}),
			owner,
			&channels,
		);
		assert!(!active.as_ref().unwrap().confirmed);
		RecipientCall::join(&mut active, Id(99), 9, false);
		assert!(active.is_none());
	}

	#[test]
	fn queued_recipient_actions_use_latest_confirmed_ringing_and_peer_presence() {
		use client_core::voice::Event as V;
		use model::Id;
		let (channel, owner, recipient) = (Id(2), Id(1), Id(3));
		let channels = BTreeMap::from([(channel, vec![recipient])]);
		let mut active = Some(RecipientCall::new(channel, 7));
		let allowed = |active: &Option<RecipientCall>, stop| {
			active
				.as_ref()
				.is_some_and(|call| call.allows(channel, 7, recipient, stop))
		};
		assert!(!allowed(&active, false));
		assert!(!allowed(&active, true));
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 6,
				revision: 1,
			}),
			owner,
			&channels,
		);
		assert!(!active.as_ref().unwrap().confirmed);
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::SessionConfirmed {
				channel,
				request: 7,
				revision: 2,
			}),
			owner,
			&channels,
		);
		assert!(
			!allowed(&active, true),
			"unknown ringing cannot authorize a queued stop"
		);
		let call = |ringing| {
			Event::Voice(V::Call {
				channel,
				ringing,
				participants: Some(vec![]),
				unavailable: false,
			})
		};
		RecipientCall::observe(&mut active, &call(Some(vec![])), owner, &channels);
		assert!(allowed(&active, false));
		// A start queued earlier is rejected after service ringing begins.
		RecipientCall::observe(&mut active, &call(Some(vec![recipient])), owner, &channels);
		assert!(!allowed(&active, false));
		assert!(allowed(&active, true));
		// A stop queued earlier is rejected after the service clears ringing.
		RecipientCall::observe(&mut active, &call(Some(vec![])), owner, &channels);
		assert!(!allowed(&active, true));
		assert!(allowed(&active, false));
		let peer = |channel, guild| {
			Event::Voice(V::State {
				guild,
				channel,
				user: recipient,
				request: None,
				member: None,
				session: None,
				negotiation_revision: None,
				server_muted: false,
				server_deafened: false,
				stage: Default::default(),
				muted: false,
				deafened: false,
				video: false,
				streaming: false,
			})
		};
		// Peer joined before dequeue: same request and membership are insufficient.
		RecipientCall::observe(&mut active, &peer(Some(channel), None), owner, &channels);
		assert!(!allowed(&active, false));
		RecipientCall::observe(
			&mut active,
			&peer(Some(Id(99)), Some(Id(98))),
			owner,
			&channels,
		);
		assert!(allowed(&active, false));
		// Malformed/duplicate service metadata never overwrites the validated state.
		RecipientCall::observe(
			&mut active,
			&call(Some(vec![recipient, recipient])),
			owner,
			&channels,
		);
		assert!(allowed(&active, false));
		let call = active.as_ref().unwrap();
		assert_eq!(
			(call.joined.capacity() + call.ringing.as_ref().unwrap().capacity()) * size_of::<Id>(),
			1024
		);
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::Departed {
				channel,
				request: 6,
			}),
			owner,
			&channels,
		);
		assert!(active.is_some());
		RecipientCall::observe(
			&mut active,
			&Event::Voice(V::TakenOver {
				channel,
				request: 7,
			}),
			owner,
			&channels,
		);
		assert!(active.is_none());
	}

	#[test]
	fn recipient_dispatch_rejects_queued_writes_after_takeover_and_local_abandon() {
		use client_core::voice::Command as V;
		use model::Id;
		let (channel, owner, recipient) = (Id(2), Id(1), Id(3));
		let peers = [recipient];
		let mut active = Some((channel, 7, true));
		assert!(recipient_action(
			channel,
			7,
			recipient,
			owner,
			active,
			Some(&peers)
		));
		assert!(release_taken_over(&mut active, Some((channel, 7))));
		assert!(!recipient_action(
			channel,
			7,
			recipient,
			owner,
			active,
			Some(&peers)
		));
		let join = |request| V::Join {
			channel,
			request,
			ring: false,
			mute: false,
			deaf: false,
		};
		assert!(join_taken_over(join(7), Some((channel, 7))));
		assert!(!join_taken_over(join(8), Some((channel, 7))));
		assert_eq!(ring_action(join(8), owner, &mut active, true), Ok(None));
		assert!(recipient_action(
			channel,
			8,
			recipient,
			owner,
			active,
			Some(&peers)
		));
		assert!(!release_taken_over(&mut active, Some((channel, 7))));
		assert_eq!(
			ring_action(
				V::AbandonSession {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			),
			Ok(None)
		);
		assert!(recipient_action(
			channel,
			8,
			recipient,
			owner,
			active,
			Some(&peers)
		));
		assert_eq!(
			ring_action(
				V::AbandonSession {
					channel,
					request: 8
				},
				owner,
				&mut active,
				true
			),
			Ok(None)
		);
		assert!(!recipient_action(
			channel,
			8,
			recipient,
			owner,
			active,
			Some(&peers)
		));
	}

	fn recipient_worker_fixture(
		stop: bool,
	) -> (
		client_core::voice::Command,
		RecipientCalls,
		BTreeMap<model::Id, Vec<model::Id>>,
	) {
		let (channel, recipient) = (model::Id(2), model::Id(3));
		let mut call = RecipientCall::new(channel, 7);
		call.confirmed = true;
		call.ringing = Some(if stop { vec![recipient] } else { vec![] });
		let calls = RecipientCalls {
			active: Some(call),
			..Default::default()
		};
		(
			client_core::voice::Command::RingRecipient {
				channel,
				request: 7,
				recipient,
				stop,
			},
			calls,
			BTreeMap::from([(channel, vec![recipient, model::Id(4)])]),
		)
	}
	#[tokio::test]
	async fn recipient_worker_rechecks_target_and_preserves_unrelated_peer_controls() {
		use client_core::voice::Event as V;
		use model::Id;
		assert!(size_of::<client_core::voice::Command>() <= 64);
		let (control, calls, channels) = recipient_worker_fixture(false);
		let calls = Arc::new(Mutex::new(calls));
		let channels = Arc::new(Mutex::new(channels));
		let (changes, updates) = watch::channel(0u64);
		let (_online, available) = watch::channel(true);
		let pending = wait_for_recipient_invalidation(
			updates,
			available,
			control,
			calls.clone(),
			channels.clone(),
		);
		tokio::pin!(pending);
		for (user, muted, video) in [
			(Id(4), false, false),
			(Id(4), true, false),
			(Id(4), false, true),
		] {
			let event = Event::Voice(V::State {
				guild: None,
				channel: Some(Id(2)),
				user,
				request: None,
				member: None,
				session: None,
				negotiation_revision: None,
				server_muted: false,
				server_deafened: false,
				stage: Default::default(),
				muted,
				deafened: muted,
				video,
				streaming: false,
			});
			{
				let channels = channels.lock().unwrap();
				let mut calls = calls.lock().unwrap();
				assert!(calls.observe(&event, Id(1), &channels));
				assert!(
					recipient_write_allowed(control, &calls, &channels),
					"main-loop abort guard keeps the eligible target"
				);
			}
			changes.send_modify(|revision| *revision += 1);
			assert!(
				tokio::time::timeout(Duration::from_millis(1), &mut pending)
					.await
					.is_err(),
				"unrelated peer controls retain the pending worker"
			);
		}
		// The requested recipient joining stops the obsolete start without a false error.
		let event = Event::Voice(V::State {
			guild: None,
			channel: Some(Id(2)),
			user: Id(3),
			request: None,
			member: None,
			session: None,
			negotiation_revision: None,
			server_muted: false,
			server_deafened: false,
			stage: Default::default(),
			muted: false,
			deafened: false,
			video: false,
			streaming: false,
		});
		{
			let channels = channels.lock().unwrap();
			let mut calls = calls.lock().unwrap();
			assert!(calls.observe(&event, Id(1), &channels));
			assert!(!recipient_write_allowed(control, &calls, &channels));
		}
		changes.send_modify(|revision| *revision += 1);
		tokio::time::timeout(Duration::from_secs(1), pending)
			.await
			.unwrap();
	}
	#[tokio::test]
	async fn recipient_worker_stops_after_service_confirms_start_or_stop() {
		for stop in [false, true] {
			let (control, calls, channels) = recipient_worker_fixture(stop);
			let calls = Arc::new(Mutex::new(calls));
			let channels = Arc::new(Mutex::new(channels));
			let (changes, updates) = watch::channel(0u64);
			let (_online, available) = watch::channel(true);
			let pending = wait_for_recipient_invalidation(
				updates,
				available,
				control,
				calls.clone(),
				channels.clone(),
			);
			tokio::pin!(pending);
			assert!(
				tokio::time::timeout(Duration::from_millis(1), &mut pending)
					.await
					.is_err()
			);
			let event = Event::Voice(client_core::voice::Event::Call {
				channel: model::Id(2),
				ringing: Some(if stop { vec![] } else { vec![model::Id(3)] }),
				participants: None,
				unavailable: false,
			});
			{
				let channels = channels.lock().unwrap();
				let mut calls = calls.lock().unwrap();
				assert!(calls.observe(&event, model::Id(1), &channels));
				assert!(!recipient_write_allowed(control, &calls, &channels));
			}
			changes.send_modify(|revision| *revision += 1);
			tokio::time::timeout(Duration::from_secs(1), pending)
				.await
				.unwrap();
		}
	}

	#[tokio::test]
	async fn recipient_http_wait_checks_membership_offline_and_abandon_before_network_poll() {
		let (members, member_updates) = watch::channel(2u64);
		let (online, availability) = watch::channel(true);
		let scope = (model::Id(2), 7);
		let (takeover, ownership) = watch::channel(None);
		let (control, calls, channels) = recipient_worker_fixture(false);
		let calls = Arc::new(Mutex::new(calls));
		let channels = Arc::new(Mutex::new(channels));
		let task = tokio::spawn(wait_for_recipient_invalidation(
			member_updates.clone(),
			availability.clone(),
			control,
			calls.clone(),
			channels.clone(),
		));
		tokio::task::yield_now().await;
		assert!(!task.is_finished());
		calls.lock().unwrap().active = None;
		members.send_replace(3);
		tokio::time::timeout(Duration::from_secs(1), task)
			.await
			.unwrap()
			.unwrap();
		for invalidation in 0..3 {
			let (control, calls, channels) = recipient_worker_fixture(false);
			let calls = Arc::new(Mutex::new(calls));
			let channels = Arc::new(Mutex::new(channels));
			if invalidation == 0 {
				channels.lock().unwrap().clear();
			}
			members.send_replace(if invalidation == 0 { 3 } else { 2 });
			online.send_replace(invalidation != 1);
			takeover.send_replace((invalidation == 2).then_some(scope));
			let contacted = std::cell::Cell::new(false);
			tokio::select! {
				biased;
				_=wait_for_takeover(ownership.clone(),scope)=>{},
				_=wait_for_recipient_invalidation(member_updates.clone(),availability.clone(),control,calls.clone(),channels.clone())=>{},
				_=async {contacted.set(true);std::future::pending::<()>().await}=>unreachable!(),
			}
			assert!(
				!contacted.get(),
				"invalidated recipient action must not poll HTTP"
			);
		}
		// A task already pending when local Abandon publishes the same invalidation cancels.
		takeover.send_replace(None);
		let task = tokio::spawn(wait_for_takeover(ownership, scope));
		tokio::task::yield_now().await;
		assert!(!task.is_finished());
		takeover.send_replace(Some(scope));
		tokio::time::timeout(Duration::from_secs(1), task)
			.await
			.unwrap()
			.unwrap();
	}

	#[test]
	fn call_discovery_never_rings_or_changes_the_active_attempt() {
		use client_core::voice::Command as V;
		for dm in [false, true] {
			for mut active in [None, Some((model::Id(20), 7, false))] {
				let before = active;
				assert_eq!(
					ring_action(
						V::Sync {
							channel: model::Id(2)
						},
						model::Id(1),
						&mut active,
						dm
					),
					Ok(None)
				);
				assert_eq!(active, before);
			}
		}
	}
	#[test]
	fn ringing_waits_for_transport_confirmation_and_rejects_old_requests() {
		use client_core::voice::Command as V;
		let mut active = None;
		let channel = model::Id(2);
		let owner = model::Id(1);
		assert_eq!(
			ring_action(
				V::Join {
					channel,
					request: 7,
					ring: true,
					mute: false,
					deaf: false,
				},
				owner,
				&mut active,
				true
			),
			Ok(None)
		);
		assert!(
			ring_action(
				V::Ring {
					channel,
					request: 6
				},
				owner,
				&mut active,
				true
			)
			.is_err()
		);
		assert_eq!(
			ring_action(
				V::Ring {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			),
			Ok(Some((None, false)))
		);
		assert!(
			ring_action(
				V::Ring {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			)
			.is_err()
		);
		assert_eq!(
			ring_action(
				V::Leave {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			),
			Ok(Some((None, true)))
		);
		assert!(
			ring_action(
				V::Ring {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			)
			.is_err()
		);
		assert_eq!(
			ring_action(
				V::Join {
					channel,
					request: 8,
					ring: false,
					mute: false,
					deaf: false,
				},
				owner,
				&mut active,
				true
			),
			Ok(None)
		);
		assert_eq!(
			ring_action(
				V::Leave {
					channel,
					request: 7
				},
				owner,
				&mut active,
				true
			),
			Ok(None)
		);
		assert_eq!(active, Some((channel, 8, true)));
		assert!(
			ring_action(
				V::Ring {
					channel,
					request: 8
				},
				owner,
				&mut active,
				true
			)
			.is_err()
		);
	}
	#[test]
	fn guild_join_mute_leave_never_ring_a_dm() {
		use client_core::voice::Command as V;
		let mut active = None;
		let channel = model::Id(20);
		let owner = model::Id(1);
		for control in [
			V::Join {
				channel,
				request: 1,
				ring: false,
				mute: false,
				deaf: false,
			},
			V::SetMute {
				channel,
				request: 1,
				mute: true,
				deaf: false,
			},
			V::Leave {
				channel,
				request: 1,
			},
		] {
			assert_eq!(ring_action(control, owner, &mut active, false), Ok(None));
		}
		assert!(
			ring_action(
				V::Ring {
					channel,
					request: 1
				},
				owner,
				&mut active,
				false
			)
			.is_err()
		);
		assert!(ring_action(V::Decline { channel }, owner, &mut active, false).is_err());
		assert!(active.is_none());
	}
	#[test]
	fn reads_scope_permission_errors_but_expiry_stays_global() {
		assert!(matches!(
			scope_history_failure(Event::Unavailable(model::Id(1)), model::Id(1), 9),
			Event::HistoryFailed {
				channel: model::Id(1),
				request: 9,
				failure: Failure::Forbidden
			}
		));
		assert!(matches!(
			scope_history_failure(Event::Failure(Failure::Network), model::Id(1), 9),
			Event::HistoryFailed {
				request: 9,
				failure: Failure::Network,
				..
			}
		));
		assert!(matches!(
			scope_history_failure(Event::Failure(Failure::Expired), model::Id(1), 9),
			Event::Failure(Failure::Expired)
		));
	}
}
