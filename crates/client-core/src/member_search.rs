//! Bounded, session-only composer and visible-author member lookups.
use crate::{Command, State, auth::Failure};
use model::{Id, Member};

pub const LIMIT: usize = 100;
pub const MAX_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
	pub guild: Id,
	pub channel: Id,
	pub query: String,
	pub users: Vec<Id>,
	pub nonce: u64,
	pub slot: usize,
}
impl Request {
	pub fn valid(&self) -> bool {
		self.guild.0 != 0
			&& self.channel.0 != 0
			&& match self.slot {
				0 => {
					self.users.is_empty()
						&& !self.query.is_empty()
						&& self.query.len() <= 256
						&& self.query.chars().count() <= 64
						&& !self.query.chars().any(char::is_control)
				}
				1 => {
					self.query.is_empty()
						&& !self.users.is_empty()
						&& self.users.len() <= LIMIT
						&& self.users.iter().all(|id| id.0 != 0)
						&& self.users.windows(2).all(|ids| ids[0] < ids[1])
				}
				_ => false,
			}
	}
}
#[derive(Default)]
pub struct View {
	pub request: Option<Request>,
	pub rows: Vec<Member>,
	/// Recently resolved labels for the same conversation, bounded like one search page.
	pub retained: Vec<Member>,
	pub finished: bool,
	pub error: Option<&'static str>,
	pub identity_revision: u64,
}
impl State {
	pub(crate) fn refresh_member_labels(&mut self, list: &model::MemberList) {
		self.member_labels.refresh(list);
		if self
			.member_search
			.iter()
			.all(|view| view.rows.is_empty() && view.retained.is_empty())
		{
			return;
		}
		let updates: std::collections::BTreeMap<_, _> = list
			.slots
			.iter()
			.flatten()
			.filter_map(|slot| match slot {
				model::MemberSlot::Person(member) => Some((member.user.id, member)),
				_ => None,
			})
			.collect();
		for view in &mut self.member_search {
			if view.request.as_ref().is_none_or(|request| {
				request.channel != list.channel || Some(request.guild) != list.guild
			}) {
				continue;
			}
			for rows in [&mut view.rows, &mut view.retained] {
				for member in rows.iter_mut() {
					if let Some(updated) = updates.get(&member.user.id) {
						*member = (*updated).clone();
					}
				}
				let mut bytes = rows.iter().map(Member::bytes).sum::<usize>();
				while bytes > MAX_BYTES {
					bytes -= rows.remove(0).bytes();
				}
				rows.shrink_to(LIMIT);
			}
		}
	}

	/// Presentation metadata only; callers must use the permission state to authorize actions.
	pub fn display_member(&self, channel: Id, user: Id) -> Option<&Member> {
		let guild = self.channel(channel)?.guild?;
		if !self.can_view(channel) {
			return None;
		}
		self.members
			.as_ref()
			.filter(|list| list.guild == Some(guild) && list.channel == channel)
			.and_then(|list| {
				list.slots.iter().flatten().find_map(|slot| match slot {
					model::MemberSlot::Person(member) if member.user.id == user => Some(member),
					_ => None,
				})
			})
			.or_else(|| {
				self.member_search.iter().find_map(|view| {
					view.request
						.as_ref()
						.filter(|request| request.channel == channel && request.guild == guild)?;
					view.rows.iter().find(|member| member.user.id == user)
				})
			})
			.or_else(|| {
				self.member_search.iter().find_map(|view| {
					view.request
						.as_ref()
						.filter(|request| request.channel == channel && request.guild == guild)?;
					view.retained.iter().find(|member| member.user.id == user)
				})
			})
	}

	pub fn search_members(&mut self, channel: Id, query: &str, slot: usize) -> Option<Command> {
		let guild = self.channel(channel)?.guild?;
		if slot != 0
			|| !self.gateway_connected
			|| !self.can_view(channel)
			|| self.selected != Some(channel)
		{
			return None;
		}
		self.member_search_nonce = self.member_search_nonce.wrapping_add(1);
		let request = Request {
			guild,
			channel,
			query: query.to_owned(),
			users: vec![],
			nonce: self.member_search_nonce,
			slot,
		};
		if !request.valid() {
			return None;
		}
		let previous = &mut self.member_search[slot];
		let retained = if previous
			.request
			.as_ref()
			.is_some_and(|old| old.channel == channel && old.guild == guild)
		{
			let mut retained = std::mem::take(&mut previous.retained);
			for member in std::mem::take(&mut previous.rows) {
				retained.retain(|old| old.user.id != member.user.id);
				retained.push(member);
				while retained.len() > LIMIT
					|| retained.iter().map(Member::bytes).sum::<usize>() > MAX_BYTES
				{
					retained.remove(0);
				}
			}
			retained.shrink_to(LIMIT);
			retained
		} else {
			vec![]
		};
		self.member_search[slot] = View {
			retained,
			identity_revision: self.member_labels.revision(),
			request: Some(request.clone()),
			..Default::default()
		};
		Some(Command::MemberSearch(request))
	}
	pub fn request_author_members(&mut self, users: &[Id]) -> Option<Command> {
		let channel = self.selected?;
		let guild = self.channel(channel)?.guild?;
		if self.demo || !self.gateway_connected || !self.can_view(channel) {
			return None;
		}
		let mut users: Vec<_> = users
			.iter()
			.copied()
			.filter(|id| id.0 != 0)
			.filter(|id| self.display_member(channel, *id).is_none())
			.filter(|id| {
				!self.member_identity(channel, *id).is_some_and(|update| {
					update.departed
						|| (!matches!(update.nick, model::Patch::Absent)
							&& matches!(update.roles, model::Patch::Value(_)))
				})
			})
			.take(LIMIT)
			.collect();
		users.sort_unstable();
		users.dedup();
		let view = &self.member_search[1];
		if users.is_empty()
			|| view.request.as_ref().is_some_and(|request| {
				request.channel == channel
					&& request.guild == guild
					&& (!view.finished || users.iter().all(|user| request.users.contains(user)))
			}) {
			return None;
		}
		self.member_search_nonce = self.member_search_nonce.wrapping_add(1);
		let request = Request {
			guild,
			channel,
			query: String::new(),
			users,
			nonce: self.member_search_nonce,
			slot: 1,
		};
		let (rows, retained) = if view
			.request
			.as_ref()
			.is_some_and(|old| old.channel == channel && old.guild == guild)
		{
			(
				std::mem::take(&mut self.member_search[1].rows),
				std::mem::take(&mut self.member_search[1].retained),
			)
		} else {
			(vec![], vec![])
		};
		self.member_search[1] = View {
			rows,
			retained,
			identity_revision: self.member_labels.revision(),
			request: Some(request.clone()),
			..Default::default()
		};
		Some(Command::MemberSearch(request))
	}
	pub(crate) fn searched_members(
		&mut self,
		request: Request,
		result: Result<Vec<Member>, Failure>,
	) {
		if !request.valid()
			|| self.selected != Some(request.channel)
			|| !self.gateway_connected
			|| !self.can_view(request.channel)
			|| self.channel(request.channel).and_then(|c| c.guild) != Some(request.guild)
			|| self.member_search[request.slot].request.as_ref() != Some(&request)
		{
			return;
		}
		self.member_search[request.slot].finished = true;
		match result {
			Ok(mut rows)
				if rows.len() <= LIMIT
					&& rows.iter().all(Member::valid)
					&& (request.users.is_empty()
						|| rows
							.iter()
							.all(|member| request.users.contains(&member.user.id)))
					&& rows.iter().map(Member::bytes).sum::<usize>() <= MAX_BYTES =>
			{
				self.member_labels.reconcile(
					request.guild,
					&mut rows,
					self.member_search[request.slot].identity_revision,
				);
				if rows.iter().map(Member::bytes).sum::<usize>() > MAX_BYTES {
					self.member_search[request.slot].error =
						Some("Member search exceeded its safety limit");
					return;
				}
				if request.slot == 1 {
					for member in &rows {
						self.timeline.apply_author_membership(
							member.user.id,
							&member.roles,
							member.nick.as_deref(),
						);
					}
				}
				let view = &mut self.member_search[request.slot];
				for member in std::mem::replace(&mut view.rows, rows) {
					view.retained.retain(|old| old.user.id != member.user.id);
					view.retained.push(member);
				}
				let mut bytes = view.retained.iter().map(Member::bytes).sum::<usize>();
				while view.retained.len() > LIMIT || bytes > MAX_BYTES {
					bytes -= view.retained.remove(0).bytes();
				}
			}
			Ok(_) => {
				self.member_search[request.slot].error =
					Some("Member search exceeded its safety limit");
			}
			Err(error) => self.member_search[request.slot].error = Some(error.label()),
		}
	}
}
