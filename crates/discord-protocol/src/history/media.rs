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
		let list = raw.get().starts_with('[');
		if matches!(self, Self::Embeds) {
			// History truncates long arrays and replaces unreadable individual cards.
			// A valid outer shape alone cannot explain the enclosing message failure.
			return if list || raw.get() == "null" {
				EmbedContext
			} else {
				EmbedList
			};
		}
		if !list {
			return AttachmentList;
		}
		struct Probe<'a>(&'a mut MediaIssue);
		impl<'de> Visitor<'de> for Probe<'_> {
			type Value = ();
			fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str("bounded attachment list")
			}
			fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
				for index in 0..=model::MAX_ATTACHMENTS {
					let Some(item) = seq.next_element::<&RawValue>()? else {
						break;
					};
					if index == model::MAX_ATTACHMENTS {
						*self.0 = AttachmentCount;
						break;
					}
					if let Err(issue) = crate::attachments::history_item(item) {
						*self.0 = issue;
						break;
					}
				}
				Ok(())
			}
		}
		// An early stop need not consume unrelated siblings. Ignore serde's trailing
		// input error; only the fixed enum set at the rejecting check leaves this pass.
		let mut issue = AttachmentContext;
		let _ = raw.deserialize_seq(Probe(&mut issue));
		issue
	}
}
