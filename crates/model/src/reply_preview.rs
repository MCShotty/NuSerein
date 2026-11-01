//! One-level, session-only reply summaries. They never retain media URLs or nested replies.
use crate::Message;

pub const MAX_REPLY_PREVIEW_BYTES: usize = 8 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct ReplyPreview {
	message: Box<Message>,
}

impl ReplyPreview {
	pub fn from_message(mut message: Message) -> Option<Self> {
		message.reply_preview = None;
		message.reply_to = None;
		message.reply_deleted = false;
		message.poll = None;
		message.sticker_items = Vec::new();
		message.components = Vec::new();
		message.interaction = None;
		message.reactions = None;
		message.embeds = Vec::new();
		message.author.cosmetics = None;
		message.author.primary_guild = None;
		message.mention_roles = Vec::new();
		message.author_roles.truncate(16);
		message.author_roles.shrink_to_fit();
		message.nonce = None;
		truncate(&mut message.content, 4096);
		message.mentions.truncate(16);
		for user in &mut message.mentions {
			user.cosmetics = None;
			user.primary_guild = None;
		}
		let spoiler = message
			.attachments
			.iter()
			.any(|attachment| attachment.spoiler);
		message.attachments.truncate(1);
		if let Some(attachment) = message.attachments.first_mut() {
			// Preserve only the classification consumed by the display renderer.
			let image = attachment.is_image();
			attachment.filename = if attachment.is_video() {
				"video.mp4"
			} else if image {
				"image.png"
			} else if attachment.is_audio() {
				"audio.ogg"
			} else {
				"attachment"
			}
			.into();
			if image && !attachment.is_image() {
				attachment.content_type = Some("image/png".into());
			}
			attachment.media = Default::default();
			attachment.description = None;
			attachment.waveform = Vec::new();
			attachment.spoiler = spoiler;
			truncate(&mut attachment.filename, 128);
			if let Some(content_type) = &mut attachment.content_type {
				truncate(content_type, 128);
			}
		}
		message.attachments.shrink_to_fit();
		message.mentions.shrink_to_fit();
		while std::mem::size_of::<Self>() + message.bytes() > MAX_REPLY_PREVIEW_BYTES
			&& !message.mentions.is_empty()
		{
			message.mentions.pop();
			message.mentions.shrink_to_fit();
		}
		(message.id.0 != 0
			&& message.channel.0 != 0
			&& message.author.id.0 != 0
			&& std::mem::size_of::<Self>() + message.bytes() <= MAX_REPLY_PREVIEW_BYTES)
			.then(|| Self {
				message: Box::new(message),
			})
	}
	pub fn message(&self) -> &Message {
		&self.message
	}
	pub fn bytes(&self) -> usize {
		std::mem::size_of::<Self>() + self.message.bytes()
	}
}

fn truncate(text: &mut String, limit: usize) {
	if text.len() > limit {
		let mut boundary = limit;
		while !text.is_char_boundary(boundary) {
			boundary -= 1;
		}
		text.truncate(boundary);
	}
	text.shrink_to_fit();
}
