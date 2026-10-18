//! Synthetic scheduled-event and Stage states; never a network or microphone adapter.
use client_core::{
	State,
	voice::{Call, Participant, Phase, RosterEntry},
};
use model::{Id, community::*};
pub fn community_state(stage: bool) -> State {
	let mut state = crate::demo_state();
	let guild = Id(10);
	let mut channel = state
		.channels
		.iter()
		.find(|c| c.guild == Some(guild) && c.kind == 2)
		.unwrap()
		.clone();
	channel.id = Id(90);
	channel.kind = 13;
	channel.name = "Community Stage".into();
	state.channels.push(channel);
	state.invalidate_navigation();
	let mut permissions = crate::permission_snapshot(&state);
	for g in &mut permissions.guilds {
		g.owner = Some(Id(1));
	}
	state.permissions.replace(permissions).unwrap();
	let start = 1_800_000_000;
	let events=vec![
		ScheduledEvent {id:Id(91),guild,creator:Some(Id(1)),status:1,draft:EventDraft{name:"Community meetup".into(),description:"Bring your ideas for the next community project. This is synthetic offline content.".into(),entity_type:3,channel:None,location:"The community garden".into(),start,end:Some(start+3600),recurrence:None},exceptions:vec![],user_count:Some(18)},
		ScheduledEvent {id:Id(92),guild,creator:Some(Id(1)),status:1,draft:EventDraft{name:"Weekly community conversations".into(),description:"A recurring conversation with speakers and an audience. Join to listen, or request to speak.".into(),entity_type:1,channel:Some(Id(90)),location:String::new(),start:start+86400,end:Some(start+90000),recurrence:Some(Recurrence {start:start+86400,end:None,frequency:2,interval:1,by_weekday:vec![5],by_n_weekday:vec![],by_month:vec![],by_month_day:vec![],by_year_day:vec![],count:None})},exceptions:vec![],user_count:Some(42)},
	];
	state.community.events = Some(EventPage {
		guild,
		events,
		subscriptions: Some(vec![]),
		subscriptions_complete: true,
	});
	state.community.opened = Some(guild);
	state.community.stale = false;
	state.community.stage_stale = false;
	state.community.stage = Some(StageInstance {
		id: Id(93),
		guild,
		channel: Id(90),
		topic: "Building our next community project together".into(),
		event: None,
	});
	state.community.stage_channel = Some(Id(90));
	if stage {
		state.voice.roster = (1..=4)
			.map(|id| RosterEntry {
				guild,
				channel: Id(90),
				participant: Participant {
					user: Id(id),
					muted: id != 2,
					deafened: false,
					server_muted: false,
					server_deafened: false,
					stage: if id == 2 {
						StageParticipation::Speaker
					} else {
						StageParticipation::Audience { requested: id == 3 }
					},
					video: false,
					streaming: false,
				},
				member: None,
			})
			.collect();
		state.voice.active = Some(Call {
			channel: Id(90),
			guild: Some(guild),
			request: 1,
			phase: Phase::Connected,
			connected_at: None,
			channel_started_at: None,
			stage_revision: 0,
			media_mode: client_core::voice::MediaMode::Stage,
			stage_consent: false,
			stage: StageParticipation::Audience { requested: false },
			server_muted: false,
			server_deafened: false,
			muted: true,
			deafened: false,
			participants: state.voice.roster.iter().map(|r| r.participant).collect(),
			camera: false,
			watching: None,
			error: None,
			reconnecting: None,
		});
		let _ = state.select(Id(90));
	}
	state
}
