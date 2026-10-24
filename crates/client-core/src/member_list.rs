//! Bounded member-list requests and cached ranges.
use super::{Command, Freshness, Id, Member, MemberList, MemberSlot, State};

impl State {
	pub(super) fn shared_member_list_id(&self, next: Id) -> Option<String> {
		if self.freshness == Freshness::Unavailable {
			return None;
		}
		let list = self.members.as_ref()?;
		if list.channel == next || !list.lazy || list.freshness == Freshness::Unavailable {
			return None;
		}
		let next_channel = self.channel(next)?.clone();
		if next_channel.guild != list.guild || matches!(next_channel.kind, 10..=12) {
			return None;
		}
		let next_id = self.member_list_id(&next_channel)?;
		let current = self.channel(list.channel)?.clone();
		self.member_list_id(&current)
			.filter(|current| current == &next_id)
	}
	pub fn request_members(&mut self) -> Option<Command> {
		let index = self.channel_index(self.selected?)?;
		let channel_id = self.channels[index].id;
		if let Some(list_id) = self.shared_member_list_id(channel_id) {
			let (request, ranges, guild) = {
				let list = self.members.as_mut()?;
				list.channel = channel_id;
				(list.request, list.ranges.clone(), list.guild)
			};
			return Some(Command::Members {
				thread: false,
				guild,
				channel: Some(channel_id),
				request,
				list_id: Some(list_id),
				ranges,
			});
		}
		let channel = &self.channels[index];
		self.member_request = self.member_request.wrapping_add(1);
		self.member_chunks.clear();
		if !self.can_view(channel.id) {
			return None;
		}
		let thread = matches!(channel.kind, 10..=12);
		let list_id = self.member_list_id(channel);
		let lazy = channel.guild.is_some() && !thread;
		let ranges = if lazy && list_id.is_some() && self.freshness != Freshness::Unavailable {
			vec![[0, 99]]
		} else {
			vec![]
		};
		let slots = if channel.guild.is_none() && self.freshness != Freshness::Unavailable {
			let mut users = channel.recipients.clone();
			if let Some(user) = &self.user
				&& !users.iter().any(|u| u.id == user.id)
			{
				users.push(user.clone());
			}
			users
				.into_iter()
				.map(|user| {
					Some(MemberSlot::Person(Member {
						roles: vec![],
						nick: None,
						status: None,
						custom_status: None,
						activities: vec![],
						clients: model::ClientPlatforms::default(),
						user,
					}))
				})
				.collect()
		} else {
			vec![]
		};
		let freshness = if self.freshness == Freshness::Unavailable {
			Freshness::Unavailable
		} else if channel.guild.is_none() {
			Freshness::Fresh
		} else if !thread && list_id.is_none() {
			Freshness::Unavailable
		} else {
			Freshness::Loading
		};
		self.members = Some(MemberList {
			guild: channel.guild,
			channel: channel.id,
			request: self.member_request,
			start: 0,
			total: if lazy { 0 } else { slots.len() as u64 },
			slots,
			lazy,
			freshness,
			groups: vec![],
			ranges: ranges.clone(),
		});
		let command = Command::Members {
			thread,
			guild: channel.guild.filter(|_| {
				(thread || list_id.is_some()) && self.freshness != Freshness::Unavailable
			}),
			channel: Some(channel.id),
			request: self.member_request,
			list_id,
			ranges,
		};
		self.apply_direct_presence(&[]);
		Some(command)
	}
	pub fn close_members(&mut self) -> Command {
		self.member_request = self.member_request.wrapping_add(1);
		self.member_chunks.clear();
		self.members = None;
		Command::Members {
			thread: false,
			guild: None,
			channel: None,
			request: self.member_request,
			list_id: None,
			ranges: vec![],
		}
	}
	pub fn member_slot(&self, index: usize) -> Option<&MemberSlot> {
		let list = self.members.as_ref()?;
		if !list.lazy || self.member_chunks.request != list.request {
			return None;
		}
		self.member_chunks.slot(index)
	}
	pub fn members_cached(&self) -> bool {
		self.members.as_ref().is_some_and(|list| {
			list.lazy && self.member_chunks.request == list.request && self.member_chunks.occupied()
		})
	}
	pub fn focus_member_ranges(&mut self, first: usize, last: usize) -> Option<Command> {
		let list = self.members.as_ref()?;
		if !list.lazy
			|| Some(list.channel) != self.selected
			|| list.freshness == Freshness::Unavailable
			|| !self.can_view(list.channel)
			|| self.freshness == Freshness::Unavailable
		{
			return None;
		}
		let index = self.channel_index(list.channel)?;
		let channel = &self.channels[index];
		if channel.guild.is_none() || matches!(channel.kind, 10..=12) {
			return None;
		}
		let list_id = self.member_list_id(channel)?;
		let max_idx = if list.total > 0 {
			(list.total as usize).saturating_sub(1)
		} else {
			0
		};
		let first = first.min(max_idx);
		let last = last.min(max_idx).max(first);
		let chunk = |i: usize| -> [usize; 2] {
			let start = (i / 100) * 100;
			[start, start + 99]
		};
		let mut ranges = vec![chunk(first)];
		let last_chunk = chunk(last);
		if last_chunk != ranges[0] {
			if last_chunk[0] > ranges[0][1].saturating_add(1) {
				let start = last_chunk[0] - 100;
				ranges = vec![[start, start + 99], last_chunk];
			} else {
				ranges.push(last_chunk);
			}
		}
		let unchanged = ranges == list.ranges;
		if unchanged {
			self.member_chunks.look_at(first, &ranges);
			return None;
		}
		let request = list.request;
		let guild = list.guild;
		let channel_id = list.channel;
		self.members.as_mut()?.ranges = ranges.clone();
		self.member_chunks.look_at(first, &ranges);
		Some(Command::Members {
			thread: false,
			guild,
			channel: Some(channel_id),
			request,
			list_id: Some(list_id),
			ranges,
		})
	}
	pub(super) fn invalidate_members(&mut self) {
		self.member_search = Default::default();
		self.member_request = self.member_request.wrapping_add(1);
		self.member_chunks.clear();
		if let Some(list) = &mut self.members {
			list.request = self.member_request;
			list.slots.clear();
			list.start = 0;
			list.groups.clear();
			list.ranges.clear();
			list.freshness = Freshness::Unavailable;
		}
	}
}
