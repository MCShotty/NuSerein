//! Validated account startup and permission preparation.
use super::{
	BTreeSet, Channel, Envelope, Event, Guild, Id, MAX_CONTENT, MAX_GUILD_STICKERS, MAX_NAV,
	MAX_PREMIUM_CONTENT, State, User, auth, notifications, permissions, read_state,
	valid_custom_emojis, valid_stickers,
};

pub struct Startup {
	/// Discord's `premium_type`: 0 none, 1 Nitro Classic, 2 Nitro, 3 Nitro Basic.
	pub premium_type: u8,
	pub user: User,
	pub guilds: Vec<Guild>,
	pub channels: Vec<Channel>,
	pub permissions: model::permissions::Snapshot,
	pub read_state: read_state::Event,
	pub notifications: Option<notifications::Event>,
	pub session_dnd: Option<bool>,
	pub warnings: model::account::Warnings,
}

impl Startup {
	/// Validate and build the permission mirror on the Gateway worker before enqueueing.
	pub fn prepare(mut self) -> Result<PreparedStartup, auth::Failure> {
		if self.bytes() > model::account::MAX_BYTES {
			return Err(auth::Failure::ProtocolAt(
				"Account startup exceeds safe capacity",
			));
		}
		let spare_bytes = (self.guilds.capacity() - self.guilds.len()) * size_of::<Guild>()
			+ (self.channels.capacity() - self.channels.len()) * size_of::<Channel>();
		let permissions = prepare_navigation(
			&self.user,
			&self.guilds,
			&self.channels,
			std::mem::take(&mut self.permissions),
			spare_bytes,
		)
		.map_err(auth::Failure::ProtocolAt)?;
		let bytes = self.bytes() + permissions.bytes() + size_of::<permissions::Permissions>();
		if bytes > model::account::MAX_BYTES {
			return Err(auth::Failure::ProtocolAt(
				"Account startup exceeds safe capacity",
			));
		}
		Ok(PreparedStartup {
			data: self,
			permissions,
			bytes,
		})
	}
	/// Bytes retained by this event for the bounded capacity budget.
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.user.heap_bytes()
			+ self.permissions.bytes()
			+ self.guilds.capacity().saturating_sub(self.guilds.len()) * size_of::<Guild>()
			+ self.channels.capacity().saturating_sub(self.channels.len()) * size_of::<Channel>()
			+ self.guilds.iter().map(Guild::bytes).sum::<usize>()
			+ self.channels.iter().map(Channel::bytes).sum::<usize>()
			+ self.read_state.bytes()
			+ self
				.notifications
				.as_ref()
				.map_or(0, notifications::Event::bytes)
	}
}

pub struct PreparedStartup {
	pub(super) data: Startup,
	pub(super) permissions: permissions::Permissions,
	pub(super) bytes: usize,
}

impl PreparedStartup {
	pub fn data(&self) -> &Startup {
		&self.data
	}
	pub fn permission_state(&self) -> &permissions::Permissions {
		&self.permissions
	}
	pub fn bytes(&self) -> usize {
		self.bytes
	}
}

impl std::ops::Deref for PreparedStartup {
	type Target = Startup;
	fn deref(&self) -> &Startup {
		&self.data
	}
}

pub(super) fn prepare_navigation(
	user: &User,
	guilds: &[Guild],
	channels: &[Channel],
	permissions: model::permissions::Snapshot,
	spare_bytes: usize,
) -> Result<permissions::Permissions, &'static str> {
	if channels.len() + guilds.len() > MAX_NAV
		|| channels.iter().any(|c| c.recipients.len() > 64)
		|| guilds
			.iter()
			.any(|g| g.emojis.as_ref().is_some_and(|e| !valid_custom_emojis(e)))
		|| guilds.iter().any(|g| {
			g.stickers.as_ref().is_some_and(|s| {
				!valid_stickers(s, MAX_GUILD_STICKERS)
					|| s.iter().any(|sticker| sticker.guild_id != Some(g.id))
			})
		}) || guilds.iter().map(Guild::bytes).sum::<usize>()
		+ channels.iter().map(Channel::bytes).sum::<usize>()
		+ permissions.bytes()
		+ spare_bytes
		> model::account::MAX_BYTES
	{
		return Err("Account navigation exceeds safe capacity");
	}
	let guild_ids: BTreeSet<_> = guilds.iter().map(|guild| guild.id).collect();
	let channel_ids: BTreeSet<_> = channels.iter().map(|channel| channel.id).collect();
	if user.id.0 == 0
		|| guild_ids.len() != guilds.len()
		|| channel_ids.len() != channels.len()
		|| guild_ids.contains(&Id(0))
		|| channel_ids.contains(&Id(0))
		|| channels.iter().any(|channel| {
			channel
				.guild
				.is_some_and(|guild| !guild_ids.contains(&guild))
		}) {
		return Err("Invalid account navigation");
	}
	let mut permission_state = permissions::Permissions::default();
	permission_state.replace(permissions)?;
	if guilds.iter().map(Guild::bytes).sum::<usize>()
		+ channels.iter().map(Channel::bytes).sum::<usize>()
		+ permission_state.bytes()
		+ spare_bytes
		> model::account::MAX_BYTES
	{
		return Err("Account navigation exceeds safe capacity");
	}
	Ok(permission_state)
}

impl State {
	pub fn logout(&mut self) {
		let generation = self.generation.wrapping_add(1);
		*self = Self {
			generation,
			..Self::default()
		};
	}
	pub(super) fn set_premium_type(&mut self, kind: u8) {
		self.premium_type = kind;
		self.stickers.external_allowed = matches!(kind, 2 | 3);
	}
	/// Longest message content Discord accepts from this account.
	pub fn content_limit(&self) -> usize {
		if matches!(self.premium_type, 1 | 2) {
			MAX_PREMIUM_CONTENT
		} else {
			MAX_CONTENT
		}
	}
	/// Largest per-message attachment total this account may send without server boosts.
	pub fn upload_limit(&self) -> u64 {
		const MIB: u64 = 1024 * 1024;
		match self.premium_type {
			2 => 500 * MIB,
			1 | 3 => 50 * MIB,
			_ => 20 * MIB,
		}
	}
}

impl State {
	pub(super) fn apply_startup(&mut self, startup: Box<PreparedStartup>, generation: u64) {
		if startup.bytes() > model::account::MAX_BYTES {
			self.cancel_history();
			self.auth = auth::AuthState::Failed;
			self.status = "Account startup exceeds safe capacity";
			return;
		}
		let PreparedStartup {
			data,
			permissions: prepared,
			..
		} = *startup;
		let Startup {
			premium_type,
			user,
			guilds,
			channels,
			permissions,
			read_state,
			notifications,
			session_dnd,
			mut warnings,
		} = data;
		self.apply_with_permissions(
			Envelope {
				generation,
				event: Event::Ready {
					user,
					guilds,
					channels,
					permissions,
				},
			},
			Some(prepared),
		);
		if self.auth != auth::AuthState::Authenticated {
			return;
		}
		self.set_premium_type(premium_type);
		warnings.read_state |= self.apply_read_state(read_state).is_err();
		if let Some(settings) = notifications {
			warnings.notifications |= self.apply_notification_preferences(settings).is_err();
		}
		let _ = self.apply_notification_preferences(notifications::Event::Presence(session_dnd));
		self.apply(Envelope {
			generation,
			event: Event::StartupWarnings(warnings),
		});
	}
}
