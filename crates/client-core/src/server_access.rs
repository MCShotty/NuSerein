//! Permission-scoped application review, access configuration and service incident state.
use crate::{Envelope, Event, State};
use model::{
	Id, permissions as p,
	server_access::{Action, Application, Applications, Outcome, Settings, Status},
};

#[derive(Default)]
pub struct View {
	pub applications: Option<Applications>,
	pub detail: Option<Application>,
	pub settings: Option<Settings>,
	pub recipient: Option<Box<model::server_admin::Member>>,
	pub interview: Option<Id>,
	pub pending_count: Option<u64>,
	pub revision: u64,
	pub applications_changed: bool,
	pub removed_application: Option<Id>,
	pub(crate) applications_revision: u64,
	pub(crate) requested_applications_revision: u64,
}
impl View {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.applications.as_ref().map_or(0, Applications::bytes)
			+ self.detail.as_ref().map_or(0, Application::bytes)
			+ self.settings.as_ref().map_or(0, Settings::bytes)
			+ self.recipient.as_ref().map_or(0, |v| v.bytes())
	}
	pub(crate) fn invalidate(&mut self) {
		*self = Self {
			revision: self.revision.wrapping_add(1),
			..Self::default()
		};
	}
	fn application(&self, id: Id) -> Option<&Application> {
		self.detail.as_ref().filter(|v| v.id == id).or_else(|| {
			self.applications
				.as_ref()?
				.items
				.iter()
				.find(|v| v.id == id)
		})
	}
}
impl State {
	pub fn can_review_guild_applications(&self, guild: Id) -> bool {
		self.guild_permission(guild, p::KICK_MEMBERS)
	}
	pub fn owns_guild(&self, guild: Id) -> bool {
		self.user.as_ref().is_some_and(|user| {
			self.permissions
				.guilds
				.get(&guild)
				.is_some_and(|g| g.owner == Some(user.id))
		})
	}
	pub fn guild_application_count(&self, guild: Id) -> Option<u64> {
		(self.server_admin.guild == Some(guild) && self.can_review_guild_applications(guild))
			.then_some(self.server_admin.access.pending_count)
			.flatten()
	}
	pub fn guild_incident(&self, guild: Id) -> Option<model::server_access::Incident> {
		self.guild(guild)
			.and_then(|_| self.server_incidents.get(&guild).copied())
	}
	pub(crate) fn apply_server_incident(
		&mut self,
		guild: Id,
		incident: Option<model::server_access::Incident>,
	) {
		if self.guild(guild).is_none() || incident.is_some_and(|v| !v.valid()) {
			return;
		}
		if self.server_admin.guild == Some(guild)
			&& let Some(settings) = &mut self.server_admin.access.settings
		{
			settings.incident = Some(incident.unwrap_or_default());
			self.server_admin.access.revision = self.server_admin.access.revision.wrapping_add(1);
		}
		let now = now();
		if let Some(incident) = incident.filter(|v| v.active(now)) {
			const MAX: usize = 256;
			if !self.server_incidents.contains_key(&guild) && self.server_incidents.len() >= MAX {
				let selected = self
					.selected
					.and_then(|id| self.channel(id))
					.and_then(|c| c.guild);
				let evict = self
					.server_incidents
					.iter()
					.find_map(|(id, incident)| (!incident.active(now)).then_some(*id))
					.or_else(|| {
						self.server_incidents
							.keys()
							.copied()
							.find(|id| Some(*id) != selected)
					})
					.or_else(|| self.server_incidents.keys().next().copied());
				if let Some(id) = evict {
					self.server_incidents.remove(&id);
				}
			}
			self.server_incidents.insert(guild, incident);
		} else {
			self.server_incidents.remove(&guild);
		}
	}
	pub(crate) fn guild_application_changed(&mut self, guild: Id, request: Id, deleted: bool) {
		if self.server_admin.guild != Some(guild) || !self.can_review_guild_applications(guild) {
			return;
		}
		let view = &mut self.server_admin.access;
		view.applications_revision = view.applications_revision.wrapping_add(1);
		view.pending_count = None;
		view.applications_changed = true;
		if deleted {
			if let Some(page) = &mut view.applications {
				page.items.retain(|v| v.id != request);
				page.total = None;
				page.next = None;
			}
			view.removed_application = Some(request);
		}
		if view.detail.as_ref().is_some_and(|v| v.id == request) {
			view.detail = None;
		}
		view.revision = view.revision.wrapping_add(1);
	}
	pub(super) fn access_action_allowed(&self, guild: Id, action: &Action) -> bool {
		if !action.valid() {
			return false;
		}
		let view = &self.server_admin.access;
		let scoped = self.server_admin.guild == Some(guild);
		match action {
			Action::Applications { status, before } => {
				self.can_review_guild_applications(guild)
					&& before.is_none_or(|id| {
						scoped
							&& view.applications.as_ref().is_some_and(|v| {
								v.guild == guild && v.status == *status && v.next == Some(id)
							})
					})
			}
			Action::Application(id) => {
				self.can_review_guild_applications(guild)
					&& scoped && view.application(*id).is_some()
			}
			Action::Decide { request, .. } | Action::Interview { request } => {
				self.can_review_guild_applications(guild)
					&& scoped && !view.applications_changed
					&& view
						.application(*request)
						.is_some_and(|v| v.guild == guild && v.status == Status::Pending)
			}
			Action::Settings => self.can_manage_guild(guild),
			Action::Incident { before, after } => {
				self.can_manage_guild(guild)
					&& scoped && after.writable(now())
					&& view
						.settings
						.as_ref()
						.is_some_and(|v| v.guild == guild && v.incident == Some(*before))
			}
			Action::Configure { before, after } => {
				self.can_manage_guild(guild)
					&& scoped && view.settings.as_ref().is_some_and(|v| {
					v.guild == guild
						&& v.configuration == *before
						&& (!after.community || v.verification_level >= 1 && v.content_filter == 2)
				}) && [after.rules, after.updates, after.alerts]
					.into_iter()
					.flatten()
					.all(|id| {
						self.channel(id)
							.is_some_and(|c| c.guild == Some(guild) && matches!(c.kind, 0 | 5))
							&& self.can_view(id)
					})
			}
			Action::MfaLevel { before, .. } => {
				self.owns_guild(guild)
					&& scoped && view
					.settings
					.as_ref()
					.is_some_and(|v| v.guild == guild && v.mfa_level == *before)
			}
			Action::Recipient(id) => {
				self.owns_guild(guild) && self.user.as_ref().is_some_and(|u| u.id != *id)
			}
			Action::Transfer {
				before_owner,
				recipient,
			} => {
				self.owns_guild(guild)
					&& scoped && self.user.as_ref().is_some_and(|u| u.id == *before_owner)
					&& view
						.settings
						.as_ref()
						.is_some_and(|v| v.guild == guild && v.owner == *before_owner)
					&& view.recipient.as_ref().is_some_and(|v| {
						v.user.id == *recipient
							&& v.user.kind == model::AccountKind::Human
							&& !v.user.webhook
					})
			}
		}
	}
	pub(super) fn access_response_expected(
		&self,
		guild: Id,
		action: &Action,
		outcome: &Outcome,
	) -> bool {
		if !outcome.valid() {
			return false;
		}
		match (action, outcome) {
			(Action::Applications { status, .. }, Outcome::Applications(v)) => {
				v.guild == guild && v.status == *status
			}
			(Action::Application(id), Outcome::Application(v)) => v.guild == guild && v.id == *id,
			(
				Action::Decide {
					request,
					status,
					reason,
				},
				Outcome::Application(v),
			) => {
				v.guild == guild
					&& v.id == *request
					&& v.status == *status
					&& (*status != Status::Rejected
						|| reason.is_empty()
						|| v.reason.as_deref() == Some(reason.as_str()))
			}
			(Action::Interview { request }, Outcome::Interview { application, .. }) => {
				application.guild == guild && application.id == *request
			}
			(Action::Settings, Outcome::Settings(v)) => v.guild == guild,
			(Action::Incident { after, .. }, Outcome::Settings(v)) => {
				v.guild == guild && v.incident == Some(*after)
			}
			(Action::Configure { after, .. }, Outcome::Settings(v)) => {
				v.guild == guild && v.configuration == *after
			}
			(Action::MfaLevel { after, .. }, Outcome::Settings(v)) => {
				v.guild == guild && v.mfa_level == *after
			}
			(Action::Transfer { recipient, .. }, Outcome::Settings(v)) => {
				v.guild == guild && v.owner == *recipient
			}
			(Action::Recipient(id), Outcome::Recipient(v)) => {
				v.user.id == *id && v.user.kind == model::AccountKind::Human && !v.user.webhook
			}
			_ => false,
		}
	}
	pub(super) fn apply_server_access(
		&mut self,
		guild: Id,
		action: &Action,
		outcome: Outcome,
	) -> Result<(), &'static str> {
		let view = &self.server_admin.access;
		if matches!(action, Action::Applications { .. } | Action::Application(_))
			&& view.applications_revision != view.requested_applications_revision
		{
			self.server_admin.error =
				Some("Applications changed while loading; reload before continuing");
			return Ok(());
		}
		if matches!(action, Action::Decide { request, .. } | Action::Interview { request } if view.removed_application == Some(*request))
		{
			self.server_admin.error =
				Some("This application was removed; reload before continuing");
			return Ok(());
		}
		if !self.access_response_expected(guild, action, &outcome) {
			return Err("Unexpected server access result");
		}
		match outcome {
			Outcome::Applications(v) => {
				if v.status == Status::Pending {
					self.server_admin.access.pending_count = v.total;
				}
				self.server_admin.access.applications_changed = false;
				self.server_admin.access.removed_application = None;
				self.server_admin.access.applications = Some(v);
				self.server_admin.access.detail = None;
			}
			Outcome::Application(v) => {
				let old = self.server_admin.access.application(v.id).map(|v| v.status);
				if old == Some(Status::Pending) && v.status != Status::Pending {
					self.server_admin.access.pending_count = self
						.server_admin
						.access
						.pending_count
						.map(|n| n.saturating_sub(1));
				}
				if let Some(page) = &mut self.server_admin.access.applications {
					if page.status != v.status {
						page.items.retain(|a| a.id != v.id);
						page.total = page.total.map(|n| n.saturating_sub(1));
						page.next = None;
					} else if let Some(row) = page.items.iter_mut().find(|a| a.id == v.id) {
						*row = v.clone();
					}
				}
				self.server_admin.access.detail = Some(v);
			}
			Outcome::Interview {
				application,
				channel,
			} => {
				let id = channel.id;
				self.apply(Envelope {
					generation: self.generation,
					event: Event::ChannelCreated(*channel),
				});
				if !self
					.channel(id)
					.is_some_and(|c| c.kind == 3 && c.guild.is_none())
				{
					return Err("The interview channel could not be admitted");
				}
				self.server_admin.access.detail = Some(application);
				self.server_admin.access.interview = Some(id);
			}
			Outcome::Settings(v) => {
				if let Some(incident) = v.incident {
					self.apply_server_incident(guild, Some(incident));
				}
				let owner = v.owner;
				if matches!(action, Action::Transfer { .. }) {
					self.apply(Envelope {
						generation: self.generation,
						event: Event::Permissions(crate::permissions::Event::Owner {
							guild,
							owner: model::Patch::Value(owner),
						}),
					});
					self.server_admin.access.recipient = None;
					self.status = "Server ownership transferred";
				}
				if self.can_manage_guild(guild) {
					self.server_admin.access.settings = Some(v);
				}
			}
			Outcome::Recipient(v) => self.server_admin.access.recipient = Some(v),
		}
		self.server_admin.access.revision = self.server_admin.access.revision.wrapping_add(1);
		if self.server_admin.access.bytes() > model::server_access::MAX_VIEW_BYTES {
			self.server_admin.access.invalidate();
			return Err("Server access exceeded its memory budget");
		}
		Ok(())
	}
}

fn now() -> i128 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_nanos() as i128
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Command, server_admin};
	fn state(bits: u128) -> State {
		crate::community_admin::tests::state(bits)
	}
	fn application() -> Application {
		Application {
			id: Id(10),
			guild: Id(2),
			user_id: Id(3),
			user: None,
			status: Status::Pending,
			created_at: 1_000_000_000,
			reviewed_at: None,
			moderator: None,
			reason: None,
			answers: Vec::new(),
			interview: None,
		}
	}
	fn dispatch(state: &mut State, action: Action) -> u64 {
		let Command::ServerAdmin { request, .. } = state
			.request_server_admin(Id(2), model::server_admin::Action::Access(action))
			.unwrap()
		else {
			panic!("expected scoped server command")
		};
		request
	}
	fn apply(state: &mut State, request: u64, outcome: Outcome) {
		state.apply(Envelope {
			generation: state.generation,
			event: Event::ServerAdmin(server_admin::Event {
				guild: Id(2),
				request,
				result: Ok(model::server_admin::Result::Access(outcome)),
			}),
		});
	}
	fn load_applications(state: &mut State) {
		let request = dispatch(
			state,
			Action::Applications {
				status: Status::Pending,
				before: None,
			},
		);
		apply(
			state,
			request,
			Outcome::Applications(Applications {
				guild: Id(2),
				status: Status::Pending,
				items: vec![application()],
				total: Some(1),
				next: None,
			}),
		);
	}
	#[test]
	fn limited_moderators_can_review_applications_without_server_configuration_access() {
		let mut state = state(p::KICK_MEMBERS);
		assert!(!state.can_manage_guild(Id(2)));
		assert!(state.can_review_guild_applications(Id(2)));
		load_applications(&mut state);
		assert_eq!(state.guild_application_count(Id(2)), Some(1));
		assert!(!state.access_action_allowed(Id(2), &Action::Application(Id(3))));
		let request = dispatch(&mut state, Action::Application(Id(10)));
		apply(
			&mut state,
			request.wrapping_sub(1),
			Outcome::Application(application()),
		);
		assert!(state.server_admin.pending && state.server_admin.access.detail.is_none());
		apply(&mut state, request, Outcome::Application(application()));
		assert!(state.server_admin.access.detail.is_some());
		assert!(!state.access_action_allowed(Id(2), &Action::Settings));
	}
	#[test]
	fn withdrawal_and_permission_revocation_retire_sensitive_results_and_counts() {
		let mut state = state(p::KICK_MEMBERS);
		load_applications(&mut state);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::GuildApplicationChanged {
				guild: Id(2),
				request: Id(10),
				deleted: true,
			},
		});
		assert_eq!(state.guild_application_count(Id(2)), None);
		assert!(
			state
				.server_admin
				.access
				.applications
				.as_ref()
				.unwrap()
				.items
				.is_empty()
		);
		assert!(!state.access_action_allowed(
			Id(2),
			&Action::Decide {
				request: Id(10),
				status: Status::Approved,
				reason: String::new()
			}
		));
		let request = dispatch(
			&mut state,
			Action::Applications {
				status: Status::Pending,
				before: None,
			},
		);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::Member {
				guild: Id(2),
				roles: model::Patch::Value(Vec::new()),
				timeout_until: model::Patch::Null,
			}),
		});
		apply(
			&mut state,
			request,
			Outcome::Applications(Applications {
				guild: Id(2),
				status: Status::Pending,
				items: vec![application()],
				total: Some(1),
				next: None,
			}),
		);
		assert!(state.server_admin.access.applications.is_none());
	}
	#[test]
	fn gateway_withdrawal_cannot_be_undone_by_a_late_http_detail_or_page() {
		let mut state = state(p::KICK_MEMBERS);
		load_applications(&mut state);
		let request = dispatch(&mut state, Action::Application(Id(10)));
		state.apply(Envelope {
			generation: state.generation,
			event: Event::GuildApplicationChanged {
				guild: Id(2),
				request: Id(10),
				deleted: true,
			},
		});
		apply(&mut state, request, Outcome::Application(application()));
		assert!(state.server_admin.access.detail.is_none());
		assert!(
			state
				.server_admin
				.access
				.applications
				.as_ref()
				.unwrap()
				.items
				.is_empty()
		);
		assert_eq!(state.server_admin.access.removed_application, Some(Id(10)));
		assert!(state.server_admin.error.is_some());
		let request = dispatch(
			&mut state,
			Action::Applications {
				status: Status::Pending,
				before: None,
			},
		);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::GuildApplicationChanged {
				guild: Id(2),
				request: Id(11),
				deleted: false,
			},
		});
		apply(
			&mut state,
			request,
			Outcome::Applications(Applications {
				guild: Id(2),
				status: Status::Pending,
				items: vec![application()],
				total: Some(1),
				next: None,
			}),
		);
		assert!(
			state
				.server_admin
				.access
				.applications
				.as_ref()
				.unwrap()
				.items
				.is_empty()
		);
		assert!(state.server_admin.access.applications_changed);
		assert!(!state.server_admin.pending);
	}
	#[test]
	fn confirmed_decision_does_not_erase_a_concurrent_application_change() {
		let mut state = state(p::KICK_MEMBERS);
		load_applications(&mut state);
		let request = dispatch(
			&mut state,
			Action::Decide {
				request: Id(10),
				status: Status::Approved,
				reason: String::new(),
			},
		);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::GuildApplicationChanged {
				guild: Id(2),
				request: Id(11),
				deleted: false,
			},
		});
		apply(
			&mut state,
			request,
			Outcome::Application(Application {
				status: Status::Approved,
				..application()
			}),
		);
		assert_eq!(
			state.server_admin.access.detail.as_ref().unwrap().status,
			Status::Approved
		);
		assert!(state.server_admin.access.applications_changed);
		assert_eq!(state.guild_application_count(Id(2)), None);
		assert!(state.server_admin.error.is_none());
	}
	#[test]
	fn confirmed_owner_transfer_survives_its_own_gateway_permission_revocation() {
		let mut state = state(0);
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(1));
		state.permissions.clear_cache();
		let settings = Settings {
			guild: Id(2),
			owner: Id(1),
			features: Vec::new(),
			configuration: Default::default(),
			mfa_level: 0,
			verification_level: 0,
			content_filter: 0,
			incident: Some(Default::default()),
		};
		let request = dispatch(&mut state, Action::Settings);
		apply(&mut state, request, Outcome::Settings(settings.clone()));
		let user = model::User {
			id: Id(3),
			name: "Recipient".into(),
			username: None,
			avatar: None,
			discriminator: 0,
			kind: Default::default(),
			webhook: false,
			primary_guild: None,
			cosmetics: None,
		};
		state.server_admin.access.recipient = Some(Box::new(model::server_admin::Member {
			user,
			nick: None,
			roles: Vec::new(),
			joined_at: None,
			join_source: None,
			invite_code: None,
			flags: None,
			unusual_dm_until: None,
			timeout_until: None,
		}));
		let request = dispatch(
			&mut state,
			Action::Transfer {
				before_owner: Id(1),
				recipient: Id(3),
			},
		);
		state.apply(Envelope {
			generation: state.generation,
			event: Event::Permissions(crate::permissions::Event::Owner {
				guild: Id(2),
				owner: model::Patch::Value(Id(3)),
			}),
		});
		assert!(!state.owns_guild(Id(2)));
		assert!(state.server_admin.transfer_pending(Id(2)));
		apply(
			&mut state,
			request,
			Outcome::Settings(Settings {
				owner: Id(3),
				..settings
			}),
		);
		assert_eq!(state.status, "Server ownership transferred");
		assert!(!state.server_admin.pending);
		assert!(state.server_admin.access.recipient.is_none());
	}
	#[test]
	fn incident_capacity_evicts_inactive_entries_and_logout_clears_them() {
		let mut state = state(0);
		let template = state.guilds[0].clone();
		let incident = model::server_access::Incident {
			invites_until: Some(now() + 1_000_000_000),
			dms_until: None,
		};
		for id in 100..400 {
			let mut guild = template.clone();
			guild.id = Id(id);
			state.guilds.push(guild);
			state.apply_server_incident(Id(id), Some(incident));
		}
		assert_eq!(state.server_incidents.len(), 256);
		state.apply_server_incident(Id(399), None);
		assert!(!state.server_incidents.contains_key(&Id(399)));
		state.logout();
		assert!(state.server_incidents.is_empty());
	}
	#[test]
	fn incident_capacity_prefers_expired_entries_over_active_servers() {
		let mut state = state(0);
		let template = state.guilds[0].clone();
		let incident = model::server_access::Incident {
			invites_until: Some(now() + 60_000_000_000),
			dms_until: None,
		};
		for id in 100..357 {
			let mut guild = template.clone();
			guild.id = Id(id);
			state.guilds.push(guild);
			if id < 356 {
				state.apply_server_incident(Id(id), Some(incident));
			}
		}
		state
			.server_incidents
			.get_mut(&Id(300))
			.unwrap()
			.invites_until = Some(1);
		state.apply_server_incident(Id(356), Some(incident));
		assert_eq!(state.server_incidents.len(), 256);
		assert!(!state.server_incidents.contains_key(&Id(300)));
		assert!(state.server_incidents.contains_key(&Id(100)));
		assert!(state.server_incidents.contains_key(&Id(356)));
	}
}
