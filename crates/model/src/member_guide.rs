//! Ordinary-member Server Guide progress, independent of administrator configuration.
use crate::{Id, community_admin::Guide};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
	Load,
	Complete { channel: Id, kind: u8 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
	pub guild: Id,
	pub user: Id,
	pub guide: Guide,
	pub completed: Vec<Id>,
	/// Header metadata is fetched separately; completion responses do not replace it.
	pub header_known: bool,
}
impl Snapshot {
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.user.0 != 0
			&& self.guide.valid()
			&& self.completed.len() <= crate::community_admin::MAX_ACTIONS
			&& self.completed.iter().enumerate().all(|(i, id)| {
				self.guide.actions.iter().any(|r| r.channel == *id)
					&& !self.completed[..i].contains(id)
			}) && self.bytes() <= crate::community_admin::MAX_BYTES
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.guide.bytes() + self.completed.capacity() * size_of::<Id>()
	}
}
