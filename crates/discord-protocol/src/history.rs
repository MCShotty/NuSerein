//! Display-only history projection. Mutation readbacks still require a strict MessageDto.
use crate::{WireMessage, reactions::ReactionList, reply_preview_field};
use model::Message;
use serde::{
	Deserializer,
	de::{Error as _, MapAccess, SeqAccess, Visitor, value::MapDeserializer},
};
use serde_json::value::RawValue;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
	Limit,
	Page,
	Message,
	People,
	Media,
	Features,
}

/// Keep element and byte ceilings independent. Do not return a partial page on error.
pub fn page(bytes: &[u8]) -> Result<Vec<Message>, PageError> {
	if bytes.len() > crate::MAX_WIRE {
		return Err(PageError::Limit);
	}
	struct Page<'a>(&'a mut PageError);
	impl<'de> Visitor<'de> for Page<'_> {
		type Value = Vec<Message>;
		fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			f.write_str("a bounded history page")
		}
		fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
			let mut rows = Vec::new();
			while let Some(raw) = seq.next_element::<&RawValue>()? {
				if rows.len() == 50 {
					*self.0 = PageError::Limit;
					return Err(A::Error::custom("history page limit"));
				}
				let message = match serde_json::from_str::<WireMessage<true>>(raw.get()) {
					Ok(dto) => dto.into_model(),
					Err(_) => {
						let mut why = PageError::Message;
						raw.deserialize_map(Recover(&mut why)).map_err(|_| {
							*self.0 = why;
							A::Error::custom("invalid history message")
						})?
					}
				};
				rows.push(message);
			}
			Ok(rows)
		}
	}
	let mut error = PageError::Page;
	let mut json = serde_json::Deserializer::from_slice(bytes);
	let rows = json.deserialize_seq(Page(&mut error)).map_err(|_| error)?;
	json.end().map_err(|_| PageError::Page)?;
	Ok(rows)
}

// Only on a failed row: omit malformed optional presentation, never an identity, flag,
// attachment, permission hint or reference target. Forward original raw values (not a JSON
// object map, which would silently resolve duplicate keys). No reconstructed wire body.
struct Recover<'a>(&'a mut PageError);
impl<'de> Visitor<'de> for Recover<'_> {
	type Value = Message;
	fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("a history message object")
	}
	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Message, A::Error> {
		let mut fields = Vec::new();
		let (mut reactions, mut preview) = (None, None);
		let mut count = 0;
		while let Some(key) = map.next_key::<String>()? {
			count += 1;
			if count > 128 || key.len() > 128 {
				return Err(A::Error::custom("message field limit"));
			}
			let value: &'de RawValue = map.next_value()?;
			let (slot, valid) = match key.as_str() {
				"reactions" => (
					&mut reactions,
					serde_json::from_str::<ReactionList>(value.get()).is_ok(),
				),
				"referenced_message" => (&mut preview, reply_preview_field(value).is_ok()),
				_ => {
					fields.push((key, value));
					continue;
				}
			};
			if slot.replace(valid).is_some() {
				return Err(A::Error::custom("duplicate message field"));
			}
			if valid {
				fields.push((key, value));
			}
		}
		let dto: WireMessage<true> = serde_path_to_error::deserialize(MapDeserializer::<
			_,
			serde_json::Error,
		>::new(fields.into_iter()))
		.map_err(|error| {
			// Never format the path, map keys, or raw error into an event or log.
			if let Some(serde_path_to_error::Segment::Map { key }) = error.path().iter().next() {
				*self.0 = match key.as_str() {
					"author" | "member" | "mentions" | "mention_roles" => PageError::People,
					"attachments" | "embeds" => PageError::Media,
					"poll" | "sticker_items" | "stickers" | "components" | "message_snapshots" => {
						PageError::Features
					}
					_ => PageError::Message,
				};
			}
			A::Error::custom("invalid history message")
		})?;
		if reactions != Some(false) && preview != Some(false) {
			return Err(A::Error::custom("invalid history message"));
		}
		let mut row = dto.into_model();
		if reactions == Some(false) {
			row.reactions = None;
		} // unknown, never an authoritative empty list
		Ok(row) // omitted preview is Absent, never a deletion marker
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::MessageDto;
	use serde_json::json;
	fn reply() -> serde_json::Value {
		json!({"id":"11","type":19,"channel_id":"7","author":{"id":"2","username":"fixture"},"content":"body","message_reference":{"channel_id":"7","message_id":"8"}})
	}
	#[test]
	fn optional_metadata_does_not_poison_neighbors_or_invent_deletion() {
		let mut row = reply();
		row["reactions"] = json!([{"count":1,"emoji":{"name":"x"}}]); // missing `me`
		row["referenced_message"] = json!({"id":"8","author":{"id":"3"}}); // incomplete preview author
		row["embeds"] = json!([{"type":"rich","title":"kept"}]);
		let mut next = reply();
		next["id"] = json!("12");
		next["referenced_message"] = serde_json::Value::Null;
		let bytes = serde_json::to_vec(&[row.clone(), next]).unwrap();
		assert!(crate::decode::<Vec<MessageDto>>(&bytes).is_err());
		let rows = page(&bytes).unwrap();
		assert_eq!(rows.len(), 2);
		let r = &rows[0];
		assert_eq!(r.content, "body");
		assert_eq!(r.embeds[0].title.as_deref(), Some("kept"));
		assert_eq!(r.reply_to, Some(model::Id(8)));
		assert!(r.reactions.is_none() && r.reply_preview.is_none() && !r.reply_deleted);
		assert!(rows[1].reply_deleted && rows[1].reactions.as_ref().unwrap().is_empty());
		row["referenced_message"] = serde_json::Value::Null;
		assert!(page(&serde_json::to_vec(&[&row]).unwrap()).unwrap()[0].reply_deleted);
		for (field, value, why) in [
			("id", json!("0"), PageError::Message),
			("channel_id", json!(42), PageError::Message),
			("flags", json!("64"), PageError::Message),
			("author", json!({"username":"unknown"}), PageError::People),
			("member", json!({"roles":["0"]}), PageError::People),
			("attachments", json!([{"id":0}]), PageError::Media),
			("message_snapshots", json!(false), PageError::Features),
		] {
			let mut bad = row.clone();
			bad[field] = value;
			assert_eq!(
				page(&serde_json::to_vec(&[reply(), bad]).unwrap()).err(),
				Some(why)
			);
		}
	}
	#[test]
	fn unreadable_file_is_inert_with_good_siblings_in_history_and_forwards() {
		let good = json!({"id":"20","filename":"kept.png","width":null});
		let mut damaged = json!({"id":"21","filename":"secret.mp4","content_type":"video/mp4","url":"https://example.invalid/file","proxy_url":"https://example.invalid/proxy","placeholder":"AAECAwQ=","placeholder_version":1,"duration_secs":1,"waveform":"AQID"});
		for (field, value, hidden) in [
			("width", json!("unknown"), false),
			("width", json!(-1), false),
			("size", json!(null), false),
			("filename", json!(null), true),
			("description", json!({}), true),
		] {
			let mut bad = damaged.clone();
			bad[field] = value;
			let files = json!([good, bad, {"id":"22","filename":"kept.ogg"}]);
			for forward in [false, true] {
				let mut middle = reply();
				middle["id"] = json!("12");
				if forward {
					middle["type"] = json!(0);
					middle["message_reference"]["type"] = json!(1);
					middle["message_snapshots"] =
						json!([{"message":{"type":0,"content":"body","attachments":files}}]);
				} else {
					middle["attachments"] = files.clone();
				}
				let mut last = reply();
				last["id"] = json!("13");
				let bytes = serde_json::to_vec(&[reply(), middle, last]).unwrap();
				assert!(crate::decode::<Vec<MessageDto>>(&bytes).is_err());
				let rows = page(&bytes).unwrap();
				assert_eq!(
					rows.iter().map(|r| r.id.0).collect::<Vec<_>>(),
					[11, 12, 13]
				);
				let row = &rows[1];
				assert_eq!(row.forwarded, forward);
				assert_eq!(row.content, "body");
				let [left, slot, right] = &row.attachments[..] else {
					panic!("lost a file slot")
				};
				assert!(left.is_image() && right.is_audio());
				assert_eq!((slot.id.0, slot.spoiler), (21, hidden));
				assert_eq!(slot.media, model::EmbedMedia::default());
				assert_eq!(slot.filename, "Attachment");
				assert!(
					slot.content_type.is_none()
						&& slot.description.is_none()
						&& slot.waveform.is_empty()
						&& slot.duration_ms.is_none()
				);
				assert!(!slot.is_video() && !slot.is_image() && !slot.is_audio());
			}
		}
		damaged["width"] = json!(-1);
		for (key, value) in [
			("filename", json!("SPOILER_x.png")),
			("flags", json!(64)),
			("description", json!("||hidden||")),
		] {
			let mut file = damaged.clone();
			file[key] = value;
			let mut row = reply();
			row["attachments"] = json!([file]);
			row["reactions"] = json!(false);
			let rows = page(&serde_json::to_vec(&[row]).unwrap()).unwrap();
			assert!(rows[0].attachments[0].spoiler && rows[0].reactions.is_none());
		}
	}
	#[test]
	fn file_recovery_keeps_identity_visibility_and_list_limits_strict() {
		for fields in [
			r#""id":"0""#,
			r#""id":4"#,
			r#""name":null"#,
			r#""id":"4","flags":"8""#,
			r#""id":"4","id":"5""#,
			r#""id":"4","height":2,"height":3"#,
		] {
			let wire = format!(
				r#"[{{"id":"1","channel_id":"7","author":{{"id":"2","username":"test"}},"attachments":[{{"width":-1,{fields}}}]}}]"#
			);
			assert_eq!(page(wire.as_bytes()).err(), Some(PageError::Media));
		}
		for (field, list) in [
			("attachments", json!(null)),
			("attachments", json!(vec![json!({"id":"4","width":-1}); 11])),
			("embeds", json!(vec![json!({}); 11])),
		] {
			let mut row = reply();
			row[field] = list;
			assert_eq!(
				page(&serde_json::to_vec(&[row]).unwrap()).err(),
				Some(PageError::Media)
			);
		}
	}
	#[test]
	fn recovery_preserves_wire_limits_duplicates_and_page_shape() {
		for bad in [b"{}".as_slice(), b"[] []", b"[{", b"null"] {
			assert_eq!(page(bad).err(), Some(PageError::Page));
		}
		assert_eq!(
			page(&serde_json::to_vec(&vec![reply(); 51]).unwrap()).err(),
			Some(PageError::Limit)
		);
		assert_eq!(
			page(&vec![b' '; crate::MAX_WIRE + 1]).err(),
			Some(PageError::Limit)
		);
		for pair in [
			r#""reactions":[],"reactions":null"#,
			r#""referenced_message":null,"referenced_message":false"#,
			r#""flags":0,"flags":64,"reactions":null"#,
		] {
			let body = format!(
				r#"[{{"id":"1","channel_id":"7","author":{{"id":"2","username":"test"}},{pair}}}]"#
			);
			assert_eq!(page(body.as_bytes()).err(), Some(PageError::Message));
		}
	}
}
