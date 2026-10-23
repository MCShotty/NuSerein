//! One owner-completed verification for an already confirmed administrative operation.
//! The transport suspends the rejected HTTP request, so resuming cannot replay earlier writes.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::Id;
use std::{
	fmt,
	time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub const LIFETIME: Duration = Duration::from_secs(300);
pub const MAX_METHODS: usize = 8;
pub const MAX_SECRET_BYTES: usize = 1024;
/// Conservative admission bound for a cloned, validated administrative command.
pub const MAX_TARGET_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
	Totp,
	Backup,
	BackupViaTotp,
	Sms,
	Password,
	Passkey,
	Unsupported,
}
impl Method {
	pub fn supported(self) -> bool {
		!matches!(self, Self::Passkey | Self::Unsupported)
	}
	pub fn wire(self) -> &'static str {
		match self {
			Self::Totp | Self::BackupViaTotp => "totp",
			Self::Backup => "backup",
			Self::Sms => "sms",
			Self::Password => "password",
			Self::Passkey => "webauthn",
			Self::Unsupported => "unsupported",
		}
	}
	pub fn label(self) -> &'static str {
		match self {
			Self::Totp => "Authenticator code",
			Self::Backup | Self::BackupViaTotp => "Backup code",
			Self::Sms => "SMS code",
			Self::Password => "Password",
			Self::Passkey => "Passkey (use Discord)",
			Self::Unsupported => "Another security method (use Discord)",
		}
	}
	pub fn accepts(self, value: &str) -> bool {
		!value.is_empty()
			&& value.len() <= MAX_SECRET_BYTES
			&& !value.chars().any(char::is_control)
			&& match self {
				Self::Totp | Self::Sms => {
					value.len() == 6 && value.bytes().all(|c| c.is_ascii_digit())
				}
				Self::Backup | Self::BackupViaTotp => {
					value.len() <= 32
						&& value
							.bytes()
							.all(|c| c.is_ascii_alphanumeric() || c == b'-')
				}
				Self::Password => true,
				Self::Passkey | Self::Unsupported => false,
			}
	}
}

/// Opaque input: never serialized by state/storage and never printed by Debug.
pub struct Input(Zeroizing<String>);
impl Input {
	pub fn new(method: Method, value: String) -> Option<Self> {
		let value = Zeroizing::new(value);
		(value.capacity() <= MAX_SECRET_BYTES && method.accepts(&value)).then_some(Self(value))
	}
	pub fn expose(&self) -> &str {
		&self.0
	}
	pub fn bytes(&self) -> usize {
		self.0.capacity()
	}
}
impl fmt::Debug for Input {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("MfaInput([REDACTED])")
	}
}

#[derive(Clone)]
pub enum Target {
	Admin {
		guild: Id,
		request: u64,
		action: Box<model::server_admin::Action>,
	},
	Settings {
		guild: Id,
		request: u64,
		edit: Box<model::server_settings::Edit>,
	},
	Delete {
		guild: Id,
		request: u64,
	},
	Channel {
		guild: Id,
		channel: Id,
		request: u64,
		action: Box<crate::channel_actions::Action>,
	},
	Community {
		guild: Id,
		user: Id,
		fence: u64,
		request: u64,
		action: Box<model::community::Action>,
	},
	Onboarding {
		guild: Id,
		request: u64,
		form: Box<model::onboarding::Form>,
		submission: Box<model::onboarding::Submission>,
	},
}
impl Target {
	pub fn from_command(command: &Command) -> Option<Self> {
		match command {
			Command::ServerAdmin {
				guild,
				request,
				action,
			} if action.write() && action.valid() => Some(Self::Admin {
				guild: *guild,
				request: *request,
				action: action.clone(),
			}),
			Command::ServerSettings {
				guild,
				request,
				edit: Some(edit),
			} if edit.valid() && !edit.is_empty() => Some(Self::Settings {
				guild: *guild,
				request: *request,
				edit: edit.clone(),
			}),
			Command::ServerAction {
				action: crate::server_actions::Action::Delete(guild),
				request,
			} => Some(Self::Delete {
				guild: *guild,
				request: *request,
			}),
			Command::ChannelAction {
				guild,
				channel,
				request,
				action,
			} if action.write() && action.valid() => Some(Self::Channel {
				guild: *guild,
				channel: *channel,
				request: *request,
				action: Box::new(action.clone()),
			}),
			Command::Community {
				guild,
				user,
				fence,
				request,
				action,
			} if action.write() && action.valid() => Some(Self::Community {
				guild: *guild,
				user: *user,
				fence: *fence,
				request: *request,
				action: action.clone(),
			}),
			Command::Onboarding {
				guild,
				request,
				action: crate::onboarding::Action::Submit { form, submission },
			} => Some(Self::Onboarding {
				guild: *guild,
				request: *request,
				form: form.clone(),
				submission: submission.clone(),
			}),
			_ => None,
		}
	}
	pub fn rejected(self, failure: Failure) -> crate::Event {
		match self {
			Self::Admin { guild, request, .. } => {
				crate::Event::ServerAdmin(crate::server_admin::Event {
					guild,
					request,
					result: Err(failure),
				})
			}
			Self::Settings { guild, request, .. } => {
				crate::Event::ServerSettings(crate::server_settings::Event {
					guild,
					request,
					result: Err(failure),
					refreshed: None,
				})
			}
			Self::Channel {
				guild,
				channel,
				request,
				..
			} => crate::Event::ChannelAction(crate::channel_actions::Event::Finished {
				guild,
				channel,
				request,
				result: Err(failure),
			}),
			Self::Community { guild, request, .. } => {
				crate::Event::Community(crate::community::Event {
					guild,
					request,
					result: Err(failure),
				})
			}
			Self::Onboarding { guild, request, .. } => {
				crate::Event::Onboarding(crate::onboarding::Event::Submitted {
					guild,
					request,
					result: Err(failure),
				})
			}
			Self::Delete { guild, request } => {
				crate::Event::ServerAction(crate::server_actions::Event::Written {
					action: crate::server_actions::Action::Delete(guild),
					request,
					result: Err(failure),
				})
			}
		}
	}
	pub fn guild(&self) -> Id {
		match self {
			Self::Admin { guild, .. }
			| Self::Settings { guild, .. }
			| Self::Delete { guild, .. }
			| Self::Channel { guild, .. }
			| Self::Community { guild, .. }
			| Self::Onboarding { guild, .. } => *guild,
		}
	}
	pub fn bytes(&self) -> usize {
		match self {
			Self::Admin { .. }
			| Self::Settings { .. }
			| Self::Channel { .. }
			| Self::Community { .. }
			| Self::Onboarding { .. } => MAX_TARGET_BYTES,
			Self::Delete { .. } => 0,
		}
	}
	fn allowed(&self, state: &State) -> bool {
		match self {
			Self::Admin {
				guild,
				request,
				action,
			} => state.server_admin_command_allowed(*guild, *request, action),
			Self::Settings {
				guild,
				request,
				edit,
			} => state.server_settings_mfa_allowed(*guild, *request, edit),
			Self::Delete { guild, request } => state.delete_server_mfa_allowed(*guild, *request),
			Self::Channel {
				guild,
				channel,
				request,
				action,
			} => state.channel_action_mfa_allowed(*guild, *channel, *request, action),
			Self::Community {
				guild,
				user,
				fence,
				request,
				action,
			} => {
				state.user.as_ref().is_some_and(|owner| owner.id == *user)
					&& state.community.fence == *fence
					&& state.community_command_allowed(*guild, *request, action)
			}
			Self::Onboarding {
				guild,
				request,
				form,
				submission,
			} => state.onboarding_mfa_allowed(*guild, *request, form, submission),
		}
	}
}
impl fmt::Debug for Target {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("MfaTarget([SCOPED])")
	}
}

pub struct Notice {
	pub id: u64,
	pub target: Target,
	pub methods: Vec<Method>,
}
pub enum Status {
	SmsSent,
	Verified,
	Resumed,
	Cancelled,
}
pub enum Event {
	Required(Notice),
	Updated {
		id: u64,
		result: Result<Status, Failure>,
	},
}
impl Event {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Required(n) => n.target.bytes() + n.methods.capacity() * size_of::<Method>(),
			_ => 0,
		}
	}
}
pub enum Action {
	Verify { method: Method, input: Input },
	SendSms,
	Resume,
	Cancel,
}
pub struct Request {
	pub id: u64,
	pub action: Action,
}
impl Request {
	pub fn bytes(&self) -> usize {
		match &self.action {
			Action::Verify { input, .. } => input.bytes(),
			_ => 0,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
	Editing,
	SendingSms,
	Verifying,
	Verified,
	Resuming,
}
pub struct Pending {
	pub id: u64,
	pub methods: Vec<Method>,
	pub phase: Phase,
	pub error: Option<&'static str>,
	pub sms_sent: bool,
	pub target: Target,
	owner: Id,
	generation: u64,
	at: Instant,
}
impl Pending {
	pub fn remaining(&self) -> Duration {
		LIFETIME.saturating_sub(self.at.elapsed())
	}
}
#[derive(Default)]
pub struct View {
	pub pending: Option<Pending>,
	cancel: Option<u64>,
}
impl View {
	pub fn bytes(&self) -> usize {
		self.pending.as_ref().map_or(0, |p| {
			p.target.bytes() + p.methods.capacity() * size_of::<Method>()
		})
	}
}
impl State {
	fn mfa_pending_allowed(&self, pending: &Pending) -> bool {
		self.auth == AuthState::Authenticated
			&& self.gateway_connected
			&& self.generation == pending.generation
			&& self.user.as_ref().is_some_and(|u| u.id == pending.owner)
			&& !pending.remaining().is_zero()
			&& pending.target.allowed(self)
	}
	/// Checked by the host immediately before dispatch, including the post-verification resume.
	pub fn mfa_command_allowed(&self, request: &Request) -> bool {
		if matches!(request.action, Action::Cancel) {
			return request.id != 0;
		}
		let Some(pending) = self.mfa.pending.as_ref().filter(|p| p.id == request.id) else {
			return false;
		};
		self.mfa_pending_allowed(pending)
			&& match &request.action {
				Action::Verify { method, input } => {
					pending.phase == Phase::Verifying
						&& pending.methods.contains(method)
						&& method.accepts(input.expose())
				}
				Action::SendSms => {
					pending.phase == Phase::SendingSms && pending.methods.contains(&Method::Sms)
				}
				Action::Resume => pending.phase == Phase::Resuming,
				Action::Cancel => true,
			}
	}
	pub fn submit_mfa(&mut self, method: Method, value: String) -> Option<Command> {
		let input = Input::new(method, value)?;
		let pending = self.mfa.pending.as_ref()?;
		if !self.mfa_pending_allowed(pending)
			|| pending.phase != Phase::Editing
			|| !pending.methods.contains(&method)
			|| method == Method::Sms && !pending.sms_sent
		{
			return None;
		}
		let pending = self.mfa.pending.as_mut()?;
		pending.phase = Phase::Verifying;
		pending.error = None;
		Some(Command::Mfa(Box::new(Request {
			id: pending.id,
			action: Action::Verify { method, input },
		})))
	}
	pub fn send_mfa_sms(&mut self) -> Option<Command> {
		let pending = self.mfa.pending.as_ref()?;
		if !self.mfa_pending_allowed(pending)
			|| pending.phase != Phase::Editing
			|| !pending.methods.contains(&Method::Sms)
		{
			return None;
		}
		let pending = self.mfa.pending.as_mut()?;
		pending.phase = Phase::SendingSms;
		pending.error = None;
		Some(Command::Mfa(Box::new(Request {
			id: pending.id,
			action: Action::SendSms,
		})))
	}
	pub fn cancel_mfa(&mut self) -> Option<Command> {
		let pending = self.mfa.pending.take()?;
		Some(Command::Mfa(Box::new(Request {
			id: pending.id,
			action: Action::Cancel,
		})))
	}
	/// Called once per UI frame: expired/revoked challenges cancel; verified ones resume once.
	pub fn maintain_mfa(&mut self) -> Option<Command> {
		if let Some(id) = self.mfa.cancel.take() {
			return Some(Command::Mfa(Box::new(Request {
				id,
				action: Action::Cancel,
			})));
		}
		let pending = self.mfa.pending.as_ref()?;
		if !self.mfa_pending_allowed(pending) {
			return self.cancel_mfa();
		}
		if pending.phase != Phase::Verified {
			return None;
		}
		let pending = self.mfa.pending.as_mut()?;
		pending.phase = Phase::Resuming;
		Some(Command::Mfa(Box::new(Request {
			id: pending.id,
			action: Action::Resume,
		})))
	}
	pub(crate) fn apply_mfa(&mut self, event: Event) {
		match event {
			Event::Required(notice) => {
				if self.mfa.pending.is_some()
					|| notice.id == 0
					|| notice.methods.is_empty()
					|| notice.methods.len() > MAX_METHODS
					|| notice.methods.capacity() > MAX_METHODS * 2
					|| !notice.target.allowed(self)
					|| self.auth != AuthState::Authenticated
					|| !self.gateway_connected
				{
					if notice.id != 0 {
						self.mfa.cancel = Some(notice.id);
					}
					return;
				}
				let Some(owner) = self.user.as_ref().map(|u| u.id) else {
					return;
				};
				self.mfa.pending = Some(Pending {
					id: notice.id,
					methods: notice.methods,
					phase: Phase::Editing,
					error: None,
					sms_sent: false,
					target: notice.target,
					owner,
					generation: self.generation,
					at: Instant::now(),
				});
			}
			Event::Updated { id, result } => {
				let Some(pending) = self.mfa.pending.as_mut().filter(|p| p.id == id) else {
					return;
				};
				match result {
					Ok(Status::SmsSent) if pending.phase == Phase::SendingSms => {
						pending.sms_sent = true;
						pending.phase = Phase::Editing;
					}
					Ok(Status::Verified) if pending.phase == Phase::Verifying => {
						pending.phase = Phase::Verified
					}
					Ok(Status::Resumed | Status::Cancelled) => self.mfa.pending = None,
					Err(failure) => {
						pending.phase = Phase::Editing;
						pending.error = Some(failure.label());
						if failure.ends_session() {
							self.fail(failure);
						}
					}
					_ => {}
				}
			}
		}
	}
	pub fn reject_mfa(&mut self, id: u64) {
		self.apply_mfa(Event::Updated {
			id,
			result: Err(Failure::ProtocolAt(
				"Verification was not queued; try again",
			)),
		});
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn fixture() -> (State, Target) {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			user: Some(model::User {
				kind: model::AccountKind::Human,
				webhook: false,
				id: Id(1),
				name: "Synthetic owner".into(),
				username: None,
				avatar: None,
				discriminator: 0,
				primary_guild: None,
				cosmetics: None,
			}),
			..State::default()
		};
		state.guilds.push(model::Guild {
			id: Id(2),
			name: "Synthetic server".into(),
			icon: None,
			default_message_notifications: None,
			emojis: None,
			stickers: None,
		});
		state.permissions.guilds.insert(
			Id(2),
			model::permissions::Guild {
				id: Id(2),
				owner: Some(Id(1)),
				roles: None,
				member: None,
			},
		);
		let command = state
			.delete_server(Id(2))
			.expect("explicit eligible owner action");
		let target = Target::from_command(&command).unwrap();
		(state, target)
	}
	fn require(state: &mut State, target: Target) {
		state.apply_mfa(Event::Required(Notice {
			id: 7,
			target,
			methods: vec![Method::Totp, Method::Sms],
		}));
	}
	#[test]
	fn verified_operation_rechecks_permissions_and_resumes_only_once() {
		let (mut state, target) = fixture();
		require(&mut state, target);
		assert!(state.maintain_mfa().is_none());
		state.apply_mfa(Event::Updated {
			id: 7,
			result: Ok(Status::Verified),
		});
		assert_eq!(
			state.mfa.pending.as_ref().unwrap().phase,
			Phase::Editing,
			"unsolicited verification cannot elevate an action"
		);
		let Command::Mfa(request) = state.submit_mfa(Method::Totp, "123456".into()).unwrap() else {
			panic!()
		};
		assert!(state.mfa_command_allowed(&request));
		state.apply_mfa(Event::Updated {
			id: 7,
			result: Ok(Status::Verified),
		});
		let Command::Mfa(resume) = state.maintain_mfa().unwrap() else {
			panic!()
		};
		assert!(state.mfa_command_allowed(&resume));
		assert!(state.maintain_mfa().is_none());
		state.permissions.guilds.get_mut(&Id(2)).unwrap().owner = Some(Id(3));
		assert!(!state.mfa_command_allowed(&resume));
		let Command::Mfa(cancel) = state.maintain_mfa().unwrap() else {
			panic!()
		};
		assert!(matches!(cancel.action, Action::Cancel));
		assert!(state.mfa.pending.is_none());
	}
	#[test]
	fn verification_input_sms_and_expiry_are_explicit_and_account_scoped() {
		let (mut state, target) = fixture();
		require(&mut state, target);
		assert!(state.submit_mfa(Method::Sms, "123456".into()).is_none());
		let Command::Mfa(sms) = state.send_mfa_sms().unwrap() else {
			panic!()
		};
		assert!(state.mfa_command_allowed(&sms));
		assert!(state.send_mfa_sms().is_none());
		state.apply_mfa(Event::Updated {
			id: 8,
			result: Ok(Status::SmsSent),
		});
		assert!(!state.mfa.pending.as_ref().unwrap().sms_sent);
		state.apply_mfa(Event::Updated {
			id: 7,
			result: Ok(Status::SmsSent),
		});
		assert!(state.submit_mfa(Method::Sms, "123456".into()).is_some());
		state.mfa.pending.as_mut().unwrap().at = Instant::now() - LIFETIME;
		assert!(matches!(state.maintain_mfa(), Some(Command::Mfa(_))));
		assert!(state.mfa.pending.is_none());
		let (mut state, target) = fixture();
		require(&mut state, target);
		state.generation += 1;
		assert!(state.submit_mfa(Method::Totp, "123456".into()).is_none());
		assert!(state.maintain_mfa().is_some());
		assert!(
			format!(
				"{:?}",
				Input::new(Method::Password, "synthetic secret".into()).unwrap()
			)
			.contains("REDACTED")
		);
	}
	#[test]
	fn stale_or_revoked_challenge_is_cancelled_without_presenting_an_input() {
		let (mut state, target) = fixture();
		state.permissions.guilds.clear();
		require(&mut state, target);
		assert!(state.mfa.pending.is_none());
		let Some(Command::Mfa(request)) = state.maintain_mfa() else {
			panic!()
		};
		assert_eq!(request.id, 7);
		assert!(matches!(request.action, Action::Cancel));
	}
}
