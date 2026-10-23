//! Explicit review links on service security messages; no alert text classification.
use client_core::State;
use model::{
	Id, Message,
	server_admin::{MemberFilters, MemberSignal},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
	RecentJoins,
	UnusualDm,
	Quarantined,
	Pending,
}
impl Filter {
	pub fn members(self) -> MemberFilters {
		let mut filters = MemberFilters::default();
		match self {
			Self::RecentJoins => filters.joined_within_days = Some(1),
			Self::UnusualDm => filters.signal = Some(MemberSignal::UnusualDm),
			Self::Quarantined => filters.signal = Some(MemberSignal::Quarantined),
			Self::Pending => filters.signal = Some(MemberSignal::Pending),
		}
		filters
	}
}
#[derive(Clone, Copy)]
pub(super) enum Choice {
	Members(Filter),
	Member(Id),
}
#[derive(Clone, Copy)]
pub(super) struct Request {
	pub generation: u64,
	pub channel: Id,
	pub message: Id,
	pub choice: Choice,
}
fn guild(state: &State, message: &Message) -> Option<Id> {
	if !matches!(message.kind, 24 | 36..=39)
		|| message.forwarded
		|| message.ephemeral
		|| state.selected != Some(message.channel)
		|| !state.can_view(message.channel)
		|| !state.can_read_history(message.channel)
	{
		return None;
	}
	state.channel(message.channel)?.guild
}
fn mentioned(message: &Message, user: Id) -> bool {
	!crate::embeds::has_spoilers(message)
		&& message
			.mentions
			.iter()
			.any(|u| u.id == user && u.kind == model::AccountKind::Human && !u.webhook)
}
impl Request {
	pub fn resolve(&self, state: &State) -> Option<Id> {
		if self.generation != state.generation {
			return None;
		}
		let message = state
			.timeline
			.get(self.message)
			.filter(|v| v.channel == self.channel)?;
		let guild = guild(state, message)?;
		match self.choice {
			Choice::Members(_) if state.can_open_member_settings(guild) => Some(guild),
			Choice::Member(user) if state.can_open_mod_view(guild) && mentioned(message, user) => {
				Some(guild)
			}
			_ => None,
		}
	}
}
pub(super) fn show(
	ui: &mut egui::Ui,
	state: &State,
	message: &Message,
	pending: &mut Option<Request>,
) {
	let Some(guild) = guild(state, message) else {
		return;
	};
	let search = state.can_open_member_settings(guild);
	let member = state.can_open_mod_view(guild)
		&& !crate::embeds::has_spoilers(message)
		&& message
			.mentions
			.iter()
			.any(|u| u.kind == model::AccountKind::Human && !u.webhook);
	if !search && !member {
		return;
	}
	ui.menu_button("Review members", |ui| {
		ui.set_max_width(310.0);
		ui.weak("Choose a review filter. This alert does not identify which members match.");
		let mut choice = None;
		if search {
			for (label, filter) in [
				("Joined in the last 24 hours", Filter::RecentJoins),
				("Unusual DM activity", Filter::UnusualDm),
				("Quarantined names", Filter::Quarantined),
				("Pending membership", Filter::Pending),
			] {
				if ui.button(label).clicked() {
					choice = Some(Choice::Members(filter));
				}
			}
		}
		if member {
			if search {
				ui.separator();
			}
			for user in message
				.mentions
				.iter()
				.filter(|u| u.kind == model::AccountKind::Human && !u.webhook)
				.take(8)
			{
				if ui
					.button(format!("Review mentioned member: {}", user.name))
					.clicked()
				{
					choice = Some(Choice::Member(user.id));
				}
			}
		}
		if let Some(choice) = choice {
			*pending = Some(Request {
				generation: state.generation,
				channel: message.channel,
				message: message.id,
				choice,
			});
			ui.close();
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	fn fixture() -> (State, Message) {
		let mut state = test_support::demo_state();
		let mut permissions = test_support::permission_snapshot(&state);
		for guild in &mut permissions.guilds {
			guild.owner = state.user.as_ref().map(|u| u.id);
		}
		state.permissions.replace(permissions).unwrap();
		let channel = state.selected.unwrap();
		let mut message = test_support::message(900, channel);
		message.kind = 38;
		message.mentions = vec![message.author.clone()];
		state
			.timeline
			.insert(message.clone(), false, false)
			.unwrap();
		(state, message)
	}
	#[test]
	fn supplied_alert_type_routes_to_explicit_filters_without_classifying_text() {
		let (mut state, message) = fixture();
		let request = Request {
			generation: state.generation,
			channel: message.channel,
			message: message.id,
			choice: Choice::Members(Filter::UnusualDm),
		};
		assert!(request.resolve(&state).is_some());
		let mut ordinary = message.clone();
		ordinary.kind = 0;
		ordinary.content = "raid alert unusual DMs".into();
		state.timeline.insert(ordinary, false, false).unwrap();
		assert!(request.resolve(&state).is_none());
		assert_eq!(Filter::RecentJoins.members().joined_within_days, Some(1));
		assert_eq!(
			Filter::UnusualDm.members().signal,
			Some(MemberSignal::UnusualDm)
		);
	}
	#[test]
	fn stale_or_removed_source_and_unsupplied_members_cannot_open_review() {
		let (mut state, message) = fixture();
		let request = Request {
			generation: state.generation,
			channel: message.channel,
			message: message.id,
			choice: Choice::Member(message.author.id),
		};
		assert!(request.resolve(&state).is_some());
		assert!(
			Request {
				choice: Choice::Member(Id(u64::MAX)),
				..request
			}
			.resolve(&state)
			.is_none()
		);
		state.generation = state.generation.wrapping_add(1);
		assert!(request.resolve(&state).is_none());
		state.generation = request.generation;
		state.selected = None;
		assert!(request.resolve(&state).is_none());
		state.selected = Some(message.channel);
		state
			.permissions
			.replace(model::permissions::Snapshot::default())
			.unwrap();
		assert!(request.resolve(&state).is_none());
		let (mut state, _) = fixture();
		state.timeline.delete(message.id).unwrap();
		assert!(request.resolve(&state).is_none());
	}
}
