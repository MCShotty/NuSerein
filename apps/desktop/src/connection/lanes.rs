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
