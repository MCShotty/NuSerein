//! Owned profile collectibles, never purchases or claims. Session RAM only.
use crate::Id;

pub const MAX_ITEMS: usize = 256;
pub const MAX_BYTES: usize = 128 * 1024;
pub const MAX_EQUIPPED: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	Decoration,
	Effect,
	Nameplate,
	Frame,
}
impl Kind {
	pub fn wire(self) -> u32 {
		match self {
			Self::Decoration => 0,
			Self::Effect => 1,
			Self::Nameplate => 2,
			Self::Frame => 3,
		}
	}
	pub fn from_wire(value: u32) -> Option<Self> {
		match value {
			0 => Some(Self::Decoration),
			1 => Some(Self::Effect),
			2 => Some(Self::Nameplate),
			3 => Some(Self::Frame),
			_ => None,
		}
	}
	pub fn label(self) -> &'static str {
		match self {
			Self::Decoration => "Avatar decoration",
			Self::Effect => "Profile effect",
			Self::Nameplate => "Nameplate",
			Self::Frame => "Profile frame",
		}
	}
}
#[derive(Clone)]
pub struct Item {
	pub sku: Id,
	pub kind: Kind,
	pub name: String,
	pub expires_at: Option<u64>,
	pub subscription: bool,
}
impl Item {
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.name.capacity()
	}
	pub fn available(&self, nitro: bool, now: u64) -> bool {
		self.expires_at.is_none_or(|expires| expires > now) && (!self.subscription || nitro)
	}
}
#[derive(Clone, Default)]
pub struct Inventory {
	pub items: Vec<Item>,
	/// Unsupported product kinds are not guessed into editable entitlements.
	pub unsupported: bool,
}
impl Inventory {
	pub fn bytes(&self) -> usize {
		size_of::<Self>()
			+ self.items.capacity() * size_of::<Item>()
			+ self
				.items
				.iter()
				.map(|item| item.name.capacity())
				.sum::<usize>()
	}
	pub fn valid(&self) -> bool {
		self.items.capacity() <= MAX_ITEMS
			&& self.bytes() <= MAX_BYTES
			&& self.items.iter().enumerate().all(|(index, item)| {
				item.sku.0 != 0
					&& !item.name.is_empty()
					&& item.name.len() <= 512
					&& item.name.chars().count() <= 128
					&& !item.name.chars().any(char::is_control)
					&& !self.items[..index]
						.iter()
						.any(|old| old.sku == item.sku && old.kind == item.kind)
			})
	}
	pub fn find(&self, kind: Kind, sku: Id) -> Option<&Item> {
		self.items
			.iter()
			.find(|item| item.kind == kind && item.sku == sku)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Equipped {
	pub kind: u32,
	pub sku: Id,
}
#[derive(Clone)]
pub struct Current {
	pub user: Id,
	pub guild: Option<Id>,
	/// Fresh own-profile entitlement; absent values never authorize subscription equips.
	pub premium_type: Option<u32>,
	pub decoration: Option<Id>,
	pub nameplate: Option<Id>,
	/// Keep all bounded profile entries, including unknown kinds, for replacement writes.
	pub profile: Vec<Equipped>,
}
impl Current {
	pub fn valid(&self) -> bool {
		self.user.0 != 0
			&& self.guild.is_none_or(|guild| guild.0 != 0)
			&& self.decoration.is_none_or(|sku| sku.0 != 0)
			&& self.nameplate.is_none_or(|sku| sku.0 != 0)
			&& self.profile.capacity() <= MAX_EQUIPPED
			&& self.profile.iter().enumerate().all(|(index, item)| {
				item.sku.0 != 0
					&& !self.profile[..index].iter().any(|old| {
						old.sku == item.sku || (matches!(item.kind, 1 | 3) && old.kind == item.kind)
					})
			})
	}
	pub fn equipped(&self, kind: Kind) -> Option<Id> {
		match kind {
			Kind::Decoration => self.decoration,
			Kind::Nameplate => self.nameplate,
			Kind::Effect | Kind::Frame => self
				.profile
				.iter()
				.find(|item| item.kind == kind.wire())
				.map(|item| item.sku),
		}
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.profile.capacity() * size_of::<Equipped>()
	}
}
#[derive(Clone)]
pub struct Snapshot {
	pub inventory: Inventory,
	pub current: Current,
}
impl Snapshot {
	pub fn valid(&self) -> bool {
		self.inventory.valid() && self.current.valid()
	}
	pub fn bytes(&self) -> usize {
		size_of::<Self>() + self.inventory.bytes() + self.current.bytes()
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
	pub kind: Kind,
	pub sku: Option<Id>,
}
impl Change {
	pub fn allowed(&self, snapshot: &Snapshot, nitro: bool, now: u64) -> bool {
		let entitled = nitro && snapshot.current.premium_type == Some(2);
		let mut expected = snapshot.current.clone();
		self.apply(&mut expected);
		snapshot.valid()
			&& expected.valid()
			&& self.sku.is_none_or(|sku| {
				(snapshot.current.guild.is_none() || entitled)
					&& snapshot
						.inventory
						.find(self.kind, sku)
						.is_some_and(|item| item.available(entitled, now))
			})
	}
	pub fn apply(&self, current: &mut Current) {
		match self.kind {
			Kind::Decoration => current.decoration = self.sku,
			Kind::Nameplate => current.nameplate = self.sku,
			Kind::Effect | Kind::Frame => {
				current.profile.retain(|item| item.kind != self.kind.wire());
				if let Some(sku) = self.sku {
					current.profile.push(Equipped {
						kind: self.kind.wire(),
						sku,
					});
				}
				current.profile.shrink_to_fit();
			}
		}
	}
}
