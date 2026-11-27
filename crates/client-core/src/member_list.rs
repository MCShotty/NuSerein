//! Bounded member-list requests and cached ranges.
use super::{Command, Freshness, Id, Member, MemberList, MemberSlot, State};

const FULL_SERVER_RANGES: [[usize; 2]; 2] = [[0, 99], [100, 199]];
const FULL_ROSTER_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Clone, Copy, Debug)]
pub(super) struct FullRosterLoad {
	generation: u64,
	channel: Id,
	request: u64,
	deadline: std::time::Instant,
}

fn complete_server_window(list: &MemberList) -> Option<u16> {
	let count = list
		.server_member_count
		.filter(|evidence| evidence.full_sync && (1..=200).contains(&evidence.count))?
		.count;
	let total = usize::try_from(list.total).ok()?.min(201);
	if list.start != 0 || total > 200 || list.groups.is_empty() || list.slots.len() < total {
		return None;
	}
	let mut rows = list.slots[..total].iter();
	let mut users = std::collections::HashSet::with_capacity(usize::from(count));
	let mut groups = std::collections::HashSet::with_capacity(list.groups.len().min(total));
	for (id, length) in &list.groups {
		if !groups.insert(id)
			|| !matches!(rows.next(), Some(Some(MemberSlot::Group(actual))) if actual == id)
		{
			return None;
		}
		for _ in 0..(*length).min(201) {
			if !matches!(rows.next(), Some(Some(MemberSlot::Person(member))) if member.user.id.0 != 0 && users.insert(member.user.id))
			{
				return None;
			}
		}
	}
	(rows.next().is_none()
		&& users.len() == usize::from(count)
		&& list.slots[total..].iter().all(Option::is_none))
	.then_some(count)
}

impl State {
	fn server_roster_window(&self, channel: Id) -> Option<&MemberList> {
		let list = self.members.as_ref()?;
		let row = self.channel(channel)?;
		(self.gateway_connected
			&& self.freshness == Freshness::Fresh
			&& self.selected == Some(channel)
			&& list.channel == channel
			&& list.freshness == Freshness::Fresh
			&& list.lazy
			&& list.guild.is_some()
			&& list.guild == row.guild
			&& self.can_view(channel)
			&& self.member_list_id(row).as_deref() == Some("everyone")
			&& list.server_member_count.is_some()
			&& list.total <= 200)
			.then_some(list)
	}
	pub fn complete_server_roster(&self, channel: Id) -> Option<u16> {
		complete_server_window(self.server_roster_window(channel)?)
	}
	fn current_roster_load(&self, channel: Id) -> Option<FullRosterLoad> {
		let list = self.members.as_ref()?;
		self.full_roster_load.filter(|load| {
			load.channel == channel
				&& Some(channel) == self.selected
				&& load.generation == self.generation
				&& list.channel == channel
				&& list.request == load.request
				&& self.can_view(channel)
				&& self.complete_server_roster(channel).is_none()
		})
	}

	/// An explicit requested roster is not a completion until the same window passes proof.
	pub fn full_roster_wait(&self, channel: Id) -> Option<std::time::Duration> {
		self.current_roster_load(channel).map(|load| {
			load.deadline
				.saturating_duration_since(std::time::Instant::now())
		})
	}

	pub fn can_load_full_server_roster(&self, channel: Id) -> bool {
		self.full_roster_wait(channel)
			.is_none_or(|wait| wait.is_zero())
			&& self
				.server_roster_window(channel)
				.is_some_and(|list| list.total > 100 && complete_server_window(list).is_none())
	}

	pub(super) fn reject_full_roster(
		&mut self,
		channel: Id,
		request: u64,
		ranges: &[[usize; 2]],
	) -> bool {
		if ranges != FULL_SERVER_RANGES
			|| !self
				.current_roster_load(channel)
				.is_some_and(|load| load.request == request)
		{
			return false;
		}
		if let Some(load) = &mut self.full_roster_load {
			load.deadline = std::time::Instant::now();
		}
		true
	}
	pub fn load_full_server_roster(&mut self, channel: Id) -> Option<Command> {
		if !self.can_load_full_server_roster(channel) {
			return None;
		}
		let list = self.members.as_mut()?;
		let ranges = FULL_SERVER_RANGES.to_vec();
		self.full_roster_load = Some(FullRosterLoad {
			generation: self.generation,
			channel,
			request: list.request,
			deadline: std::time::Instant::now() + FULL_ROSTER_WAIT,
		});
		list.ranges.clone_from(&ranges);
		self.member_chunks.look_at(0, &ranges);
		Some(Command::Members {
			thread: false,
			guild: list.guild,
			channel: Some(channel),
			request: list.request,
			list_id: Some("everyone".into()),
			ranges,
		})
	}
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
		if let Some(mut parked) = self.parked_members.take()
			&& parked.generation == self.generation
			&& parked.parked.elapsed() < std::time::Duration::from_secs(5 * 60)
			&& self.freshness != Freshness::Unavailable
			&& self.can_view(channel_id)
			&& self.channel(channel_id).is_some_and(|channel| {
				channel.guild == parked.list.guild
					&& !matches!(channel.kind, 10..=12)
					&& self.member_list_id(channel).as_deref() == Some(parked.list_id.as_str())
			}) {
			self.member_request = self.member_request.wrapping_add(1);
			parked.list.channel = channel_id;
			parked.list.request = self.member_request;
			parked.list.freshness = Freshness::Stale;
			parked.chunks.request = self.member_request;
			let command = Command::Members {
				thread: false,
				guild: parked.list.guild,
				channel: Some(channel_id),
				request: self.member_request,
				list_id: Some(parked.list_id),
				ranges: parked.list.ranges.clone(),
			};
			self.member_chunks = parked.chunks;
			self.members = Some(parked.list);
			return Some(command);
		}
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
			server_member_count: None,
			missing_identity: vec![],
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
		let list_id = self
			.members
			.as_ref()
			.filter(|list| list.lazy && list.freshness != Freshness::Unavailable)
			.and_then(|list| self.channel(list.channel))
			.filter(|channel| self.can_view(channel.id))
			.and_then(|channel| self.member_list_id(channel));
		self.parked_members = match (self.members.take(), list_id) {
			(Some(list), Some(list_id)) => Some(crate::ParkedMembers {
				list,
				chunks: std::mem::take(&mut self.member_chunks),
				list_id,
				generation: self.generation,
				parked: std::time::Instant::now(),
			}),
			_ => {
				self.member_chunks.clear();
				None
			}
		};
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
		if list_id == "everyone"
			&& list.server_member_count.is_some()
			&& list.total <= 200
			&& list.ranges.as_slice() == FULL_SERVER_RANGES
		{
			self.member_chunks.look_at(first, &FULL_SERVER_RANGES);
			return None;
		}
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
		self.full_roster_load = None;
		self.parked_members = None;
		self.member_search = Default::default();
		self.member_labels.clear();
		self.member_request = self.member_request.wrapping_add(1);
		self.member_chunks.clear();
		if let Some(list) = &mut self.members {
			list.request = self.member_request;
			list.slots.clear();
			list.start = 0;
			list.groups.clear();
			list.server_member_count = None;
			list.ranges.clear();
			list.freshness = Freshness::Unavailable;
		}
	}
}
