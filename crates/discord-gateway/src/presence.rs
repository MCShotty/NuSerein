use crate::{Event, Failure, Instant};
use client_core::{MAX_MEMBER_PRESENCE_BYTES, presence::Update};
use discord_protocol::presence::PresenceUpdate;
use model::Id;
use std::{collections::BTreeMap, time::Duration};

fn empty_update(user: Id) -> Update {
	Update {
		user,
		status: model::Patch::Absent,
		custom_status: model::Patch::Absent,
		activities: model::Patch::Absent,
		clients: model::Patch::Absent,
	}
}

#[derive(Default)]
pub(crate) struct Pending {
	updates: BTreeMap<Id, Update>,
	bytes: usize,
	pub bootstrap_users: std::collections::BTreeSet<Id>,
	pub deadline: Option<Instant>,
}
impl Pending {
	pub fn push(
		&mut self,
		update: PresenceUpdate,
		now: Instant,
		emit: impl FnOnce(Event) -> Result<(), Failure>,
	) -> Result<(), Failure> {
		// Guild-scoped presence is not authoritative for a user's global/DM presence.
		if update.guild.is_some() {
			return Ok(());
		}
		let update = Update {
			user: update.user,
			status: update.status,
			custom_status: update.custom_status,
			activities: update.activities,
			clients: update.clients,
		};
		let mut merged = self
			.updates
			.get(&update.user)
			.cloned()
			.unwrap_or_else(|| empty_update(update.user));
		merged.merge(update.clone());
		let mut bytes = self.bytes - self.updates.get(&merged.user).map_or(0, Update::heap_bytes)
			+ merged.heap_bytes();
		if (self.updates.len() >= 100 && !self.updates.contains_key(&merged.user))
			|| bytes + 100 * size_of::<Update>() + size_of::<Event>() > MAX_MEMBER_PRESENCE_BYTES
		{
			if let Some(event) = self.take() {
				emit(event)?;
			}
			merged = empty_update(update.user);
			merged.merge(update);
			bytes = merged.heap_bytes();
		}
		if bytes + 100 * size_of::<Update>() + size_of::<Event>() > MAX_MEMBER_PRESENCE_BYTES {
			return Ok(());
		}
		self.bytes = bytes;
		self.updates.insert(merged.user, merged);
		self.deadline
			.get_or_insert(now + Duration::from_millis(100));
		Ok(())
	}
	pub fn take(&mut self) -> Option<Event> {
		self.deadline = None;
		self.bytes = 0;
		let updates = std::mem::take(&mut self.updates);
		(!updates.is_empty()).then(|| Event::DirectPresence(updates.into_values().collect()))
	}
	#[cfg(test)]
	pub fn supplemental(
		&mut self,
		bytes: &[u8],
		now: Instant,
		emit: &impl Fn(Event) -> Result<(), client_core::auth::Failure>,
	) -> Result<(), client_core::auth::Failure> {
		// Identify sends no DEDUPE_USER_OBJECTS capability, so READY uses presences.
		// Also accept merged_presences.friends from the unofficial supplemental format.
		// Decode only bounded wire data; no assets/secrets survive the presence decoder.
		#[derive(serde::Deserialize)]
		struct Snapshot<'a> {
			#[serde(default, borrow)]
			presences: Option<&'a serde_json::value::RawValue>,
			#[serde(default, borrow)]
			merged_presences: Option<Merged<'a>>,
		}
		#[derive(serde::Deserialize)]
		struct Merged<'a> {
			#[serde(default, borrow)]
			friends: Option<&'a serde_json::value::RawValue>,
		}
		if bytes.len() > discord_protocol::MAX_GATEWAY_WIRE {
			return Ok(());
		}
		let Ok(data) = serde_json::from_slice::<Snapshot<'_>>(bytes) else {
			return Ok(());
		};
		let Some(friends) = data
			.merged_presences
			.and_then(|m| m.friends)
			.or(data.presences)
		else {
			return Ok(());
		};
		self.friends(friends, now, emit)
	}
	pub fn friends(
		&mut self,
		friends: &serde_json::value::RawValue,
		now: Instant,
		emit: &impl Fn(Event) -> Result<(), client_core::auth::Failure>,
	) -> Result<(), client_core::auth::Failure> {
		let Ok(friends) =
			serde_json::from_str::<discord_protocol::presence::Friends<'_>>(friends.get())
		else {
			return Ok(());
		};
		for friend in friends.0 {
			if let Ok(update) = discord_protocol::presence::decode(friend.get().as_bytes()) {
				if !self.bootstrap_users.contains(&update.user) {
					continue;
				}
				if (self.updates.len() >= 100 || self.bytes > 96 * 1024)
					&& let Some(event) = self.take()
				{
					emit(event)?;
				}
				self.push(update, now, emit)?;
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::Patch;

	#[test]
	fn direct_presence_burst_delivers_every_user() {
		let now = Instant::now();
		let mut pending = Pending::default();
		let received = std::cell::RefCell::new(Vec::new());
		let emit = |event: Event| {
			assert!(event.bytes() <= MAX_MEMBER_PRESENCE_BYTES);
			let Event::DirectPresence(updates) = event else {
				panic!("expected direct presence")
			};
			assert!(updates.len() <= 100);
			received
				.borrow_mut()
				.extend(updates.into_iter().map(|u| u.user));
			Ok(())
		};
		for user in 1..=150 {
			let wire = format!(r#"{{"user":{{"id":"{user}"}},"status":"online"}}"#);
			pending
				.push(
					discord_protocol::presence::decode(wire.as_bytes()).unwrap(),
					now,
					emit,
				)
				.unwrap();
		}
		emit(pending.take().unwrap()).unwrap();
		assert_eq!(*received.borrow(), (1..=150).map(Id).collect::<Vec<_>>());
	}

	#[test]
	fn direct_presence_flushes_at_the_byte_limit_and_propagates_delivery_failure() {
		let now = Instant::now();
		let mut pending = Pending::default();
		let received = std::cell::RefCell::new(Vec::new());
		let emit = |event: Event| {
			assert!(event.bytes() <= MAX_MEMBER_PRESENCE_BYTES);
			let Event::DirectPresence(updates) = event else {
				panic!()
			};
			received
				.borrow_mut()
				.extend(updates.into_iter().map(|u| u.user));
			Ok(())
		};
		let text = "🚀".repeat(128);
		let activities: Vec<_> = (0..4)
			.map(|kind| serde_json::json!({"type":kind,"name":text,"details":text,"state":text}))
			.collect();
		for user in 1..=25 {
			let wire = serde_json::json!({"user":{"id":user.to_string()},"status":"online","activities":activities});
			pending
				.push(
					discord_protocol::presence::decode(&serde_json::to_vec(&wire).unwrap())
						.unwrap(),
					now,
					emit,
				)
				.unwrap();
		}
		assert!(
			!received.borrow().is_empty(),
			"the byte limit should flush before 100 users"
		);
		emit(pending.take().unwrap()).unwrap();
		assert_eq!(*received.borrow(), (1..=25).map(Id).collect::<Vec<_>>());

		for user in 1..=100 {
			let wire = format!(r#"{{"user":{{"id":"{user}"}},"status":"online"}}"#);
			pending
				.push(
					discord_protocol::presence::decode(wire.as_bytes()).unwrap(),
					now,
					emit,
				)
				.unwrap();
		}
		let next =
			discord_protocol::presence::decode(br#"{"user":{"id":"101"},"status":"online"}"#)
				.unwrap();
		assert!(matches!(
			pending.push(next, now, |_| Err(Failure::Capacity)),
			Err(Failure::Capacity)
		));
		assert!(pending.take().is_none());
	}

	#[test]
	fn partial_presence_after_a_flush_keeps_the_previous_fields() {
		let now = Instant::now();
		let mut pending = Pending::default();
		let events = std::cell::RefCell::new(Vec::new());
		let emit = |event| {
			events.borrow_mut().push(event);
			Ok(())
		};
		let first = br#"{"user":{"id":"1"},"status":"online","activities":[{"type":0,"name":"Synthetic"}]}"#;
		pending
			.push(
				discord_protocol::presence::decode(first).unwrap(),
				now,
				emit,
			)
			.unwrap();
		for user in 2..=101 {
			let wire = format!(r#"{{"user":{{"id":"{user}"}},"status":"online"}}"#);
			pending
				.push(
					discord_protocol::presence::decode(wire.as_bytes()).unwrap(),
					now,
					emit,
				)
				.unwrap();
		}
		let update = br#"{"user":{"id":"1"},"status":"idle"}"#;
		pending
			.push(
				discord_protocol::presence::decode(update).unwrap(),
				now,
				emit,
			)
			.unwrap();
		emit(pending.take().unwrap()).unwrap();
		let mut previous = None;
		for event in events.into_inner() {
			let Event::DirectPresence(updates) = event else {
				panic!()
			};
			for update in updates.into_iter().filter(|update| update.user == Id(1)) {
				previous = Some(update.resolve(previous.as_ref()));
			}
		}
		let final_presence = previous.unwrap();
		assert_eq!(final_presence.status.as_deref(), Some("idle"));
		assert_eq!(final_presence.activities[0].name, "Synthetic");
	}

	#[test]
	fn direct_patches_coalesce_without_losing_absent_fields_and_stay_bounded() {
		let now = Instant::now();
		let mut pending = Pending::default();
		pending.bootstrap_users.insert(Id(3));
		pending.supplemental(br#"{"merged_presences":{"friends":[{"user_id":"3","status":"online","activities":[{"type":0,"name":"Synthetic game"}]}]}}"#, now, &|_| Ok(())).unwrap();
		pending
			.push(
				discord_protocol::presence::decode(br#"{"user":{"id":"3"},"status":"idle"}"#)
					.unwrap(),
				now + Duration::from_millis(50),
				|_| Ok(()),
			)
			.unwrap();
		assert_eq!(pending.deadline, Some(now + Duration::from_millis(100)));
		let event = pending.take().unwrap();
		assert!(event.bytes() <= MAX_MEMBER_PRESENCE_BYTES);
		let Event::DirectPresence(updates) = event else {
			panic!()
		};
		assert_eq!(updates.len(), 1);
		assert_eq!(updates[0].status, Patch::Value("idle".into()));
		assert!(
			matches!(&updates[0].activities, Patch::Value(a) if a.len()==1 && a[0].name=="Synthetic game")
		);
		assert!(pending.take().is_none());
		let received = std::cell::RefCell::new(0);
		let emit = |event: Event| {
			assert!(event.bytes() <= MAX_MEMBER_PRESENCE_BYTES);
			let Event::DirectPresence(updates) = event else {
				panic!()
			};
			assert!(updates.len() <= 100);
			*received.borrow_mut() += updates.len();
			Ok(())
		};
		for id in 1..1000 {
			let bytes = format!(
				r#"{{"user":{{"id":"{id}"}},"activities":[{{"type":0,"name":"Synthetic"}}]}}"#
			);
			pending
				.push(
					discord_protocol::presence::decode(bytes.as_bytes()).unwrap(),
					now,
					emit,
				)
				.unwrap();
		}
		emit(pending.take().unwrap()).unwrap();
		assert_eq!(*received.borrow(), 999);
		for wire in [
            br#"{"user":{"id":"3"},"status":"online","activities":[{"type":0,"name":"Old activity"}]}"#.as_slice(),
            br#"{"user":{"id":"3"},"status":"offline"}"#,
            br#"{"user":{"id":"3"},"status":"online"}"#,
        ] { pending.push(discord_protocol::presence::decode(wire).unwrap(), now, |_| Ok(())).unwrap(); }
		let Event::DirectPresence(updates) = pending.take().unwrap() else {
			panic!()
		};
		assert_eq!(updates[0].activities, Patch::Null);
		assert_eq!(updates[0].custom_status, Patch::Null);
	}
	#[test]
	fn supplemental_filters_unknown_users_and_streams_more_than_one_batch() {
		let now = Instant::now();
		let mut pending = Pending::default();
		pending.bootstrap_users.extend((100..=255).map(Id));
		let friends: Vec<_> = (1..=300).map(|id| serde_json::json!({"user_id":id.to_string(),"status":"online","activities":[{"type":0,"name":"Synthetic"}]})).collect();
		let bytes =
			serde_json::to_vec(&serde_json::json!({"merged_presences":{"friends":friends}}))
				.unwrap();
		let received = std::cell::RefCell::new(Vec::new());
		pending
			.supplemental(&bytes, now, &|event| {
				assert!(event.bytes() <= MAX_MEMBER_PRESENCE_BYTES);
				let Event::DirectPresence(updates) = event else {
					panic!()
				};
				received
					.borrow_mut()
					.extend(updates.into_iter().map(|u| u.user));
				Ok(())
			})
			.unwrap();
		let Event::DirectPresence(updates) = pending.take().unwrap() else {
			panic!()
		};
		received
			.borrow_mut()
			.extend(updates.into_iter().map(|u| u.user));
		assert_eq!(*received.borrow(), (100..=255).map(Id).collect::<Vec<_>>());
	}
}
