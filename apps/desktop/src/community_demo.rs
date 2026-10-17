//! The community preview changes only synthetic session state.
use client_core::{Event, State};
use model::{Id, community::*};
pub fn execute(state: &mut State, guild: Id, request: u64, action: Action) -> Event {
	let outcome = match action {
		Action::ResetOccurrenceRsvp { event, occurrence } => {
			Outcome::OccurrenceRsvpReset { event, occurrence }
		}
		Action::EditOccurrence {
			before,
			original,
			canceled,
			start,
			end,
		} => Outcome::Exception {
			event: before.id,
			exception: EventException {
				event: before.id,
				id: occurrence_id(original).unwrap(),
				canceled,
				start,
				end,
			},
		},
		Action::RestoreOccurrence { before, exception } => Outcome::ExceptionRestored {
			event: before.id,
			exception,
		},
		Action::RsvpOccurrence {
			event,
			occurrence,
			interested,
		} => Outcome::Rsvp {
			event,
			occurrence: Some(occurrence),
			interested,
		},
		Action::ListEvents => Outcome::Events(
			state
				.community
				.events
				.clone()
				.filter(|p| p.guild == guild)
				.unwrap_or(EventPage {
					guild,
					events: vec![],
					subscriptions: Some(vec![]),
					subscriptions_complete: true,
				}),
		),
		Action::GetEvent(id) => match state
			.community
			.events
			.as_ref()
			.and_then(|p| p.events.iter().find(|e| e.id == id))
			.cloned()
		{
			Some(e) => Outcome::Event(e),
			None => return failure(guild, request),
		},
		Action::Create(draft) => Outcome::Event(ScheduledEvent {
			id: Id(1_000_000 + request),
			guild,
			creator: state.user.as_ref().map(|u| u.id),
			status: 1,
			draft,
			exceptions: vec![],
			user_count: Some(1),
		}),
		Action::Edit { mut before, after } => {
			before.draft = after;
			Outcome::Event(before)
		}
		Action::Status { event, status } => {
			let Some(mut e) = state
				.community
				.events
				.as_ref()
				.and_then(|p| p.events.iter().find(|e| e.id == event))
				.cloned()
			else {
				return failure(guild, request);
			};
			e.status = status;
			Outcome::Event(e)
		}
		Action::Delete(id) => Outcome::Deleted(id),
		Action::Rsvp { event, interested } => Outcome::Rsvp {
			event,
			occurrence: None,
			interested,
		},
		Action::GetStage {
			channel,
			call_request,
		} => Outcome::Stage {
			channel,
			instance: state
				.community
				.stage
				.clone()
				.filter(|s| s.channel == channel),
			voice: call_request
				.and_then(|r| {
					state
						.voice
						.active
						.as_ref()
						.filter(|c| c.channel == channel && c.request == r)
				})
				.map(|c| StageVoiceSnapshot {
					user: state.user.as_ref().unwrap().id,
					channel,
					participation: c.stage,
				}),
		},
		Action::CreateStage {
			channel,
			topic,
			event,
		} => Outcome::Stage {
			channel,
			instance: Some(StageInstance {
				id: Id(2_000_000 + request),
				guild,
				channel,
				topic,
				event,
			}),
			voice: None,
		},
		Action::EditStage { channel, topic } => {
			let Some(mut stage) = state.community.stage.clone() else {
				return failure(guild, request);
			};
			stage.topic = topic;
			Outcome::Stage {
				channel,
				instance: Some(stage),
				voice: None,
			}
		}
		Action::EndStage(channel) => Outcome::Stage {
			channel,
			instance: None,
			voice: None,
		},
		Action::StageVoice {
			channel, change, ..
		} => {
			if let Some(call) = &mut state.voice.active {
				call.stage = match change {
					StageVoiceChange::RequestToSpeak(requested) => {
						StageParticipation::Audience { requested }
					}
					StageVoiceChange::AcceptInvitation | StageVoiceChange::BecomeSpeaker => {
						StageParticipation::Speaker
					}
					StageVoiceChange::Audience => StageParticipation::Audience { requested: false },
					_ => call.stage,
				};
				call.stage_revision = call.stage_revision.wrapping_add(1);
			}
			Outcome::StageVoice { channel }
		}
	};
	Event::Community(client_core::community::Event {
		guild,
		request,
		result: Ok(outcome),
	})
}
fn failure(guild: Id, request: u64) -> Event {
	Event::Community(client_core::community::Event {
		guild,
		request,
		result: Err(client_core::auth::Failure::Forbidden),
	})
}
