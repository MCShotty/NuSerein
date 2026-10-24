//! Message records, sparse patches and retained-byte accounting.
use super::{
	Attachment, Component, Deserialize, Embed, ExtraContent, ExtraContentPatch, Id,
	MAX_MENTION_ROLES, Patch, Reaction, ReplyPreview, Serialize, Sticker, SystemMessage, User,
	attachment_bytes, component_bytes, embed_bytes, mention_bytes, polls, reaction_bytes,
	sticker_bytes, system_messages,
};

/// The command invocation that produced an application response message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interaction {
	pub user: User,
	/// Bounded command name without the leading slash; empty when the service omitted it.
	#[serde(default)]
	pub command: String,
}

impl Interaction {
	pub fn heap_bytes(&self) -> usize {
		size_of::<Self>() + self.user.heap_bytes() + self.command.capacity()
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct Message {
	pub poll: Option<Box<polls::Poll>>,
	pub sticker_items: Vec<Sticker>,
	/// Original outer message flags, retained for interaction submissions.
	pub flags: u64,
	pub ephemeral: bool,
	pub components: Vec<Component>,
	pub application_id: Option<Id>,
	/// Last known counts. None means they have not been loaded yet.
	pub reactions: Option<Vec<Reaction>>,
	pub id: Id,
	pub channel: Id,
	pub author: User,
	/// Role membership supplied with this message.
	pub author_roles: Vec<Id>,
	/// Guild nickname supplied with this message.
	pub author_nick: Option<String>,
	pub content: String,
	pub mentions: Vec<User>,
	/// Session-only service notification metadata; never inferred from message text.
	pub mention_roles: Vec<Id>,
	pub mention_everyone: bool,
	pub suppress_notifications: bool,
	pub edited: bool,
	pub edited_at: Option<i128>,
	pub revision: u64,
	pub nonce: Option<String>,
	/// The replied-to message, or the closed poll of a poll result (type 46).
	pub reply_to: Option<Id>,
	/// Bounded original-message summary supplied by the service; never persisted.
	pub reply_preview: Option<Box<ReplyPreview>>,
	/// Discord message type; 255 denotes an unknown legacy cached type.
	pub kind: u8,
	/// The service explicitly returned a null referenced message, not an unresolved preview.
	pub reply_deleted: bool,
	/// Body is the immutable snapshot attached to a forwarded message.
	pub forwarded: bool,
	/// The application command invocation this message answers.
	pub interaction: Option<Box<Interaction>>,
	pub unsupported: bool,
	pub extra_content: ExtraContent,
	pub embeds: Vec<Embed>,
	pub embeds_suppressed: bool,
	pub attachments: Vec<Attachment>,
}

impl Message {
	/// A plain-text description, separate from the original service content.
	pub fn system_summary(&self) -> Option<String> {
		self.system_message().map(|system| system.summary())
	}

	/// Styled runs describing a service-generated message, or `None` for user content.
	pub fn system_message(&self) -> Option<SystemMessage> {
		system_messages::describe(self)
	}

	/// Whether the service, not a user, generated this message.
	pub fn is_system(&self) -> bool {
		system_messages::is_system(self.kind)
	}

	pub fn display_text(&self) -> std::borrow::Cow<'_, str> {
		match self.system_message() {
			Some(system) if system.content_shown || self.content.is_empty() => {
				system.summary().into()
			}
			Some(system) => format!("{}\n{}", system.summary(), self.content).into(),
			None => self.content.as_str().into(),
		}
	}

	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.poll.as_ref().map_or(0, |poll| poll.bytes())
			+ self
				.reply_preview
				.as_ref()
				.map_or(0, |preview| preview.bytes())
			+ self.reactions.as_ref().map_or(0, |r| {
				reaction_bytes(r) + r.capacity().saturating_sub(r.len()) * size_of::<Reaction>()
			}) + self.content.capacity()
			+ self.author.heap_bytes()
			+ self.interaction.as_ref().map_or(0, |i| i.heap_bytes())
			+ self.author_nick.as_ref().map_or(0, String::capacity)
			+ self.author_roles.capacity() * size_of::<Id>()
			+ mention_bytes(&self.mentions)
			+ self.mention_roles.capacity() * size_of::<Id>()
			+ self.nonce.as_ref().map_or(0, String::capacity)
			+ attachment_bytes(&self.attachments)
			+ self
				.attachments
				.capacity()
				.saturating_sub(self.attachments.len())
				* size_of::<Attachment>()
			+ sticker_bytes(&self.sticker_items)
			+ component_bytes(&self.components)
			+ embed_bytes(&self.embeds)
			+ self.embeds.capacity().saturating_sub(self.embeds.len()) * size_of::<Embed>()
	}
}

pub fn valid_mention_roles(roles: &Vec<Id>) -> bool {
	roles.len() <= MAX_MENTION_ROLES
		&& roles.capacity() <= MAX_MENTION_ROLES
		&& roles
			.iter()
			.enumerate()
			.all(|(index, id)| id.0 != 0 && !roles[..index].contains(id))
}

#[derive(Clone)]
pub struct MessagePatch {
	pub poll: Patch<Option<Box<polls::Poll>>>,
	pub sticker_items: Patch<Vec<Sticker>>,
	pub flags: Patch<u64>,
	pub components: Patch<Vec<Component>>,
	pub application_id: Patch<Id>,
	pub extra_content: ExtraContentPatch,
	pub reactions: Patch<Vec<Reaction>>,
	pub id: Id,
	pub channel: Id,
	pub content: Patch<String>,
	pub mentions: Patch<Vec<User>>,
	pub edited: Patch<i128>,
	pub embeds: Patch<Vec<Embed>>,
	pub embeds_suppressed: Patch<bool>,
	pub attachments: Patch<Vec<Attachment>>,
}
