//! Existing bounded read, mutation and speculative-work lanes.
use super::{
	AbortTask, Command, Event, Failure, JoinHandle, PREFETCHES, ReadKey, ReadScopes, watch,
};

/// Community retains its Stage/call cancellation worker, but all administrative
/// mutations share one admission slot across that worker and the general lane.
pub(super) fn admin_mutation_busy(
	administrative: &Option<AbortTask>,
	community: &Option<AbortTask>,
) -> bool {
	[administrative, community]
		.iter()
		.any(|job| job.as_ref().is_some_and(|task| !task.0.is_finished()))
}

/// Revalidate the account, guild and channel scope immediately before an unsent PATCH.
pub(super) fn admit_preference(
	request: Box<client_core::preferences_sync::Request>,
	scope: Option<(u64, model::Id, u64)>,
) -> Result<Box<client_core::preferences_sync::Request>, Box<client_core::preferences_sync::Finished>>
{
	if scope == Some((request.generation, request.guild, request.request)) {
		Ok(request)
	} else {
		Err(Box::new(client_core::preferences_sync::Finished {
			generation: request.generation,
			guild: request.guild,
			request: request.request,
			result: Err(Failure::ProtocolAt(
				client_core::preferences_sync::NOT_DISPATCHED,
			)),
		}))
	}
}

/// A queued preference write waits without a polling timer or a second mutation owner.
pub(super) async fn admin_mutation_finished(
	administrative: &mut Option<AbortTask>,
	community: &mut Option<AbortTask>,
) {
	let administrative = administrative.as_mut().filter(|task| !task.0.is_finished());
	let community = community.as_mut().filter(|task| !task.0.is_finished());
	match (administrative, community) {
		(Some(a), Some(c)) => {
			tokio::select! {_=&mut a.0=>{},_=&mut c.0=>{}}
		}
		(Some(a), None) => {
			let _ = (&mut a.0).await;
		}
		(None, Some(c)) => {
			let _ = (&mut c.0).await;
		}
		(None, None) => {}
	}
}

pub(super) fn mfa_job_available(
	job: &Option<AbortTask>,
	request: &client_core::mfa::Request,
	resume_ready: bool,
) -> bool {
	job.as_ref().is_none_or(|task| task.0.is_finished())
		|| matches!(request.action, client_core::mfa::Action::Cancel)
		|| matches!(request.action, client_core::mfa::Action::Resume) && resume_ready
}

/// At most two replaceable reads. This container never owns a mutation task.
#[derive(Default)]
pub(super) struct AdminReads {
	pub(super) slots: [Option<(ReadKey, AbortTask)>; 2],
}

impl AdminReads {
	pub(super) fn retire(&mut self, scopes: ReadScopes) {
		for slot in &mut self.slots {
			if slot
				.as_ref()
				.is_some_and(|(key, task)| !scopes.contains(*key) || task.0.is_finished())
			{
				*slot = None;
			}
		}
	}
	pub(super) fn replace(&mut self, key: ReadKey) -> bool {
		for slot in &mut self.slots {
			if slot.as_ref().is_some_and(|(old, _)| old.kind == key.kind) {
				*slot = None;
			}
		}
		self.slots.iter().any(Option::is_none)
	}
	pub(super) fn track(&mut self, key: ReadKey, task: JoinHandle<()>) {
		let slot = self
			.slots
			.iter_mut()
			.find(|slot| slot.is_none())
			.expect("read capacity was admitted before spawning");
		*slot = Some((key, AbortTask(task)));
	}
}

pub(super) async fn read_retired(scope: &mut watch::Receiver<ReadScopes>, key: ReadKey) {
	loop {
		let current = *scope.borrow_and_update();
		if !current.contains(key) || scope.changed().await.is_err() {
			return;
		}
	}
}

pub(super) async fn scoped_admin_read(
	operation: impl std::future::Future<Output = Event>,
	mut scope: watch::Receiver<ReadScopes>,
	key: ReadKey,
) -> Option<Event> {
	let event = tokio::select! {
		biased;
		_=read_retired(&mut scope,key)=>return None,
		event=operation=>event,
	};
	// Account revocation remains global even if the page was closed as it arrived.
	(scope.borrow().contains(key) || admin_read_failure(&event).is_some()).then_some(event)
}

pub(super) fn admin_read_failure(event: &Event) -> Option<Failure> {
	match event {
		Event::ServerAdmin(client_core::server_admin::Event {
			result: Err(failure),
			..
		})
		| Event::ServerSettings(client_core::server_settings::Event {
			result: Err(failure),
			..
		})
		| Event::Community(client_core::community::Event {
			result: Err(failure),
			..
		})
		| Event::ChannelAction(client_core::channel_actions::Event::Finished {
			result: Err(failure),
			..
		}) if failure.ends_session() => Some(*failure),
		_ => None,
	}
}

// Reads belong to the open Inbox view. Accepted writes belong to this connection,
// so closing/switching a tab cannot discard their confirmation or start a duplicate.
#[derive(Default)]
pub(super) struct InboxTasks {
	pub(super) read: Option<AbortTask>,
	pub(super) write: Option<AbortTask>,
}

impl InboxTasks {
	pub(super) fn cancel_read(&mut self) {
		drop(self.read.take());
	}
	pub(super) fn can_start(&self, write: bool) -> bool {
		!write || self.write.as_ref().is_none_or(|task| task.0.is_finished())
	}
	pub(super) fn track(&mut self, write: bool, task: JoinHandle<()>) {
		if write {
			self.write = Some(AbortTask(task));
		} else {
			self.read = Some(AbortTask(task));
		}
	}
}

pub(super) fn reject_prefetch_at_capacity(
	tasks: &mut Vec<AbortTask>,
	command: &Command,
) -> Option<Event> {
	tasks.retain(|task| !task.0.is_finished());
	if tasks.len() < PREFETCHES {
		return None;
	}
	// Accepted reads may already belong to a click; reject the newcomer with its own id.
	Some(match command {
		Command::History {
			channel, request, ..
		} => Event::HistoryFailed {
			channel: *channel,
			request: *request,
			failure: Failure::Capacity,
		},
		Command::Profile {
			user,
			guild,
			request,
			..
		} => Event::Profile {
			user: *user,
			guild: *guild,
			request: *request,
			result: Err(Failure::Capacity),
		},
		_ => Event::Failure(Failure::Capacity),
	})
}

#[cfg(test)]
mod preference_tests {
	use super::*;
	#[tokio::test]
	async fn queued_preferences_wait_for_either_existing_mutation_without_polling() {
		for community in [false, true] {
			let (release, held) = tokio::sync::oneshot::channel::<()>();
			let task = Some(AbortTask(tokio::spawn(async move {
				let _ = held.await;
			})));
			let (mut administrative, mut community_job) = if community {
				(None, task)
			} else {
				(task, None)
			};
			{
				let wait = admin_mutation_finished(&mut administrative, &mut community_job);
				tokio::pin!(wait);
				assert!(
					tokio::time::timeout(std::time::Duration::from_millis(20), &mut wait)
						.await
						.is_err()
				);
				release.send(()).unwrap();
				tokio::time::timeout(std::time::Duration::from_secs(1), &mut wait)
					.await
					.unwrap();
			}
			assert!(!admin_mutation_busy(&administrative, &community_job));
		}
	}
	#[tokio::test]
	async fn revoked_preference_scope_while_waiting_never_opens_an_api_connection() {
		use client_core::preferences_sync::Request;
		use model::{Id, guild_preferences::Write};
		use tokio::net::{TcpListener, TcpStream};
		for community in [false, true] {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let address = listener.local_addr().unwrap();
			let (release, held) = tokio::sync::oneshot::channel::<()>();
			let task = Some(AbortTask(tokio::spawn(async move {
				let _ = held.await;
			})));
			let (mut administrative, mut community_job) = if community {
				(None, task)
			} else {
				(task, None)
			};
			let request = Box::new(Request {
				generation: 7,
				guild: Id(2),
				request: 9,
				patch: Write::default(),
			});
			let (scope, mut updates) = watch::channel(Some((7, Id(2), 9)));
			assert!(admin_mutation_busy(&administrative, &community_job));
			scope.send_replace(None);
			tokio::select! {
				changed=updates.changed()=>assert!(changed.is_ok()),
				_=admin_mutation_finished(&mut administrative,&mut community_job)=>panic!("the synthetic mutation is still held"),
			}
			let admission = admit_preference(request, *updates.borrow_and_update());
			assert!(
				admission.is_err(),
				"guild/account/access revocation retires the unsent request"
			);
			if admission.is_ok() {
				let _ = TcpStream::connect(address).await.unwrap();
			}
			assert!(
				tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
					.await
					.is_err(),
				"no API connection is made for the revoked request"
			);
			assert!(
				admin_mutation_busy(&administrative, &community_job),
				"cancelling the preference must not cancel the other mutation"
			);
			release.send(()).unwrap();
			tokio::time::timeout(
				std::time::Duration::from_secs(1),
				admin_mutation_finished(&mut administrative, &mut community_job),
			)
			.await
			.unwrap();
		}
	}
	#[tokio::test]
	async fn conflicting_gateway_preferences_behind_a_mutation_never_issue_the_queued_patch() {
		use client_core::{Command, Envelope, Event, preferences_sync::Status};
		use model::{
			Id, Patch, channel_visibility as p,
			guild_preferences::{Fields, Row, Update},
		};
		use tokio::net::{TcpListener, TcpStream};
		for flags in [false, true] {
			let mut state = test_support::demo_state();
			if flags {
				state.apply(Envelope {
					generation: state.generation,
					event: Event::NotificationPreferences(
						client_core::notifications::Event::Updates {
							entries: vec![Update {
								guild: Some(Id(10)),
								fields: Fields {
									flags: Patch::Value(p::OPT_IN_CHANNELS_OFF),
									..Default::default()
								},
								..Default::default()
							}],
							replace: false,
						},
					),
				});
				let action = state.personal_show_all_change(Id(10), false).unwrap();
				state.request_personal_channel_action(Id(10), action);
			} else {
				state.request_channel_action(
					Id(20),
					client_core::channel_actions::Action::Notifications(1),
				);
			}
			state.flush_channel_preferences();
			let Some(Command::ChannelPreferences(request)) =
				state.poll_channel_preferences(std::time::Instant::now())
			else {
				panic!()
			};
			let initial = state.channel_preference_scope();
			assert!(initial.is_some());
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let address = listener.local_addr().unwrap();
			let (release, held) = tokio::sync::oneshot::channel::<()>();
			let mut administrative = Some(AbortTask(tokio::spawn(async move {
				let _ = held.await;
			})));
			let mut community = None;
			let (scope, mut updates) = watch::channel(initial);
			let update = if flags {
				Update {
					guild: Some(Id(10)),
					fields: Fields {
						flags: Patch::Value(0),
						..Default::default()
					},
					..Default::default()
				}
			} else {
				Update {
					guild: Some(Id(10)),
					channels: Some(vec![Row {
						channel: Id(20),
						fields: Fields {
							level: Patch::Value(0),
							..Default::default()
						},
					}]),
					..Default::default()
				}
			};
			state.apply(Envelope {
				generation: state.generation,
				event: Event::NotificationPreferences(client_core::notifications::Event::Updates {
					entries: vec![update],
					replace: false,
				}),
			});
			let target = if flags { None } else { Some(Id(20)) };
			assert_eq!(
				state.channel_preference_status(Id(10), target),
				Some(Status::Conflict)
			);
			assert_eq!(state.channel_preference_scope(), None);
			scope.send_replace(state.channel_preference_scope());
			tokio::select! {changed=updates.changed()=>assert!(changed.is_ok()),_=admin_mutation_finished(&mut administrative,&mut community)=>panic!("mutation remains held")}
			let admitted = admit_preference(request, *updates.borrow_and_update());
			match admitted {
				Ok(_) => {
					let _ = TcpStream::connect(address).await.unwrap();
					panic!("conflicting request must retire");
				}
				Err(event) => state.apply(Envelope {
					generation: state.generation,
					event: Event::ChannelPreferences(*event),
				}),
			}
			assert!(
				tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
					.await
					.is_err()
			);
			assert_eq!(
				state.channel_preference_status(Id(10), target),
				Some(Status::Conflict)
			);
			assert!(
				state
					.poll_channel_preferences(
						std::time::Instant::now() + std::time::Duration::from_secs(1)
					)
					.is_none()
			);
			assert!(admin_mutation_busy(&administrative, &community));
			release.send(()).unwrap();
			tokio::time::timeout(
				std::time::Duration::from_secs(1),
				admin_mutation_finished(&mut administrative, &mut community),
			)
			.await
			.unwrap();
		}
	}
}
