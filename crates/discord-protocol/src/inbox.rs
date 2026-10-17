//! Unofficial saved-message account endpoints; only bounded typed data crosses the boundary.
use crate::{DecodeError, MessageDto, Timestamp, permissions::List};
use model::inbox::{MAX_BYTES, MAX_SAVED, SavedMessage, valid_saved};
use serde::Deserialize;

#[derive(Deserialize)]
struct Data {
	channel_id: model::Id,
	message_id: model::Id,
	guild_id: Option<model::Id>,
	due_at: Option<Timestamp>,
}
#[derive(Deserialize)]
struct Saved {
	message: Option<MessageDto>,
	save_data: Data,
}
impl Saved {
	fn checked(self) -> Result<SavedMessage, DecodeError> {
		let value = SavedMessage {
			channel: self.save_data.channel_id,
			message: self.save_data.message_id,
			guild: self.save_data.guild_id,
			due_at: self.save_data.due_at.map(|time| time.0),
			body: self.message.map(MessageDto::into_model),
		};
		value.valid().then_some(value).ok_or(DecodeError)
	}
}
pub fn saved(bytes: &[u8]) -> Result<Vec<SavedMessage>, DecodeError> {
	#[derive(Deserialize)]
	struct Page {
		results: List<Saved, MAX_SAVED>,
	}
	let mut rows = crate::decode::<Page>(bytes)?
		.results
		.0
		.into_iter()
		.map(Saved::checked)
		.collect::<Result<Vec<_>, _>>()?;
	rows.shrink_to_fit();
	if !valid_saved(&rows)
		|| rows.capacity() * size_of::<SavedMessage>()
			+ rows.iter().map(SavedMessage::bytes).sum::<usize>()
			> MAX_BYTES
	{
		return Err(DecodeError);
	}
	Ok(rows)
}
pub fn saved_one(bytes: &[u8]) -> Result<SavedMessage, DecodeError> {
	crate::decode::<Saved>(bytes)?.checked()
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn saved_metadata_is_bounded_and_bodies_must_match() {
		let row = serde_json::json!({"message":null,"save_data":{"channel_id":"20","message_id":"30","due_at":"2026-10-17T15:00:00Z"}});
		assert_eq!(
			saved_one(&serde_json::to_vec(&row).unwrap())
				.unwrap()
				.message,
			model::Id(30)
		);
		let page = serde_json::json!({"results":[row.clone(),row.clone()]});
		assert!(saved(&serde_json::to_vec(&page).unwrap()).is_err());
		let mut mismatch = row;
		mismatch["message"] = serde_json::json!({"id":"31","channel_id":"20","author":{"id":"2","username":"Synthetic"},"content":"Wrong message"});
		assert!(saved_one(&serde_json::to_vec(&mismatch).unwrap()).is_err());
		assert!(
			saved(
				&serde_json::to_vec(&serde_json::json!({"results":vec![mismatch;MAX_SAVED+1]}))
					.unwrap()
			)
			.is_err()
		);
	}
}
