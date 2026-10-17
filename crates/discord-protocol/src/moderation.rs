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
