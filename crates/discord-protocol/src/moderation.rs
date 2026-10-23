use crate::{DecodeError, UserDto, permissions::List};
use model::{Id, moderation as m};
use serde::Deserialize;
#[derive(Deserialize)]
struct Ban {
	user: UserDto,
	reason: Option<String>,
}
pub fn bans(bytes: &[u8], guild: Id, after: Option<Id>) -> Result<m::Bans, DecodeError> {
	let mut rows = crate::decode::<List<Ban, { m::MAX_BANS }>>(bytes)?
		.0
		.into_iter()
		.map(|row| m::Ban {
			user: row.user.into_model(),
			reason: row.reason,
		})
		.collect::<Vec<_>>();
	rows.sort_unstable_by_key(|row| row.user.id);
	rows.shrink_to_fit();
	if rows
		.iter()
		.any(|row| after.is_some_and(|after| row.user.id <= after))
	{
		return Err(DecodeError);
	}
	let next = (rows.len() == m::MAX_BANS).then(|| rows.last().unwrap().user.id);
	let page = m::Bans {
		guild,
		items: rows,
		next,
	};
	page.valid().then_some(page).ok_or(DecodeError)
}

/// Unofficial normal-account completion, never interpreted as a new command.
pub fn bulk_update(bytes: &[u8]) -> Result<(Id, m::BulkReport), DecodeError> {
	#[derive(Deserialize)]
	struct Report {
		banned_users: List<Id, { m::MAX_BULK }>,
		failed_users: List<Id, { m::MAX_BULK }>,
	}
	#[derive(Deserialize)]
	struct Update {
		guild_id: Id,
		bulk_ban: Report,
	}
	let update: Update = crate::decode(bytes)?;
	let users = update
		.bulk_ban
		.banned_users
		.0
		.into_iter()
		.map(|id| (id, m::BanStatus::Banned))
		.chain(
			update
				.bulk_ban
				.failed_users
				.0
				.into_iter()
				.map(|id| (id, m::BanStatus::Failed)),
		)
		.collect();
	let report = m::BulkReport {
		users,
		accepted: true,
	};
	(guild_id_valid(update.guild_id) && report.valid())
		.then_some((update.guild_id, report))
		.ok_or(DecodeError)
}
fn guild_id_valid(id: Id) -> bool {
	id.0 != 0
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn bulk_completion_is_bounded_and_rejects_overlapping_or_duplicate_targets() {
		let (guild, report) = bulk_update(
			br#"{"guild_id":"2","bulk_ban":{"banned_users":["7"],"failed_users":["8"]}}"#,
		)
		.unwrap();
		assert_eq!(guild, Id(2));
		assert_eq!(
			report.users,
			vec![(Id(7), m::BanStatus::Banned), (Id(8), m::BanStatus::Failed)]
		);
		assert!(
			bulk_update(
				br#"{"guild_id":"2","bulk_ban":{"banned_users":["7"],"failed_users":["7"]}}"#
			)
			.is_err()
		);
		assert!(
			bulk_update(
				br#"{"guild_id":"2","bulk_ban":{"banned_users":["7","7"],"failed_users":[]}}"#
			)
			.is_err()
		);
		let payload = serde_json::json!({"guild_id":"2","bulk_ban":{"banned_users":(1..=101).map(|id|id.to_string()).collect::<Vec<_>>(),"failed_users":[]}});
		assert!(bulk_update(&serde_json::to_vec(&payload).unwrap()).is_err());
	}
}
