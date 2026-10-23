//! Documented command permissions objects; normal-account route use is unofficial.
use crate::{DecodeError, permissions::List};
use model::{Id, command_permissions as m};
use serde::Deserialize;
#[derive(Deserialize)]
struct Wire {
	id: Id,
	application_id: Id,
	guild_id: Id,
	permissions: List<m::Override, { m::MAX_OVERRIDES }>,
}
impl Wire {
	fn checked(self, guild: Id, app: Id) -> Result<m::Permissions, DecodeError> {
		let result = m::Permissions {
			id: self.id,
			overrides: self.permissions.0,
		};
		if self.guild_id != guild || self.application_id != app || !result.valid() {
			return Err(DecodeError);
		}
		Ok(result)
	}
}
pub fn list(bytes: &[u8], guild: Id, app: Id) -> Result<Vec<m::Permissions>, DecodeError> {
	let rows: List<Wire, { m::MAX_COMMANDS + 1 }> = crate::decode(bytes)?;
	let rows = rows
		.0
		.into_iter()
		.map(|row| row.checked(guild, app))
		.collect::<Result<Vec<_>, _>>()?;
	if rows
		.iter()
		.enumerate()
		.any(|(i, row)| rows[..i].iter().any(|old| old.id == row.id))
	{
		return Err(DecodeError);
	}
	Ok(rows)
}
pub fn one(bytes: &[u8], guild: Id, app: Id, id: Id) -> Result<m::Permissions, DecodeError> {
	let result = crate::decode::<Wire>(bytes)?.checked(guild, app)?;
	if result.id != id {
		return Err(DecodeError);
	}
	Ok(result)
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn permissions_reject_wrong_scope_duplicates_and_keep_unknown_kinds() {
		let input=br#"[{"id":"4","application_id":"2","guild_id":"3","permissions":[{"id":"8","type":42,"permission":true}]}]"#;
		assert_eq!(list(input, Id(3), Id(2)).unwrap()[0].overrides[0].kind, 42);
		assert!(list(input, Id(9), Id(2)).is_err());
		assert!(one(br#"{"id":"4","application_id":"2","guild_id":"3","permissions":[{"id":"8","type":1,"permission":true},{"id":"8","type":1,"permission":false}]}"#,Id(3),Id(2),Id(4)).is_err());
	}
}
