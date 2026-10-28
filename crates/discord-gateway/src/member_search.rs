//! On-demand Gateway member queries; never requests a complete guild directory.
use client_core::{
	Event,
	auth::Failure,
	member_search::{LIMIT, MAX_BYTES, Request},
};
use discord_protocol::{MemberDto, MemberItem};
use model::Id;
use serde::Deserialize;
use std::collections::BTreeMap;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message as Frame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Purpose {
	Composer,
	Authors,
}
impl Purpose {
	pub fn slot(self) -> usize {
		match self {
			Self::Composer => 0,
			Self::Authors => 1,
		}
	}
}
pub(super) struct Query {
	pub purpose: Purpose,
	pub frame: Frame,
}

#[derive(Deserialize)]
struct Chunk {
	guild_id: Id,
	nonce: Option<String>,
	chunk_index: usize,
	chunk_count: usize,
	members: Vec<MemberDto>,
}
pub(super) struct Search {
	pub queued: [Option<Request>; 2],
	active: [Option<(Request, Option<Instant>)>; 2],
	cooldowns: BTreeMap<Id, Instant>,
	blocked_until: Option<Instant>,
	next_send: Instant,
	seen: [Option<u64>; 2],
	reported: [Option<u64>; 2],
	following: Option<(Id, Id)>,
	users: Vec<Id>,
	announced: Option<(Id, Vec<Id>)>,
}
impl Default for Search {
	fn default() -> Self {
		Self {
			queued: Default::default(),
			active: Default::default(),
			cooldowns: BTreeMap::new(),
			blocked_until: None,
			seen: [None; 2],
			reported: [None; 2],
			following: None,
			users: vec![],
			announced: None,
			next_send: Instant::now(),
		}
	}
}
impl Search {
	pub fn reconnect(&mut self, resumable: bool) {
		self.announced = None;
		self.active = Default::default();
		self.queued = Default::default();
		self.seen = [None; 2];
		self.reported = [None; 2];
		if !resumable {
			self.following = None;
			self.users.clear();
			self.cooldowns.clear();
			self.blocked_until = None;
		}
	}
	pub fn update(&mut self, requests: &[Option<Request>; 2]) -> u8 {
		let scope = requests
			.iter()
			.flatten()
			.filter(|r| r.valid())
			.max_by_key(|r| r.nonce)
			.map(|r| (r.guild, r.channel));
		if self.following != scope {
			self.following = scope;
			self.users.clear();
		}
		for request in requests
			.iter()
			.flatten()
			.filter(|r| Some((r.guild, r.channel)) == scope && r.valid())
		{
			self.follow_users(request.users.iter().copied());
		}
		let mut changed = 0;
		for (slot, request) in requests.iter().enumerate() {
			if self.seen[slot] != request.as_ref().map(|request| request.nonce) {
				self.seen[slot] = request.as_ref().map(|request| request.nonce);
				self.active[slot] = None;
				self.queued[slot] = request.clone();
				changed |= 1 << slot;
			}
		}
		changed
	}
	fn follow_users(&mut self, users: impl IntoIterator<Item = Id>) {
		for user in users {
			self.users.retain(|known| *known != user);
			if self.users.len() == 128 {
				self.users.remove(0);
			}
			self.users.push(user);
		}
	}
	pub fn pending_subscription(&self) -> Option<(Id, Vec<Id>, Frame)> {
		let wanted = self
			.following
			.filter(|_| !self.users.is_empty())
			.map(|scope| scope.0);
		let (guild, users) = match (&self.announced, wanted) {
			(Some((old, _)), next) if Some(*old) != next => (*old, vec![]),
			(Some((_, sent)), Some(_)) if sent == &self.users => return None,
			(_, Some(guild)) => (guild, self.users.clone()),
			_ => return None,
		};
		// A sparse update leaves channel ranges, thread lists and typing subscriptions intact.
		let ids: Vec<_> = users.iter().map(ToString::to_string).collect();
		let frame = Frame::Text(
			serde_json::json!({"op":37,"d":{"subscriptions":{guild.to_string():{"members":ids}}}})
				.to_string()
				.into(),
		);
		Some((guild, users, frame))
	}
	pub fn subscribed(&mut self, guild: Id, users: Vec<Id>) {
		self.announced = (!users.is_empty()).then_some((guild, users));
	}
	pub fn canceled_subscription(&mut self, guild: Id) {
		if self
			.announced
			.as_ref()
			.is_some_and(|(old, _)| *old == guild)
		{
			self.announced = None;
		}
	}
	pub fn report_cooldowns(
		&mut self,
		emit: &impl Fn(Event) -> Result<(), Failure>,
	) -> Result<(), Failure> {
		let now = Instant::now();
		for (slot, request) in self.queued.iter().enumerate() {
			if let Some(request) = request
				&& self.reported[slot] != Some(request.nonce)
				&& (self
					.cooldowns
					.get(&request.guild)
					.is_some_and(|at| *at > now)
					|| self.blocked_until.is_some_and(|at| at > now))
			{
				self.reported[slot] = Some(request.nonce);
				emit(Event::MemberSearch {
					request: request.clone(),
					result: Err(Failure::RateLimited),
				})?;
			}
		}
		Ok(())
	}
	pub fn deadline(&self) -> Option<Instant> {
		self.active
			.iter()
			.flatten()
			.filter_map(|(_, deadline)| *deadline)
			.chain(self.queued.iter().flatten().map(|request| {
				self.cooldowns
					.get(&request.guild)
					.copied()
					.unwrap_or(self.next_send)
					.max(self.next_send)
					.max(self.blocked_until.unwrap_or(self.next_send))
			}))
			.min()
	}
	pub fn tick(
		&mut self,
		emit: &impl Fn(Event) -> Result<(), Failure>,
	) -> Result<Option<Query>, Failure> {
		self.tick_at(Instant::now(), emit)
	}
	fn tick_at(
		&mut self,
		now: Instant,
		emit: &impl Fn(Event) -> Result<(), Failure>,
	) -> Result<Option<Query>, Failure> {
		self.cooldowns.retain(|_, deadline| *deadline > now);
		for active in &mut self.active {
			if active
				.as_ref()
				.is_some_and(|(_, deadline)| deadline.is_some_and(|at| now >= at))
			{
				let (request, _) = active.take().unwrap();
				emit(Event::MemberSearch {
					request,
					result: Err(Failure::ProtocolAt("Member search timed out; try again")),
				})?;
			}
		}
		if now < self.next_send || self.blocked_until.is_some_and(|at| now < at) {
			return Ok(None);
		}
		let Some(slot) = self.queued.iter().position(|request| {
			request.as_ref().is_some_and(|request| {
				self.cooldowns
					.get(&request.guild)
					.is_none_or(|at| *at <= now)
			})
		}) else {
			return Ok(None);
		};
		let request = self.queued[slot].take().unwrap();
		if !request.valid() {
			return Ok(None);
		}
		// Only two queued searches, and 64 retained unexpired guild cooldowns.
		// Capacity cannot erase an active cooldown and accidentally permit early traffic.
		if self.cooldowns.len() >= 64 && !self.cooldowns.contains_key(&request.guild) {
			self.blocked_until = self.cooldowns.values().copied().max();
			self.queued[slot] = Some(request);
			return Ok(None);
		}
		self.cooldowns
			.insert(request.guild, now + Duration::from_secs(30));
		let mut data = serde_json::json!({"guild_id":request.guild.to_string(),"limit":LIMIT,"nonce":request.nonce.to_string()});
		if !request.users.is_empty() {
			data["user_ids"] = serde_json::json!(
				request
					.users
					.iter()
					.map(ToString::to_string)
					.collect::<Vec<_>>()
			);
		} else if let Ok(id) = request.query.parse::<u64>()
			&& id != 0
		{
			data["user_ids"] = serde_json::json!([id.to_string()]);
		} else {
			data["query"] = serde_json::json!(request.query);
		}
		self.active[slot] = Some((request, None));
		self.next_send = now + Duration::from_secs(1);
		Ok(Some(Query {
			purpose: if slot == 0 {
				Purpose::Composer
			} else {
				Purpose::Authors
			},
			frame: Frame::Text(serde_json::json!({"op":8,"d":data}).to_string().into()),
		}))
	}
	pub fn sent(&mut self, guild: Id, nonce: u64, sent: Instant) {
		// Keep the service session's cooldown even when the old view was replaced
		// before this completion arrived. The writer knows its real send time.
		let until = sent + Duration::from_secs(30);
		if self.cooldowns.len() < 64 || self.cooldowns.contains_key(&guild) {
			self.cooldowns
				.entry(guild)
				.and_modify(|at| *at = (*at).max(until))
				.or_insert(until);
		} else {
			self.blocked_until = Some(self.blocked_until.map_or(until, |at| at.max(until)));
		}
		for (request, deadline) in self.active.iter_mut().flatten() {
			if request.nonce == nonce && deadline.is_none() {
				*deadline = Some(sent + Duration::from_secs(15));
				self.cooldowns
					.insert(request.guild, sent + Duration::from_secs(30));
			}
		}
	}
	pub fn limited(
		&mut self,
		guild: Option<Id>,
		nonce: Option<&str>,
		delay: Duration,
		emit: &impl Fn(Event) -> Result<(), Failure>,
	) -> Result<(), Failure> {
		let until = Instant::now() + delay;
		if let Some(guild) = guild {
			if self.cooldowns.len() < 64 || self.cooldowns.contains_key(&guild) {
				self.cooldowns
					.entry(guild)
					.and_modify(|at| *at = (*at).max(until))
					.or_insert(until);
			} else {
				self.blocked_until = Some(self.blocked_until.map_or(until, |at| at.max(until)));
			}
		} else {
			self.blocked_until = Some(self.blocked_until.map_or(until, |at| at.max(until)));
		}
		for active in &mut self.active {
			if active.as_ref().is_some_and(|(request, _)| {
				guild.is_none_or(|guild| guild == request.guild)
					&& nonce.is_none_or(|nonce| nonce == request.nonce.to_string())
			}) {
				let (request, _) = active.take().unwrap();
				emit(Event::MemberSearch {
					request,
					result: Err(Failure::RateLimited),
				})?;
			}
		}
		Ok(())
	}
	pub fn chunk(&mut self, bytes: &[u8]) -> Option<Event> {
		if bytes.len() > 512 * 1024 {
			return None;
		}
		let chunk: Chunk = discord_protocol::decode(bytes).ok()?;
		let slot = self.active.iter().position(|active| {
			active.as_ref().is_some_and(|(request, _)| {
				request.guild == chunk.guild_id
					&& chunk.nonce.as_deref() == Some(request.nonce.to_string().as_str())
			})
		})?;
		let (request, _) = self.active[slot].take()?;
		let result = (|| {
			if chunk.chunk_index != 0 || chunk.chunk_count != 1 || chunk.members.len() > LIMIT {
				return Err(Failure::Protocol);
			}
			let rows: Vec<_> = chunk
				.members
				.into_iter()
				.filter_map(|member| {
					MemberItem::Member {
						presence: model::Patch::Absent,
						member: Box::new(member),
					}
					.into_model()
				})
				.collect();
			if rows.iter().any(|m| !m.valid())
				|| rows.iter().map(model::Member::bytes).sum::<usize>() > MAX_BYTES
			{
				return Err(Failure::Capacity);
			}
			Ok(rows)
		})();
		if self.following == Some((request.guild, request.channel))
			&& let Ok(rows) = &result
		{
			self.follow_users(rows.iter().map(|member| member.user.id));
		}
		Some(Event::MemberSearch { request, result })
	}
}

#[cfg(debug_assertions)]
pub fn debug_check(request: Request) -> Event {
	let mut search = Search::default();
	search.queued[request.slot] = Some(request.clone());
	let frame = search.tick(&|_| Ok(())).unwrap().unwrap().frame;
	let packet: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
	assert_eq!(packet["op"], 8);
	if request.users.is_empty() {
		assert_eq!(packet["d"]["query"], request.query);
	} else {
		assert_eq!(
			packet["d"]["user_ids"],
			serde_json::json!(
				request
					.users
					.iter()
					.map(ToString::to_string)
					.collect::<Vec<_>>()
			)
		);
		assert!(packet["d"].get("query").is_none());
	}
	assert_eq!(packet["d"]["limit"], 100);
	let mut chunk = serde_json::json!({"guild_id":request.guild.to_string(),"nonce":"wrong","chunk_index":0,"chunk_count":1,"members":[{"user":{"id":"987654321","username":"remote-person"},"nick":"OutsideFirstHundred","roles":[]}]});
	assert!(search.chunk(&serde_json::to_vec(&chunk).unwrap()).is_none());
	chunk["nonce"] = serde_json::json!(request.nonce.to_string());
	let event = search.chunk(&serde_json::to_vec(&chunk).unwrap()).unwrap();
	assert!(
		matches!(&event, Event::MemberSearch { result: Ok(rows), .. } if rows.len() == 1 && rows[0].user.id == Id(987654321))
	);
	assert!(search.chunk(&serde_json::to_vec(&chunk).unwrap()).is_none());
	search.next_send = Instant::now();
	search.cooldowns.clear();
	search.queued[request.slot] = Some(request.clone());
	search.tick(&|_| Ok(())).unwrap();
	chunk["members"] = serde_json::Value::Array(vec![chunk["members"][0].clone(); LIMIT + 1]);
	assert!(matches!(
		search.chunk(&serde_json::to_vec(&chunk).unwrap()),
		Some(Event::MemberSearch { result: Err(_), .. })
	));
	event
}

#[cfg(test)]
mod tests {
	use super::*;
	fn request(guild: u64, nonce: u64) -> Request {
		Request {
			guild: Id(guild),
			channel: Id(guild + 100),
			query: "synthetic".into(),
			users: vec![],
			nonce,
			slot: 0,
		}
	}
	#[test]
	fn resolved_members_receive_bounded_sparse_live_subscriptions() {
		let mut search = Search::default();
		let lookup = request(1, 1);
		search.update(&[Some(lookup), None]);
		assert!(search.pending_subscription().is_none());
		search.tick(&|_| Ok(())).unwrap().unwrap();
		let event = search.chunk(br#"{"guild_id":"1","nonce":"1","chunk_index":0,"chunk_count":1,"members":[{"user":{"id":"7","username":"account"},"nick":"Nickname","roles":[]}]}"#);
		assert!(matches!(
			event,
			Some(Event::MemberSearch { result: Ok(_), .. })
		));
		let (guild, users, frame) = search.pending_subscription().unwrap();
		let packet: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
		assert_eq!(
			packet,
			serde_json::json!({"op":37,"d":{"subscriptions":{"1":{"members":["7"]}}}})
		);
		search.subscribed(guild, users);
		assert!(search.pending_subscription().is_none());
		search.follow_users((1..=1000).map(Id));
		assert_eq!(search.users.len(), 128);
		assert_eq!(search.users.first(), Some(&Id(873)));
		assert_eq!(search.users.last(), Some(&Id(1000)));
		assert!(search.pending_subscription().unwrap().2.len() < 4096);
	}
	#[test]
	fn identity_subscriptions_retire_and_resume_without_crossing_guilds() {
		let mut search = Search::default();
		let mut author = request(1, 1);
		author.slot = 1;
		author.query.clear();
		author.users = vec![Id(7)];
		search.update(&[None, Some(author.clone())]);
		let (guild, users, _) = search.pending_subscription().unwrap();
		search.subscribed(guild, users);
		search.reconnect(true);
		assert_eq!(search.pending_subscription().unwrap().1, vec![Id(7)]);
		search.subscribed(Id(1), vec![Id(7)]);
		author.guild = Id(2);
		author.channel = Id(102);
		author.nonce = 2;
		author.users = vec![Id(8)];
		search.update(&[None, Some(author)]);
		let (old, users, _) = search.pending_subscription().unwrap();
		assert_eq!(old, Id(1));
		assert!(users.is_empty());
		search.subscribed(old, users);
		let (guild, users, _) = search.pending_subscription().unwrap();
		assert_eq!((guild, users.clone()), (Id(2), vec![Id(8)]));
		search.subscribed(guild, users);
		search.canceled_subscription(Id(2));
		assert_eq!(search.pending_subscription().unwrap().1, vec![Id(8)]);
		search.reconnect(false);
		assert!(search.pending_subscription().is_none());
	}
	#[test]
	fn query_purpose_follows_its_slot_when_both_requests_encode_user_ids() {
		let mut search = Search::default();
		let now = Instant::now();
		search.next_send = now;
		let mut numeric = request(1, 1);
		numeric.query = "123456789".into();
		let mut authors = request(2, 2);
		authors.slot = 1;
		authors.query.clear();
		authors.users = vec![Id(123456789)];
		search.update(&[Some(numeric), Some(authors)]);
		for (offset, purpose) in [(0, Purpose::Composer), (1, Purpose::Authors)] {
			let query = search
				.tick_at(now + Duration::from_secs(offset), &|_| Ok(()))
				.unwrap()
				.unwrap();
			assert_eq!(query.purpose, purpose);
			let packet: serde_json::Value =
				serde_json::from_str(query.frame.to_text().unwrap()).unwrap();
			assert_eq!(packet["d"]["user_ids"], serde_json::json!(["123456789"]));
		}
	}
	#[test]
	fn per_guild_cooldown_retains_latest_query_and_uses_actual_send_time() {
		let now = Instant::now();
		let mut search = Search {
			next_send: now,
			..Default::default()
		};
		search.update(&[Some(request(1, 1)), None]);
		assert!(search.tick_at(now, &|_| Ok(())).unwrap().is_some());
		assert!(search.active[0].as_ref().unwrap().1.is_none());
		search.sent(Id(1), 1, now + Duration::from_secs(5));
		search.update(&[Some(request(1, 2)), None]);
		assert!(
			search
				.tick_at(now + Duration::from_secs(30), &|_| Ok(()))
				.unwrap()
				.is_none()
		);
		assert_eq!(search.queued[0].as_ref().unwrap().nonce, 2);
		assert!(
			search
				.tick_at(now + Duration::from_secs(35), &|_| Ok(()))
				.unwrap()
				.is_some()
		);
		assert_eq!(search.active[0].as_ref().unwrap().0.nonce, 2);
	}
	#[test]
	fn resume_preserves_cooldown_and_fresh_identify_resets_it() {
		let mut search = Search::default();
		let now = Instant::now();
		search.next_send = now;
		search.update(&[Some(request(1, 1)), None]);
		search.tick_at(now, &|_| Ok(())).unwrap();
		search.reconnect(true);
		assert!(search.cooldowns.contains_key(&Id(1)));
		search.reconnect(false);
		assert!(search.cooldowns.is_empty());
	}
}
