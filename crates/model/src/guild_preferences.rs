//! Normalized, bounded account guild preferences. These are unofficial user-account fields.
use crate::{Id, Patch};

pub const MAX_EDITS: usize = 1024;
pub const MAX_PENDING_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
	pub flags: Patch<u64>,
	pub collapsed: Patch<bool>,
	pub muted: Patch<bool>,
	pub level: Patch<u8>,
	pub mute_until: Patch<i64>,
	pub suppress_everyone: Patch<bool>,
	pub suppress_roles: Patch<bool>,
	pub hide_muted: Patch<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
	pub channel: Id,
	pub fields: Fields,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
	pub guild: Option<Id>,
	pub version: Option<u64>,
	/// Full records replace the override collection; sparse acknowledgements preserve omissions.
	pub full: bool,
	pub fields: Fields,
	pub channels: Option<Vec<Row>>,
	/// Explicit null makes the override collection unknown; None alone means absent.
	pub channels_unknown: bool,
}
impl Update {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self
				.channels
				.as_ref()
				.map_or(0, |rows| rows.capacity() * size_of::<Row>())
	}
	pub fn valid(&self) -> bool {
		let rows = self.channels.as_deref().unwrap_or_default();
		(!self.channels_unknown || self.channels.is_none())
			&& self.guild.is_none_or(|guild| guild.0 != 0)
			&& self.bytes() <= 32 * 1024 * 1024
			&& rows.len() <= crate::account::MAX_ENTRIES
			&& self.fields.collapsed == Patch::Absent
			&& rows
				.iter()
				.map(|row| row.channel)
				.collect::<std::collections::BTreeSet<_>>()
				.len() == rows.len()
			&& rows.iter().all(|row| {
				row.channel.0 != 0
					&& row.fields.suppress_everyone == Patch::Absent
					&& row.fields.suppress_roles == Patch::Absent
					&& row.fields.hide_muted == Patch::Absent
			}) && std::iter::once(&self.fields)
			.chain(rows.iter().map(|row| &row.fields))
			.all(|fields| {
				!matches!(fields.level,Patch::Value(level) if level>3)
					&& !matches!(fields.mute_until,Patch::Value(until) if until<0)
			})
	}
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Write {
	pub fields: Fields,
	pub channels: Vec<Row>,
}
impl Write {
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.channels.capacity() * size_of::<Row>()
	}
	pub fn valid(&self) -> bool {
		self.channels.len() <= MAX_EDITS
			&& self.bytes() <= MAX_PENDING_BYTES
			&& self.fields.collapsed == Patch::Absent
			&& self.channels.iter().enumerate().all(|(index, row)| {
				row.channel.0 != 0
					&& !self.channels[..index]
						.iter()
						.any(|other| other.channel == row.channel)
					&& row.fields.suppress_everyone == Patch::Absent
					&& row.fields.suppress_roles == Patch::Absent
					&& row.fields.hide_muted == Patch::Absent
			}) && std::iter::once(&self.fields)
			.chain(self.channels.iter().map(|row| &row.fields))
			.all(|fields| {
				!matches!(fields.level, Patch::Value(level) if level > 3)
					&& !matches!(fields.mute_until, Patch::Value(until) if until < 0)
			})
	}
}
