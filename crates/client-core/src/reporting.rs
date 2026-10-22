//! One explicit report flow. Navigating reasons never submits a report.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id,
	reporting::{Menu, Submission, Target},
};
use std::{
	collections::BTreeMap,
	time::{Duration, Instant},
};

pub struct Request {
	pub target: Target,
	pub request: u64,
	pub submission: Option<Box<Submission>>,
}
pub enum Outcome {
	Menu(Box<Menu>),
	Sent(Id),
}
impl Outcome {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Menu(menu) => menu.bytes(),
			Self::Sent(_) => 0,
		}
	}
}
pub struct Event {
	pub target: Target,
	pub request: u64,
	pub result: Result<Outcome, Failure>,
}

#[derive(Default)]
pub struct View {
	pub target: Option<Target>,
	pub menu: Option<Box<Menu>>,
	pub breadcrumbs: Vec<u32>,
	pub values: BTreeMap<String, Vec<String>>,
	pub loading: bool,
	pub submitting: bool,
	pub sent: Option<Id>,
	pub error: Option<Failure>,
	sequence: u64,
	started: Option<Instant>,
}
impl View {
	pub fn close(&mut self) {
		*self = Self {
			sequence: self.sequence.wrapping_add(1),
			..Self::default()
		};
	}
}
impl State {
	pub fn can_report_message(&self, target: Target) -> bool {
		(self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
			&& self.selected == Some(target.channel)
			&& self.can_read_history(target.channel)
			&& !self.timeline.is_deleted(target.message)
			&& self.timeline.get(target.message).is_some_and(|message| {
				message.channel == target.channel
					&& !message.ephemeral
					&& self
						.user
						.as_ref()
						.is_some_and(|user| user.id != message.author.id)
			})
	}
	pub fn sync_reporting(&mut self) {
		let Some(target) = self.reporting.target else {
			return;
		};
		if !self.can_report_message(target) {
			self.reporting.close();
		} else if self
			.reporting
			.started
			.is_some_and(|at| at.elapsed() > Duration::from_secs(600))
		{
			self.reporting.sequence = self.reporting.sequence.wrapping_add(1);
			self.reporting.menu = None;
			self.reporting.loading = false;
			self.reporting.submitting = false;
			self.reporting.started = None;
			self.reporting.values.clear();
			self.reporting.breadcrumbs.clear();
			self.reporting.error = Some(Failure::ProtocolAt(
				"This report form expired; reopen it or use Discord",
			));
		}
	}
	pub fn open_report(&mut self, target: Target) -> Option<Command> {
		if !self.can_report_message(target) {
			return None;
		}
		self.reporting.close();
		self.reporting.target = Some(target);
		self.reporting.loading = true;
		self.reporting.started = Some(Instant::now());
		Some(Command::Report(Box::new(Request {
			target,
			request: self.reporting.sequence,
			submission: None,
		})))
	}
	pub fn report_node(&self) -> Option<&model::reporting::Node> {
		self.reporting
			.menu
			.as_ref()?
			.nodes
			.get(self.reporting.breadcrumbs.last()?)
	}
	pub fn choose_report_node(&mut self, target: u32) {
		if self.reporting.loading || self.reporting.sent.is_some() {
			return;
		}
		let Some(menu) = &self.reporting.menu else {
			return;
		};
		if !menu.validate_values(&self.reporting.breadcrumbs, &self.reporting.values) {
			return;
		}
		let mut path = self.reporting.breadcrumbs.clone();
		path.push(target);
		if menu.valid_path(&path) {
			self.reporting.breadcrumbs = path;
		}
	}
	pub fn report_back(&mut self) {
		if self.reporting.loading
			|| self.reporting.breadcrumbs.len() <= 1
			|| self.reporting.sent.is_some()
		{
			return;
		}
		self.reporting.breadcrumbs.pop();
		if let Some(menu) = &self.reporting.menu {
			self.reporting.values.retain(|name, _| {
				self.reporting.breadcrumbs.iter().any(|id| {
					menu.nodes[id]
						.elements
						.iter()
						.any(|e| e.submit && e.name == *name)
				})
			});
		}
	}
	pub fn submit_report(&mut self) -> Option<Command> {
		self.sync_reporting();
		let target = self.reporting.target?;
		if self.reporting.loading
			|| self.reporting.sent.is_some()
			|| self.reporting.error == Some(Failure::Ambiguous)
		{
			return None;
		}
		let menu = self.reporting.menu.as_ref()?;
		let submission = Submission {
			version: menu.version.clone(),
			variant: menu.variant.clone(),
			language: menu.language.clone(),
			breadcrumbs: self.reporting.breadcrumbs.clone(),
			elements: self.reporting.values.clone(),
		};
		if !submission.valid_for(menu) {
			return None;
		}
		self.reporting.sequence = self.reporting.sequence.wrapping_add(1);
		self.reporting.loading = true;
		self.reporting.submitting = true;
		self.reporting.error = None;
		Some(Command::Report(Box::new(Request {
			target,
			request: self.reporting.sequence,
			submission: Some(Box::new(submission)),
		})))
	}
	pub fn report_command_allowed(&self, request: &Request) -> bool {
		self.can_report_message(request.target)
			&& self.reporting.target == Some(request.target)
			&& self.reporting.sequence == request.request
			&& self.reporting.loading
			&& self
				.reporting
				.started
				.is_some_and(|at| at.elapsed() <= Duration::from_secs(600))
			&& match &request.submission {
				None => !self.reporting.submitting,
				Some(submission) => {
					self.reporting.submitting
						&& self
							.reporting
							.menu
							.as_ref()
							.is_some_and(|menu| submission.valid_for(menu))
						&& submission.breadcrumbs == self.reporting.breadcrumbs
						&& submission.elements == self.reporting.values
				}
			}
	}
	pub(crate) fn apply_report(&mut self, event: Event) {
		if self.reporting.target != Some(event.target)
			|| self.reporting.sequence != event.request
			|| !self.reporting.loading
		{
			return;
		}
		self.sync_reporting();
		if self.reporting.target != Some(event.target) || self.reporting.sequence != event.request {
			return;
		}
		self.reporting.loading = false;
		match event.result {
			Ok(Outcome::Menu(menu)) if !self.reporting.submitting && menu.valid() => {
				self.reporting.breadcrumbs = vec![menu.root];
				self.reporting.menu = Some(menu);
			}
			Ok(Outcome::Sent(id)) if self.reporting.submitting && id.0 > 0 => {
				self.reporting.sent = Some(id);
				self.reporting.menu = None;
				self.reporting.values.clear();
			}
			Err(failure) if failure.ends_session() => {
				self.reporting.close();
				self.fail(failure);
			}
			Err(failure) => self.reporting.error = Some(failure),
			_ => {
				self.reporting.error = Some(if self.reporting.submitting {
					Failure::Ambiguous
				} else {
					Failure::Protocol
				})
			}
		}
	}
}
