//! Classify only the original field of an already rejected history row. Nothing
//! here can admit a row, change pagination, or retain any part of the response.
use serde::{
	Deserializer,
	de::{SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaIssue {
	AttachmentList,
	AttachmentCount,
	AttachmentObject,
	AttachmentId,
	AttachmentFlags,
	AttachmentDuplicate,
	AttachmentFieldLimit,
	AttachmentContext,
	EmbedList,
	EmbedCount,
	EmbedContext,
}

#[derive(Clone, Copy)]
pub(super) enum Field {
	Attachments,
	Embeds,
}
impl Field {
	pub(super) fn rejected(self, raw: &RawValue) -> MediaIssue {
		use MediaIssue::*;
		// The direct field reader can succeed even if projection through the enclosing
		// message failed. Give that case its own code rather than blaming a valid file.
		let context = match self {
			Self::Attachments => AttachmentContext,
			Self::Embeds => EmbedContext,
		};
		if raw.get() == "null" && matches!(self, Self::Embeds) {
			return context;
		}
		if !raw.get().starts_with('[') {
			return match self {
				Self::Attachments => AttachmentList,
				Self::Embeds => EmbedList,
			};
		}
		struct Probe<'a>(Field, &'a mut MediaIssue);
		impl<'de> Visitor<'de> for Probe<'_> {
			type Value = ();
			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("bounded media list")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
				let (limit, overflow) = match self.0 {
					Field::Attachments => (model::MAX_ATTACHMENTS, AttachmentCount),
					Field::Embeds => (model::MAX_EMBEDS, EmbedCount),
				};
				for index in 0..=limit {
					let Some(item) = seq.next_element::<&RawValue>()? else {
						break;
					};
					if index == limit {
						*self.1 = overflow;
						break;
					}
					if matches!(self.0, Field::Attachments)
						&& let Err(issue) = crate::attachments::history_item(item)
					{
						*self.1 = issue;
						break;
					}
					// EmbedList already replaces any unreadable individual card with an inert slot.
				}
				Ok(())
			}
		}
		// An early stop need not consume unrelated siblings. Ignore serde's trailing
		// input error; only the fixed enum set at the rejecting check leaves this pass.
		let mut issue = context;
		let _ = raw.deserialize_seq(Probe(self, &mut issue));
		issue
	}
}
