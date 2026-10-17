//! Session-only, bounded soundboard catalog for the joined server voice channel.
//! Sound audio never enters this state; the desktop fetches and plays it.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
	voice::Phase,
};
use model::{Id, permissions as p, soundboard::Sound};
use std::time::{Duration, Instant};
pub const MAX_PLAYBACK_LATENESS: Duration = Duration::from_secs(3);
/// Largest accepted catalog response.
pub const MAX_WIRE_BYTES: usize = 256 * 1024;
pub enum Request {
	Default {
		request: u64,
	},
	Guild {
		guild: Id,
		request: u64,
	},
	Send {
		request: u64,
		call: u64,
		channel: Id,
		sound: Id,
		/// Required by the service only for a sound owned by another server.
		source_guild: Option<Id>,
	},
}
pub enum Event {
	Default {
		request: u64,
		result: Result<Vec<Sound>, Failure>,
	},
	Guild {
		request: u64,
		guild: Id,
		result: Result<Vec<Sound>, Failure>,
	},
	Sent {
		request: u64,
		channel: Id,
		sound: Id,
		result: Result<(), Failure>,
	},
	/// Another participant played a sound in a voice channel. Not retained.
	Effect {
		channel: Id,
		user: Id,
		sound: Id,
		volume: f32,
	},
	/// A server's sounds changed; its catalog is reloaded when next shown.
	Changed(Id),
}
impl Event {
	pub fn bytes(&self) -> usize {
		match self {
			Self::Default {
				result: Ok(sounds), ..
			}
			| Self::Guild {
				result: Ok(sounds), ..
			} => model::soundboard::sound_bytes(sounds),
			_ => 0,
		}
	}
}
#[derive(Default)]
pub struct Catalog {
	pub request: u64,
	pub sounds: Vec<Sound>,
	pub loading: bool,
	pub loaded: bool,
	/// The sounds changed while a read was outstanding; its response is shown but reloaded.
	pub stale: bool,
	pub error: Option<&'static str>,
}
impl Catalog {
	/// A failed load waits for an explicit retry instead of repeating every frame.
	fn begin(&mut self) -> bool {
		if self.loading || self.loaded || self.error.is_some() {
			return false;
		}
		self.loading = true;
		self.stale = false;
		self.error = None;
		true
	}
	fn finish(&mut self, request: u64, result: Result<Vec<Sound>, Failure>, guild: Option<Id>) {
		if !self.loading || self.request != request {
			return;
		}
		self.loading = false;
		match result {
			Ok(sounds)
				if model::soundboard::valid_sounds(&sounds)
					&& sounds.iter().all(|sound| sound.guild == guild) =>
			{
				self.sounds = sounds;
				self.loaded = !std::mem::take(&mut self.stale);
				self.error = None;
			}
			Ok(_) => self.error = Some(Failure::Capacity.label()),
			Err(error) => self.error = Some(error.label()),
		}
	}
	fn interrupt(&mut self) {
		if self.loading {
			self.loading = false;
			self.error = Some("Soundboard loading interrupted; try again");
		}
	}
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Pending {
	request: u64,
	channel: Id,
	sound: Id,
	call: u64,
	requested: Instant,
}
#[derive(Default)]
pub struct Soundboard {
	/// Monotonic within the account generation, including interrupted requests.
	pub sequence: u64,
	pub default: Catalog,
	/// Sounds of the one server whose voice channel was last shown.
	pub guild: Option<(Id, Catalog)>,
	/// One unanswered play request; another waits for its outcome.
	pub sending: Option<Pending>,
	pub error: Option<&'static str>,
}
impl Soundboard {
	fn next_request(&mut self) -> u64 {
		self.sequence = self.sequence.wrapping_add(1).max(1);
		self.sequence
	}
	pub fn sound(&self, id: Id) -> Option<&Sound> {
		self.default
			.sounds
			.iter()
			.chain(self.guild.iter().flat_map(|(_, catalog)| &catalog.sounds))
			.find(|sound| sound.id == id)
	}
}
impl State {
	pub(crate) fn interrupt_soundboard(&mut self) {
		self.soundboard.default.interrupt();
		if let Some((_, catalog)) = &mut self.soundboard.guild {
			catalog.interrupt();
		}
		if self.soundboard.sending.take().is_some() {
			self.soundboard.error = Some("Soundboard request interrupted; try again");
		}
	}
	/// The connected server voice channel and its guild; private calls have no soundboard.
	pub fn soundboard_channel(&self) -> Option<(Id, Id)> {
		let call = self.voice.active.as_ref()?;
		matches!(call.phase, Phase::Connected | Phase::Waiting)
			.then_some(call.guild)
			.flatten()
			.map(|guild| (call.channel, guild))
	}
	/// Why playing is unavailable, as a fixed label; `None` when a sound may be sent.
	pub fn soundboard_unavailable(&self) -> Option<&'static str> {
		let (Some((channel, _)), Some(call)) = (self.soundboard_channel(), &self.voice.active)
		else {
			return Some("Join a server voice channel to use the soundboard");
		};
		// The offline preview has no session; its play requests are answered locally.
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			Some("Soundboard is unavailable while offline")
		} else if self.permission(channel, p::SPEAK | p::USE_SOUNDBOARD) != Some(true) {
			Some("You do not have permission to use the soundboard in this channel")
		} else if call.server_muted || call.server_deafened {
			Some("Soundboard is unavailable while server muted or deafened")
		} else if call.deafened {
			Some("Undeafen to use the soundboard")
		} else if call.reconnecting.is_some() {
			Some("Soundboard is unavailable while the call reconnects")
		} else {
			None
		}
	}
	/// Load the catalogs for the connected channel once; at most two bounded reads.
	pub fn request_soundboard(&mut self) -> [Option<Command>; 2] {
		let Some((_, guild)) = self.soundboard_channel() else {
			return [None, None];
		};
		if self.demo || self.auth != AuthState::Authenticated || !self.gateway_connected {
			return [None, None];
		}
		if self.soundboard.guild.as_ref().map(|(id, _)| *id) != Some(guild) {
			self.soundboard.guild = Some((guild, Catalog::default()));
		}
		let default = if self.soundboard.default.begin() {
			let request = self.soundboard.next_request();
			self.soundboard.default.request = request;
			Some(Command::Soundboard(Request::Default { request }))
		} else {
			None
		};
		let guild = if self
			.soundboard
			.guild
			.as_mut()
			.is_some_and(|(_, catalog)| catalog.begin())
		{
			let request = self.soundboard.next_request();
			self.soundboard.guild.as_mut().unwrap().1.request = request;
			Some(Command::Soundboard(Request::Guild { guild, request }))
		} else {
			None
		};
		[default, guild]
	}
	/// Forget a failed load so the next `request_soundboard` tries again.
	pub fn retry_soundboard(&mut self) {
		self.soundboard.default.error = None;
		if let Some((_, catalog)) = &mut self.soundboard.guild {
			catalog.error = None;
		}
		self.soundboard.error = None;
	}
	pub fn soundboard_sending(&self, sound: Id) -> bool {
		self.soundboard.sending.is_some_and(|pending| {
			pending.sound == sound
				&& self.voice.active.as_ref().is_some_and(|call| {
					call.channel == pending.channel && call.request == pending.call
				})
		})
	}
	pub fn can_play_sound(&self, sound: &Sound) -> bool {
		let Some((channel, guild)) = self.soundboard_channel() else {
			return false;
		};
		sound.valid()
			&& sound.available
			&& self.soundboard.sending.is_none_or(|pending| {
				self.voice.active.as_ref().is_none_or(|call| {
					pending.call != call.request || pending.channel != call.channel
				})
			}) && self.soundboard_unavailable().is_none()
			&& (sound.guild.is_none()
				|| sound.guild == Some(guild)
				|| self.permission(channel, p::USE_EXTERNAL_SOUNDS) == Some(true))
	}
	pub fn play_sound(&mut self, sound: &Sound) -> Option<Command> {
		if !self.can_play_sound(sound) {
			return None;
		}
		let (channel, guild) = self.soundboard_channel()?;
		let call = self.voice.active.as_ref()?.request;
		let request = self.soundboard.next_request();
		self.soundboard.sending = Some(Pending {
			channel,
			sound: sound.id,
			request,
			call,
			requested: Instant::now(),
		});
		self.soundboard.error = None;
		Some(Command::Soundboard(Request::Send {
			request,
			call,
			channel,
			sound: sound.id,
			source_guild: sound.guild.filter(|owner| *owner != guild),
		}))
	}
	/// Recheck queued requests against the active call and current permissions.
	pub fn soundboard_command_allowed(&self, action: &Request) -> bool {
		if !self.demo && (self.auth != AuthState::Authenticated || !self.gateway_connected) {
			return false;
		}
		match action {
			Request::Default { request } => {
				*request != 0
					&& self.soundboard.default.loading
					&& self.soundboard.default.request == *request
			}
			Request::Guild { guild, request } => {
				*request != 0
					&& self
						.soundboard_channel()
						.is_some_and(|(_, current)| current == *guild)
					&& self.soundboard.guild.as_ref().is_some_and(|(id, catalog)| {
						id == guild && catalog.loading && catalog.request == *request
					})
			}
			Request::Send {
				request,
				call,
				channel,
				sound,
				source_guild,
			} => {
				*request != 0
					&& self.soundboard_unavailable().is_none()
					&& self.soundboard.sending.is_some_and(|pending| {
						pending.request == *request
							&& pending.call == *call
							&& pending.channel == *channel
							&& pending.sound == *sound
							&& self.voice.active.as_ref().is_some_and(|call| {
								call.channel == *channel && call.request == pending.call
							})
					}) && source_guild.is_none_or(|id| {
					id.0 != 0 && self.permission(*channel, p::USE_EXTERNAL_SOUNDS) == Some(true)
				})
			}
		}
	}
	/// Called before reducing an acknowledgement, so an old call cannot play into a new one.
	pub fn soundboard_playback_current(&self, event: &Event) -> bool {
		let Some(call) = self.voice.active.as_ref().filter(|call| {
			matches!(call.phase, Phase::Connected | Phase::Waiting)
				&& !call.deafened
				&& !call.server_deafened
				&& call.reconnecting.is_none()
		}) else {
			return false;
		};
		match event {
			Event::Effect { channel, .. } => *channel == call.channel,
			Event::Sent {
				request,
				channel,
				sound,
				result: Ok(()),
			} => self.soundboard.sending.is_some_and(|pending| {
				pending.request == *request
					&& pending.channel == *channel
					&& pending.sound == *sound
					&& pending.call == call.request
					&& call.channel == *channel
					&& pending.requested.elapsed() <= MAX_PLAYBACK_LATENESS
			}),
			_ => false,
		}
	}
	pub(crate) fn soundboard_rejected(&mut self, request: &Request) {
		const MESSAGE: &str = "Soundboard request was not queued; try again";
		match request {
			Request::Default { request } if self.soundboard.default.request == *request => {
				self.soundboard.default.loading = false;
				self.soundboard.default.error = Some(MESSAGE);
			}
			Request::Guild { guild, request } => {
				if let Some((id, catalog)) = &mut self.soundboard.guild
					&& id == guild && catalog.request == *request
				{
					catalog.loading = false;
					catalog.error = Some(MESSAGE);
				}
			}
			Request::Send {
				channel,
				sound,
				request,
				..
			} if self.soundboard.sending.is_some_and(|pending| {
				pending.request == *request
					&& pending.channel == *channel
					&& pending.sound == *sound
			}) =>
			{
				self.soundboard.sending = None;
				self.soundboard.error = Some(MESSAGE);
			}
			_ => {}
		}
	}
	/// Mark the call panel's copy of a server's sounds for another read.
	pub(crate) fn call_sounds_changed(&mut self, guild: Id) {
		if let Some((id, catalog)) = &mut self.soundboard.guild
			&& *id == guild
		{
			// A read already in flight may predate the change; it is repeated.
			catalog.stale = catalog.loading;
			catalog.loaded = false;
		}
	}
	pub(crate) fn apply_soundboard(&mut self, event: Event) {
		match event {
			Event::Default { request, result } => {
				self.soundboard.default.finish(request, result, None)
			}
			Event::Guild {
				guild,
				request,
				result,
			} => {
				if let Some((id, catalog)) = &mut self.soundboard.guild
					&& *id == guild
				{
					catalog.finish(request, result, Some(guild));
				}
			}
			Event::Sent {
				request,
				channel,
				sound,
				result,
			} => {
				if let Some(pending) = self.soundboard.sending.filter(|pending| {
					pending.request == request
						&& pending.channel == channel
						&& pending.sound == sound
				}) {
					self.soundboard.sending = None;
					if self
						.voice
						.active
						.as_ref()
						.is_some_and(|call| call.channel == channel && call.request == pending.call)
					{
						self.soundboard.error = result.err().map(|error| match error {
							Failure::Forbidden => {
								"Discord did not allow this sound in this channel"
							}
							Failure::Ambiguous => "The sound may already have played",
							other => other.label(),
						});
					}
				}
			}
			Event::Changed(guild) => {
				self.call_sounds_changed(guild);
				// The open management page reloads too, once its current request settles. A
				// read in flight may predate the change, so its response keeps this marker.
				if self.server_admin.guild == Some(guild) {
					self.server_admin.sounds_stale = true;
				}
			}
			Event::Effect { .. } => {}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::voice::Call;
	fn sound(id: u64, guild: Option<u64>) -> Sound {
		Sound {
			id: Id(id),
			name: "Quack".into(),
			volume: 1.0,
			emoji_id: None,
			emoji_name: None,
			guild: guild.map(Id),
			available: true,
		}
	}
	fn role(bits: u128) -> p::Snapshot {
		p::Snapshot {
			guilds: vec![p::Guild {
				id: Id(9),
				owner: Some(Id(999)),
				roles: Some(vec![p::Role {
					name: String::new(),
					color: 0,
					secondary_color: None,
					tertiary_color: None,
					position: 0,
					hoist: false,
					id: Id(9),
					bits,
				}]),
				member: Some(p::Member {
					roles: vec![],
					timeout_until: None,
				}),
			}],
			channels: vec![p::Channel {
				id: Id(2),
				guild: Id(9),
				overwrites: Some(vec![]),
			}],
		}
	}
	const ALLOWED: u128 = p::VIEW_CHANNEL | p::CONNECT | p::SPEAK | p::USE_SOUNDBOARD;
	fn connected(bits: u128) -> State {
		let mut state = State {
			user: Some(model::User {
				primary_guild: None,
				cosmetics: None,
				id: Id(1),
				name: "Owner".into(),
				avatar: None,
				webhook: false,
				kind: Default::default(),
				discriminator: 0,
			}),
			guilds: vec![model::Guild {
				default_message_notifications: None,
				stickers: None,
				id: Id(9),
				name: "Synthetic".into(),
				icon: None,
				emojis: None,
			}],
			channels: vec![model::Channel {
				id: Id(2),
				guild: Some(Id(9)),
				parent_id: None,
				kind: 2,
				name: "Voice".into(),
				position: 0,
				recipients: vec![],
				last_message: None,
				icon: None,
				member_list_id: None,
				tags: None,
				message_count: None,
			}],
			auth: AuthState::Authenticated,
			freshness: crate::Freshness::Fresh,
			gateway_connected: true,
			..State::default()
		};
		state.permissions.replace(role(bits)).unwrap();
		state.voice.active = Some(Call {
			reconnecting: None,
			channel: Id(2),
			guild: Some(Id(9)),
			connected_at: None,
			channel_started_at: None,
			server_muted: false,
			server_deafened: false,
			request: 1,
			phase: Phase::Connected,
			muted: false,
			deafened: false,
			participants: vec![],
			camera: false,
			watching: None,
			error: None,
		});
		state
	}
	#[test]
	fn soundboard_retries_reject_stale_catalogs_and_old_call_playbacks() {
		let mut state = connected(ALLOWED);
		state.request_soundboard();
		let old_default = state.soundboard.default.request;
		let old_guild = state.soundboard.guild.as_ref().unwrap().1.request;
		state.interrupt_soundboard();
		state.retry_soundboard();
		state.request_soundboard();
		state.apply_soundboard(Event::Default {
			request: old_default,
			result: Ok(vec![sound(1, None)]),
		});
		state.apply_soundboard(Event::Guild {
			guild: Id(9),
			request: old_guild,
			result: Ok(vec![sound(3, Some(9))]),
		});
		assert!(state.soundboard.default.loading && state.soundboard.default.sounds.is_empty());
		assert!(
			state.soundboard.guild.as_ref().unwrap().1.loading
				&& state.soundboard.guild.as_ref().unwrap().1.sounds.is_empty()
		);
		let request = state.soundboard.default.request;
		state.apply_soundboard(Event::Default {
			request,
			result: Ok(vec![sound(1, None)]),
		});
		assert!(state.soundboard.default.loaded);
		let Command::Soundboard(old) = state.play_sound(&sound(1, None)).unwrap() else {
			panic!()
		};
		let Request::Send {
			request: old_request,
			..
		} = old
		else {
			panic!()
		};
		let ack = |request| Event::Sent {
			request,
			channel: Id(2),
			sound: Id(1),
			result: Ok(()),
		};
		assert!(state.soundboard_command_allowed(&old));
		assert!(state.soundboard_playback_current(&ack(old_request)));
		state.voice.active.as_mut().unwrap().request += 1;
		assert!(!state.soundboard_command_allowed(&old));
		assert!(
			!state.soundboard_sending(Id(1)),
			"old-call requests do not leave a spinner in the new call"
		);
		assert!(!state.soundboard_playback_current(&ack(old_request)));
		let Command::Soundboard(new) = state.play_sound(&sound(1, None)).unwrap() else {
			panic!()
		};
		let Request::Send { request, .. } = new else {
			panic!()
		};
		assert_ne!(request, old_request);
		assert!(state.soundboard_sending(Id(1)));
		state.apply_soundboard(Event::Sent {
			request: old_request,
			channel: Id(2),
			sound: Id(1),
			result: Err(Failure::Forbidden),
		});
		assert!(state.soundboard.sending.is_some() && state.soundboard.error.is_none());
		assert!(state.soundboard_playback_current(&ack(request)));
		state.soundboard.sending.as_mut().unwrap().requested -= Duration::from_secs(4);
		assert!(
			!state.soundboard_playback_current(&ack(request)),
			"late acknowledgements never play an old sound"
		);
		state.apply_soundboard(ack(request));
		assert!(state.soundboard.sending.is_none());
		let Command::Soundboard(queued) = state.play_sound(&sound(1, None)).unwrap() else {
			panic!()
		};
		state
			.permissions
			.guilds
			.get_mut(&Id(9))
			.unwrap()
			.roles
			.as_mut()
			.unwrap()[0]
			.bits &= !p::USE_SOUNDBOARD;
		state.permissions.clear_cache();
		assert!(!state.soundboard_command_allowed(&queued));
		state.command_rejected(Command::Soundboard(queued));
		assert!(state.soundboard.sending.is_none());
	}
	#[test]
	fn catalogs_load_once_per_guild_and_reload_after_a_change_or_failure() {
		let mut state = connected(ALLOWED);
		let [default, guild] = state.request_soundboard();
		assert!(matches!(
			default,
			Some(Command::Soundboard(Request::Default { .. }))
		));
		assert!(matches!(
			guild,
			Some(Command::Soundboard(Request::Guild { guild: Id(9), .. }))
		));
		assert!(state.request_soundboard().iter().all(Option::is_none));

		state.apply_soundboard(Event::Default {
			request: state.soundboard.default.request,
			result: Err(Failure::Network),
		});
		assert!(state.soundboard.default.error.is_some());
		assert!(state.request_soundboard().iter().all(Option::is_none));
		state.retry_soundboard();
		assert!(state.request_soundboard()[0].is_some());
		state.apply_soundboard(Event::Default {
			request: state.soundboard.default.request,
			result: Ok(vec![sound(1, None)]),
		});
		// A catalog for another server, or naming a foreign owner, is never retained.
		state.apply_soundboard(Event::Guild {
			request: state.soundboard.guild.as_ref().unwrap().1.request,
			guild: Id(8),
			result: Ok(vec![sound(3, Some(8))]),
		});
		assert!(state.soundboard.guild.as_ref().unwrap().1.loading);
		state.apply_soundboard(Event::Guild {
			request: state.soundboard.guild.as_ref().unwrap().1.request,
			guild: Id(9),
			result: Ok(vec![sound(3, Some(8))]),
		});
		assert!(state.soundboard.guild.as_ref().unwrap().1.error.is_some());
		state.retry_soundboard();
		assert!(state.request_soundboard()[1].is_some());
		state.apply_soundboard(Event::Guild {
			request: state.soundboard.guild.as_ref().unwrap().1.request,
			guild: Id(9),
			result: Ok(vec![sound(3, Some(9))]),
		});
		assert!(state.soundboard.sound(Id(1)).is_some());
		assert!(state.soundboard.sound(Id(3)).is_some());
		assert!(state.request_soundboard().iter().all(Option::is_none));

		state.apply_soundboard(Event::Changed(Id(8)));
		assert!(state.request_soundboard().iter().all(Option::is_none));
		state.apply_soundboard(Event::Changed(Id(9)));
		assert!(matches!(
			state.request_soundboard(),
			[
				None,
				Some(Command::Soundboard(Request::Guild { guild: Id(9), .. }))
			]
		));
		// A change during that read keeps its response but schedules another read.
		state.apply_soundboard(Event::Changed(Id(9)));
		state.apply_soundboard(Event::Guild {
			request: state.soundboard.guild.as_ref().unwrap().1.request,
			guild: Id(9),
			result: Ok(vec![sound(4, Some(9))]),
		});
		assert!(state.soundboard.sound(Id(4)).is_some());
		assert!(matches!(
			state.request_soundboard(),
			[
				None,
				Some(Command::Soundboard(Request::Guild { guild: Id(9), .. }))
			]
		));
		state.apply_soundboard(Event::Guild {
			request: state.soundboard.guild.as_ref().unwrap().1.request,
			guild: Id(9),
			result: Ok(vec![sound(4, Some(9))]),
		});
		assert!(state.request_soundboard().iter().all(Option::is_none));

		state.voice.active.as_mut().unwrap().guild = None;
		assert!(state.request_soundboard().iter().all(Option::is_none));
		assert!(state.soundboard_unavailable().is_some());
	}
	#[test]
	fn playing_requires_permission_an_undeafened_call_and_one_request_at_a_time() {
		let quack = sound(1, None);
		// Missing SPEAK or USE_SOUNDBOARD never authorizes a write.
		for bits in [ALLOWED & !p::USE_SOUNDBOARD, ALLOWED & !p::SPEAK] {
			let mut denied = connected(bits);
			assert!(denied.soundboard_unavailable().is_some());
			assert!(denied.play_sound(&quack).is_none());
		}
		for edit in [
			(|call: &mut Call| call.deafened = true) as fn(&mut Call),
			|call| call.server_muted = true,
			|call| call.server_deafened = true,
			|call| call.phase = Phase::Securing,
		] {
			let mut blocked = connected(ALLOWED);
			edit(blocked.voice.active.as_mut().unwrap());
			assert!(blocked.play_sound(&quack).is_none());
		}
		let mut state = connected(ALLOWED);
		let mut unavailable = quack.clone();
		unavailable.available = false;
		assert!(state.play_sound(&unavailable).is_none());
		// Another server's sound needs the external-sounds permission and names its source.
		assert!(state.play_sound(&sound(4, Some(8))).is_none());
		let mut external = connected(ALLOWED | p::USE_EXTERNAL_SOUNDS);
		assert!(matches!(
			external.play_sound(&sound(4, Some(8))),
			Some(Command::Soundboard(Request::Send {
				source_guild: Some(Id(8)),
				..
			}))
		));

		assert!(matches!(
			state.play_sound(&sound(3, Some(9))),
			Some(Command::Soundboard(Request::Send {
				channel: Id(2),
				sound: Id(3),
				source_guild: None,
				..
			}))
		));
		assert!(state.play_sound(&quack).is_none());
		// An outcome for another request leaves the pending one in place.
		state.apply_soundboard(Event::Sent {
			request: state.soundboard.sending.unwrap().request,
			channel: Id(2),
			sound: Id(1),
			result: Ok(()),
		});
		assert!(state.soundboard.sending.is_some());
		state.apply_soundboard(Event::Sent {
			request: state.soundboard.sending.unwrap().request,
			channel: Id(2),
			sound: Id(3),
			result: Err(Failure::Forbidden),
		});
		assert!(state.soundboard.sending.is_none());
		assert!(state.soundboard.error.is_some());
		assert!(state.play_sound(&quack).is_some());
		assert!(state.soundboard.error.is_none());
		state.interrupt_soundboard();
		assert!(state.soundboard.sending.is_none());
	}
}
