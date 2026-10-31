//! Start the account session and coordinate its existing independent workers.
use super::{
	AbortTask, AdminReads, Arc, AtomicU64, BTreeMap, COMMAND_SLOTS, Command, Connection,
	DiscordApi, Envelope, Event, Failure, Handle, InboxTasks, Instant, Mutex, Ordering,
	PresenceSync, ReadKey, ReadScopes, RecipientCalls, SessionSecret, TypingGate, VoiceLifecycle,
	abandonment_blocks_join, admin_mutation_busy, admin_read_failure, apply_voice_lifecycle, egui,
	emit_event_at, join_taken_over, mfa_job_available, mpsc, private_call, queue_abandonment,
	queue_confirmation, queue_join, recipient_action, recipient_ids, recipient_write_allowed,
	reject_prefetch_at_capacity, release_taken_over, reliable_events, report_confirmation_failure,
	resolve_account_presence, ring_action, run_activity_sharing, run_presence_sync,
	scope_history_failure, scoped_admin_read, scoped_soundboard_event,
	stream_preview_session_failure, wait_for_recipient_invalidation, wait_for_takeover,
	wait_for_voice_interruption, watch,
};

type StreamScope = (model::Id, u64, u64);

/// History and a text search may read concurrently in the same conversation.
/// Pins and archives, or reads belonging to a different conversation, are retired.
fn retain_search_for_history(
	search: &mut Option<AbortTask>,
	search_channel: &mut Option<model::Id>,
	history_channel: model::Id,
) {
	if *search_channel != Some(history_channel) {
		drop(search.take());
		*search_channel = None;
	}
}

/// A full write lane rejects only the unsent operation and preserves the session.
fn queue_write(
	sender: &mpsc::Sender<Command>,
	command: Command,
	emit: &impl Fn(Event) -> Result<(), Failure>,
) -> Result<(), Failure> {
	match sender.try_send(command) {
		Ok(()) => Ok(()),
		Err(mpsc::error::TrySendError::Full(command)) => {
			emit(Event::CommandRejected(Box::new(command)))
		}
		Err(mpsc::error::TrySendError::Closed(_)) => Err(Failure::Network),
	}
}

/// One share owns at most one remote deletion. Queue pressure must not drop it,
/// and repeated Stop clicks must not publish another delete while its ACK waits.
#[derive(Default)]
struct StreamStops {
	active: Option<StreamScope>,
	interruption: u64,
	requested: bool,
	pending: bool,
}

impl StreamStops {
	fn started(&mut self, scope: StreamScope, interruption: u64) {
		*self = Self {
			active: Some(scope),
			interruption,
			..Self::default()
		};
	}

	fn reconcile(
		&mut self,
		owner: Option<(model::Id, u64)>,
		online: bool,
		interruption: u64,
		taken_over: Option<(model::Id, u64)>,
	) {
		if let Some((channel, request, _)) = self.active {
			if owner != Some((channel, request)) || taken_over == Some((channel, request)) {
				*self = Self::default();
				return;
			}
			// RESUME preserves the same service call/stream but drains queued
			// controls. Retain its exact deletion once for the resumed socket.
			if self.requested && (!online || self.interruption != interruption) {
				self.pending = true;
			}
			self.interruption = interruption;
		}
	}

	fn acknowledge(&mut self, scope: Option<StreamScope>) {
		if scope.is_some() && self.active == scope {
			*self = Self::default();
		}
	}

	fn request(
		&mut self,
		sender: &mpsc::Sender<client_core::voice::Command>,
		scope: StreamScope,
		online: bool,
	) -> Result<bool, &'static str> {
		if self.active != Some(scope) {
			return Ok(false);
		}
		if self.requested {
			return Ok(true);
		}
		self.requested = true;
		if !online {
			self.pending = true;
			return Ok(true);
		}
		let control = Self::command(scope);
		match sender.try_send(control) {
			Ok(()) => {}
			Err(mpsc::error::TrySendError::Full(_)) => self.pending = true,
			Err(mpsc::error::TrySendError::Closed(_)) => {
				return Err("Screen-share stop could not be sent; voice signaling is disconnected");
			}
		}
		Ok(true)
	}

	fn take_pending(&mut self) -> Option<client_core::voice::Command> {
		if !std::mem::take(&mut self.pending) {
			return None;
		}
		self.active.map(Self::command)
	}

	fn command((channel, request, stream_request): StreamScope) -> client_core::voice::Command {
		client_core::voice::Command::StopStream {
			channel,
			request,
			stream_request,
		}
	}
}

/// A queued Leave supersedes a share deletion: the Gateway's ordered Leave
/// clears its stream before a new Join can establish another owner.
fn queue_leave(
	sender: &mpsc::Sender<client_core::voice::Command>,
	pending: &mut Option<client_core::voice::Command>,
	control: client_core::voice::Command,
	owner: Option<(model::Id, u64)>,
) -> Result<(), Failure> {
	debug_assert!(matches!(control, client_core::voice::Command::Leave { .. }));
	if !matches!(control, client_core::voice::Command::Leave { channel, request } if owner == Some((channel, request)))
	{
		return Ok(());
	}
	match sender.try_send(control) {
		Ok(()) => Ok(()),
		Err(mpsc::error::TrySendError::Full(control)) => {
			if pending.is_none() {
				*pending = Some(control);
			}
			Ok(())
		}
		Err(mpsc::error::TrySendError::Closed(_)) => Err(Failure::Network),
	}
}

fn release_ready(pending: Option<client_core::voice::Command>, online: bool) -> bool {
	pending.is_some()
		&& (online || !matches!(pending, Some(client_core::voice::Command::Leave { .. })))
}

fn dispatch_release(
	permit: mpsc::Permit<'_, client_core::voice::Command>,
	pending: &mut Option<client_core::voice::Command>,
	online: bool,
) {
	// A disconnect may race the queue reservation. Remote Leave must survive
	// until RESUME; local AbandonSession retains its existing dispatch semantics.
	if release_ready(*pending, online) {
		permit.send(pending.take().expect("pending voice release"));
	}
}

impl Connection {
	pub fn start(
		runtime: &Handle,
		secret: Arc<SessionSecret>,
		generation: u64,
		expected_user: Option<model::Id>,
		cached_presence: BTreeMap<model::Id, model::OwnPresence>,
		api_proxy: watch::Receiver<Option<discord_api::proxy::ApiProxy>>,
		ctx: egui::Context,
	) -> Self {
		let (commands, mut receive) = mpsc::channel(COMMAND_SLOTS);
		let (sync_api_send, sync_api) = watch::channel(None);
		let (sync_roster, sync_roster_input) = discord_gateway::roster::channel();
		let (member_watches, member_watch_receive) = watch::channel(None);
		let (admin_scopes, mut admin_scope) = watch::channel(ReadScopes::default());
		let (preference_scope, mut preference_updates) = watch::channel(None);
		let (uploads, mut upload_receive) = mpsc::channel::<crate::uploads::UploadRequest>(1);
		let (send, events) = reliable_events(ctx.clone());
		let (typing_send, typing) = mpsc::channel(8);
		let (finished, terminal) = watch::channel(None);
		let sync_failure = finished.clone();
		let (confirmation_report, confirmation_failure) = watch::channel(None);
		let (share_activity, share_receive) = watch::channel(false);
		let (custom_rich_presence, custom_receive) = watch::channel(None);
		let (own_presence, presence_receive) = watch::channel(model::OwnPresence::default());
		let (presence_edits, presence_edit_events) = watch::channel(None);
		let (account_presence_send, account_presence) = watch::channel(None);
		let (presence_error_send, presence_error) = watch::channel(None);
		let presence_send = own_presence.clone();
		let (game_report, game_activity) = watch::channel(Ok(None));
		let (registered_games, registered_receive) = watch::channel(Vec::new());
		let (running_send, running_game) = watch::channel(None);
		let (spotify_send, spotify_activity) = watch::channel(None);
		let spotify_receive = spotify_activity.clone();
		let (invite_send, rpc_invite) = watch::channel(None);
		let (activity_observed, activity_observation) =
			watch::channel(discord_gateway::ActivityObservation::Unconfirmed);
		let (sharing_report, activity_sharing) = watch::channel(Ok(None));
		let (activity_sharing_request, sharing_requests) = mpsc::channel(1);
		let wake = ctx.clone();
		let typing_channel = Arc::new(AtomicU64::new(0));
		let active_typing = typing_channel.clone();
		let typing_gate = Mutex::new(TypingGate::default());
		let reconnect = Arc::new(tokio::sync::Notify::new());
		let gateway_reconnect = reconnect.clone();
		let diagnostics = discord_gateway::ConnectionDiagnostics::default();
		let diagnostic_revision = diagnostics.revision();
		let diagnostics = Arc::new(Mutex::new(diagnostics));
		let gateway_diagnostics = diagnostics.clone();
		let status_changed = Arc::new(tokio::sync::Notify::new());
		let status_refresh = status_changed.clone();
		let (community_fence, community_scope) = watch::channel(0u64);
		let task=runtime.spawn(async move {
            let gateway_delivery=send.clone();
            let delivery_gate=send.delivery();
            let emit_scoped=Arc::new(move |event:Event,epoch:Option<u64>| -> Result<(),Failure> {
                if event.changes_access() || matches!(&event,Event::Disconnected|Event::Resumed|Event::Voice(client_core::voice::Event::TakenOver{..})) {
                    community_fence.send_modify(|r|*r=r.wrapping_add(1));
                    emit_event_at(&send,&typing_send,Envelope{generation,event:Event::CommunityFence(*community_fence.borrow())},&ctx,epoch)?;
                }
                if let Event::AccountSettings { status: true, .. } = &event { status_changed.notify_one(); }
                if let Event::Typing(signal) = &event {
                    let active = active_typing.load(Ordering::Relaxed);
                    if !typing_gate.lock().is_ok_and(|mut gate| gate.accept(signal, active, Instant::now())) { return Ok(()); }
                }
                emit_event_at(&send, &typing_send, Envelope {generation,event}, &ctx,epoch)
            });
            let gateway_emit_scoped=emit_scoped.clone();
            // HTTP completion loss triggers a fresh snapshot, never a repeated mutation.
            let emit=move |event| {let result=emit_scoped(event,None);if result==Err(Failure::Busy){Ok(())}else{result}};
            let result=async {
                let api=DiscordApi::with_proxy(secret.clone(), api_proxy)?;
                let (user,gateway)=api.bootstrap().await?;
                if expected_user.is_some_and(|id|id!=user.id){return Err(Failure::InvalidCredential);}
                let api=Arc::new(api);
				let _ = sync_api_send.send_replace(Some(api.clone()));
				let cached = cached_presence.get(&user.id).cloned().filter(|presence| presence.valid());
				let account_presence_send = Arc::new(account_presence_send);
				let _presence_edits = AbortTask(tokio::spawn(run_presence_sync(PresenceSync {
					api: api.clone(),
					edits: presence_edit_events,
					remote_changed: status_refresh,
					presence: presence_send.clone(),
					account: account_presence_send.clone(),
					note: presence_error_send,
					finished: finished.clone(),
					wake: wake.clone(),
				})));
				let chosen = resolve_account_presence(&api, &presence_send, cached).await;
				if chosen.is_some() {
					wake.request_repaint();
				}
				let _ = account_presence_send.send_replace(chosen);
                let emit=Arc::new(emit);
                let mfa_emit=emit.clone();
                api.set_mfa_notifier(move |event| mfa_emit(Event::Mfa(event)))?;
                let review_emit=emit.clone();
                api.set_member_review_notifier(move |event| review_emit(Event::MemberReviewProgress(event)))?;
                let (member_send,member_receive)=watch::channel(None);
                let (voice_send,voice_receive)=mpsc::channel(8);
				let (activity_send,activity_receive)=watch::channel(None);
				let (member_query_send, member_query_receive) = watch::channel([None, None]);
				let _sharing_task=AbortTask(tokio::spawn(run_activity_sharing(api.clone(),share_receive.clone(),sharing_requests,sharing_report,finished.clone(),wake.clone())));
				let _activity_task=AbortTask(tokio::spawn(crate::game_activity::run((share_receive,custom_receive,crate::game_activity::Registered{games:registered_receive,current:running_send}),activity_send,game_report,invite_send,wake.clone(),user.clone(),api.clone())));
				let _spotify_task=AbortTask(tokio::spawn(crate::spotify::run(api.clone(),user.id,presence_receive.clone(),spotify_send,wake.clone())));
                let dm_channels=Arc::new(Mutex::new(BTreeMap::new()));
                let (recipient_scope,mut recipient_scope_changed)=watch::channel(0u64);
                let recipient_call=Arc::new(Mutex::new(RecipientCalls::default()));
                let gateway_recipient_call=recipient_call.clone();
                let gateway_channels=dm_channels.clone();
                let (voice_online,mut voice_availability)=watch::channel(false);
                let (stream_ended,mut stream_ended_receive)=watch::channel(None);
                let (voice_lifecycle,mut lifecycle_changed)=watch::channel(VoiceLifecycle::default());
                let (takeover_send,mut takeover_receive)=watch::channel(None);
                let gateway_takeover=takeover_send.clone();
                let gateway_api=api.clone();let gateway_emit=gateway_emit_scoped;let terminal_send=finished.clone();
                let gateway_wake=wake.clone();
                let activity_wake=wake.clone();
                let gateway_delivery_gate=delivery_gate.clone();
                let mut gateway_task=AbortTask(tokio::spawn(async move {
					let error=discord_gateway::run_with_activity_recovery_diagnostics_sync(secret,gateway,member_receive,voice_receive,gateway_reconnect,(activity_receive,presence_receive,member_query_receive,spotify_receive),Some(member_watch_receive),Some(sync_roster_input),gateway_delivery_gate,gateway_diagnostics,move |observation| {
                        if activity_observed.send_if_modified(|current| { if *current == observation { false } else { *current = observation; true } }) { activity_wake.request_repaint(); }
                        Ok(())
                    },|event,epoch|{
                        if gateway_delivery.delivery_blocked() {let _=voice_online.send(false);gateway_api.interaction_session(None)?;return Err(Failure::Busy);}
                        if let Event::Interaction(client_core::interactions::Event::Session(session)) = event { return gateway_api.interaction_session(Some(session)); }
                        if matches!(&event,Event::Disconnected|Event::Resync) { gateway_api.interaction_session(None)?; }
                        if let Some((ready_user,_,channels))=event.ready_navigation() {
                            if ready_user.id!=user.id {return Err(Failure::InvalidCredential);}
                            *gateway_channels.lock().map_err(|_|Failure::Protocol)?=channels.iter().filter(|c|private_call(c)).take(client_core::MAX_NAV).map(|c|(c.id,recipient_ids(c))).collect();
                        }
                        if let Event::ChannelCreated(channel)=&event {
                            let mut channels=gateway_channels.lock().map_err(|_|Failure::Protocol)?;
                            channels.remove(&channel.id);
                            if private_call(channel) && channels.len()<client_core::MAX_NAV {channels.insert(channel.id,recipient_ids(channel));}
                        }
                        if let Event::Unavailable(channel)=&event {gateway_channels.lock().map_err(|_|Failure::Protocol)?.remove(channel);}
                        if let Event::RecipientRemoved {channel,user:removed}=&event {
                            let mut channels=gateway_channels.lock().map_err(|_|Failure::Protocol)?;
                            if *removed==user.id {channels.remove(channel);}
                            else if let Some(recipients)=channels.get_mut(channel) {recipients.retain(|id|id!=removed);}
                        }
                        if let Event::RecipientAdded {channel,user:added}=&event && let Some(recipients)=gateway_channels.lock().map_err(|_|Failure::Protocol)?.get_mut(channel) && !recipients.contains(&added.id) {
                            if recipients.len()+1<client_core::voice::MAX_PARTICIPANTS {recipients.push(added.id);} else {recipients.clear();}
                        }
                        let invalidates_recipient = {
                            let channels=gateway_channels.lock().map_err(|_|Failure::Protocol)?;
                            let mut calls=gateway_recipient_call.lock().map_err(|_|Failure::Protocol)?;
                            let (invalidates,released)=calls.observe_scoped(&event,user.id,&channels);
                            if let Some(scope)=released {let _=gateway_takeover.send_replace(Some(scope));}
                            invalidates
                        };
                        if invalidates_recipient {
                            recipient_scope.send_modify(|revision|*revision=revision.wrapping_add(1));
                        }
                        if let Event::ChannelChanged(patch)=&event && let model::Patch::Value(kind)=patch.kind && !matches!(kind,1|3) {gateway_channels.lock().map_err(|_|Failure::Protocol)?.remove(&patch.id);}

                        if event.ready_navigation().is_some() || matches!(&event,Event::Resumed) {let _=voice_online.send(true);}
                        if matches!(&event,Event::Disconnected|Event::Resync) {let _=voice_online.send(false);}
                        if let Event::Voice(client_core::voice::Event::Stream{channel,request,stream_request,event:client_core::screen::Event::Deleted{..}})=&event {let _=stream_ended.send_replace(Some((*channel,*request,*stream_request)));}
                        voice_lifecycle.send_if_modified(|lifecycle|lifecycle.observe(&event));
                        if let Event::Voice(client_core::voice::Event::TakenOver{channel,request})=&event {let _=gateway_takeover.send_replace(Some((*channel,*request)));}
                        gateway_emit(event,Some(epoch))?;
                        if gateway_delivery.delivery_blocked(){let _=voice_online.send(false);gateway_api.interaction_session(None)?;Err(Failure::Busy)}else{Ok(())}
                    }).await.err().unwrap_or(Failure::Network).protocol_at("Gateway connection: unsupported handshake or event");
                    gateway_api.stop();let _=terminal_send.send(Some(error));gateway_wake.request_repaint();
                }));
                // Keep hangup/mute controls responsive while an HTTP message write is awaiting Discord.
                let (write_send,mut write_receive)=mpsc::channel(COMMAND_SLOTS);
                let write_api=api.clone();let write_emit=emit.clone();let write_finished=finished.clone();let write_wake=wake.clone();
                let mut write_delivery=delivery_gate;
                let mut writes=AbortTask(tokio::spawn(async move {
                    while let Some(command)=write_receive.recv().await {
                        if write_delivery.wait_for(|state| !state.blocked && !state.recovering).await.is_err(){break;}
                        let event=write_api.execute_write(command,|nonce|{let _=write_emit(Event::SendThrottled{nonce:nonce.to_owned()});write_wake.request_repaint();}).await;
                        let failure=match &event {Event::MemberGuide(client_core::member_guide::Event{result:Err(f),..})=>Some(*f),Event::Interaction(client_core::interactions::Event::Submitted{result:Err(f),..})=>Some(*f),Event::MessagingPermissions{result:Err(f),..}=>Some(*f),Event::ChannelAction(client_core::channel_actions::Event::Finished{result:Err(f),..})=>Some(*f),Event::ChannelPreferences(client_core::preferences_sync::Finished{result:Err(f),..})=>Some(*f),Event::ServerAdmin(client_core::server_admin::Event{result:Err(f),..})=>Some(*f),Event::ServerSettings(client_core::server_settings::Event{result:Err(f),..})=>Some(*f),Event::Onboarding(client_core::onboarding::Event::Loaded{result:Err(f),..}|client_core::onboarding::Event::Submitted{result:Err(f),..})=>Some(*f),Event::Failure(f)=>Some(*f),Event::ProfileEdited{result:Err(f),..} if *f != Failure::Capacity =>Some(*f),Event::OwnedCosmetics(client_core::owned_cosmetics::Event{result:Err(f),..})|Event::ServerProfile(client_core::server_profile::Event{result:Err(f),..})|Event::AccountOverview{result:Err(f),..}|Event::AccountConnections{result:Err(f),..} if *f != Failure::Capacity =>Some(*f),Event::Edited{result:Err(f),..}|Event::Pinned{result:Err(f),..}=>Some(*f),Event::GuildFolders(Err(f))=>Some(*f),Event::JoinInvite{result:Err(f),..}|Event::GuildCreated{result:Err(f),..}=>Some(*f),Event::SendResult{result:Err(f),..}=>Some(*f),Event::UserAction(client_core::user_actions::Event::Written{result:Err(f),..})=>Some(*f),Event::UserAction(client_core::user_actions::Event::DmOpened{result:Err(f),..})=>Some(*f),Event::ServerAction(client_core::server_actions::Event::Written{result:Err(f),..})=>Some(*f),Event::ServerAction(client_core::server_actions::Event::InviteSent{result:Err(f),..})=>Some(*f),Event::GroupAction(client_core::group_actions::Event::Written{result:Err(f),..}|client_core::group_actions::Event::MembersLoaded{result:Err(f),..}|client_core::group_actions::Event::Created{result:Err(f),..})=>Some(*f),Event::Reactions(client_core::reactions::Event::Written{result:Err(f),..})=>Some(*f),Event::ReadState(client_core::read_state::Event::Result{result:Err(f),..})=>Some(*f),_=>None};
                        let error=write_emit(event).err().or(failure.filter(|f|f.ends_session()));
                        if let Some(error)=error {write_api.stop();let _=write_finished.send(Some(error));write_wake.request_repaint();break;}
                    }
                }));
                // Administrative writes may wait for owner verification without holding up
                // the ordinary message-send queue. One protected operation and one check.
                let mut administrative:Option<AbortTask>=None;
                let mut pending_preferences:Option<Box<client_core::preferences_sync::Request>>=None;
				let mut admin_reads = AdminReads::default();
                let mut mfa_job:Option<AbortTask>=None;
                let mut history:Option<AbortTask>=None;
                let mut reply_previews:Vec<AbortTask>=Vec::new();
                // Preloads never cancel the open conversation's reads or each other; each kind
                // keeps at most one in flight, so this list stays tiny.
                let mut prefetches:Vec<AbortTask>=Vec::new();
                let mut community_job:Option<AbortTask>=None;
                let mut community_call:Option<(model::Id,u64,model::Id,u64)>=None;
                let mut profile:Option<AbortTask>=None;
                let mut profile_note:Option<AbortTask>=None;
				let mut stream_preview:Option<AbortTask>=None;
                let mut invite:Option<AbortTask>=None;
                let mut search:Option<AbortTask>=None;
                let mut search_channel:Option<model::Id>=None;
                let mut inbox=InboxTasks::default();
                let mut gifs:Option<AbortTask>=None;
                let mut gif_favorites:Option<AbortTask>=None;
                let mut application_commands:Option<AbortTask>=None;
                let mut autocomplete:Option<AbortTask>=None;
                let mut sticker_packs:Option<AbortTask>=None;
                let mut sticker_detail:Option<AbortTask>=None;
                // Default catalog, server catalog and the one unanswered play request.
                let mut soundboard:[Option<AbortTask>;3]=[None,None,None];
                let mut reaction_read:Option<AbortTask>=None;
                let mut ringing:Option<AbortTask>=None;
                let mut recipient_ringing:Option<(client_core::voice::Command,AbortTask)>=None;
                let mut upload:Option<AbortTask>=None;
                let mut upload_cancel:Option<watch::Sender<bool>>=None;
                let mut voice_request:Option<(model::Id,u64,bool)>=None;
				let (soundboard_owner,soundboard_owner_changed)=watch::channel(None);
                let mut current_lifecycle=VoiceLifecycle::default();
                // One release may wait for queue space; later joins cannot overtake it.
                let mut pending_release=None;
                let mut stream_stops=StreamStops::default();
                loop {
					let owner=voice_request.map(|(channel,request,_)|(channel,request));
					stream_stops.acknowledge(*stream_ended_receive.borrow_and_update());
					stream_stops.reconcile(owner,*voice_availability.borrow(),lifecycle_changed.borrow().interruption,*takeover_receive.borrow());
					soundboard_owner.send_if_modified(|current| {if *current==owner {false} else {*current=owner;true}});
                    if pending_preferences.is_some() && !admin_mutation_busy(&administrative,&community_job) {
                        let request=pending_preferences.take().expect("one queued preference write");
                        let request=match super::lanes::admit_preference(request,*preference_updates.borrow()){
                            Ok(request)=>request,Err(event)=>{emit(Event::ChannelPreferences(*event))?;continue;}
                        };
                        let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();let preference_scope=preference_updates.clone();
                        administrative=Some(AbortTask(tokio::spawn(async move {
                            // A spawned task can be scheduled after the next UI/Gateway update.
                            // Recheck the unsent request at the actual worker entry, before HTTP.
                            let scope=*preference_scope.borrow();
                            let request=match super::lanes::admit_preference(request,scope){
                                Ok(request)=>request,Err(event)=>{if let Err(error)=emit(Event::ChannelPreferences(*event)){api.stop();let _=finished.send(Some(error));}wake.request_repaint();return;}
                            };
                            let event=api.execute(Command::ChannelPreferences(request)).await;
                            let failure=match &event {Event::ChannelPreferences(client_core::preferences_sync::Finished{result:Err(f),..}) if f.ends_session()=>Some(*f),_=>None};
                            if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                            wake.request_repaint();
                        })));
                    }
                    tokio::select! {
                        changed=preference_updates.changed()=>{
                            if changed.is_err(){break;}
                            let scope=*preference_updates.borrow_and_update();
                            if pending_preferences.as_ref().is_some_and(|request|scope!=Some((request.generation,request.guild,request.request))){
                                let request=pending_preferences.take().expect("retired queued preference");
                                if let Err(event)=super::lanes::admit_preference(request,scope){emit(Event::ChannelPreferences(*event))?;}
                            }
                        }
                        _=super::lanes::admin_mutation_finished(&mut administrative,&mut community_job), if pending_preferences.is_some()=>{}
						changed=admin_scope.changed()=> {
							if changed.is_err() {break;}
							admin_reads.retire(*admin_scope.borrow_and_update());
						}
                        _=&mut gateway_task.0=>{break;}
                        _=&mut writes.0=>{break;}
                        changed=recipient_scope_changed.changed()=> {
                            if changed.is_err() {break;}
                            recipient_scope_changed.borrow_and_update();
                            let channels=dm_channels.lock().map_err(|_|Failure::Protocol)?;
                            let calls=recipient_call.lock().map_err(|_|Failure::Protocol)?;
                            if recipient_ringing.as_ref().is_some_and(|(control,_)| !recipient_write_allowed(*control,&calls,&channels)) {drop(recipient_ringing.take());}
                        }
                        permit=voice_send.reserve(), if release_ready(pending_release,*voice_availability.borrow())=> {
                            if let Ok(permit)=permit {
                                dispatch_release(permit,&mut pending_release,*voice_availability.borrow());
                            } else {
                                // The Gateway task owns connection termination when its queue closes.
                                pending_release=None;
                            }
                        }
                        changed=stream_ended_receive.changed()=> {
                            if changed.is_err() {break;}
                            stream_stops.acknowledge(*stream_ended_receive.borrow_and_update());
                        }
                        permit=voice_send.reserve(), if stream_stops.pending && pending_release.is_none() && *voice_availability.borrow()=> {
                            // Watch changes can become ready in the same select as capacity.
                            // Recheck them before sending a deletion for the retired call.
                            stream_stops.acknowledge(*stream_ended_receive.borrow_and_update());
                            let online=*voice_availability.borrow();
                            stream_stops.reconcile(voice_request.map(|(channel,request,_)|(channel,request)),online,lifecycle_changed.borrow().interruption,*takeover_receive.borrow());
                            if let Ok(permit)=permit {
                                if online && let Some(control)=stream_stops.take_pending() {permit.send(control);}
                            } else {
                                stream_stops=StreamStops::default();
                            }
                        }
                        changed=takeover_receive.changed()=> {
                            if changed.is_err() {break;}
                            if release_taken_over(&mut voice_request,*takeover_receive.borrow_and_update()) {drop(ringing.take());drop(recipient_ringing.take());recipient_call.lock().map_err(|_|Failure::Protocol)?.active=None;}
                        }
                        changed=voice_availability.changed()=> {
								if changed.is_err() {break;}
								if !*voice_availability.borrow_and_update() {drop(pending_preferences.take());drop(ringing.take());drop(recipient_ringing.take());drop(profile.take());drop(stream_preview.take());drop(search.take());inbox.cancel_read();if let Some(cancel)=&upload_cancel {let _=cancel.send(true);}}
                        }
                        changed=lifecycle_changed.changed()=> {
                            if changed.is_err() {break;}
                            let latest=*lifecycle_changed.borrow_and_update();
                            let channels=dm_channels.lock().map_err(|_|Failure::Protocol)?;
                            let calls=recipient_call.lock().map_err(|_|Failure::Protocol)?;
                            if apply_voice_lifecycle(&mut current_lifecycle,latest,&mut voice_request,&calls,&channels) {drop(ringing.take());drop(recipient_ringing.take());drop(profile.take());drop(stream_preview.take());drop(search.take());inbox.cancel_read();if let Some(cancel)=&upload_cancel {let _=cancel.send(true);}}
                        }
                        request=upload_receive.recv()=>{
                            let Some(request)=request else {break;};
                            if !*voice_availability.borrow() || upload.as_ref().is_some_and(|job|!job.0.is_finished()) {
                                match request.command {
                                    Command::Interaction(request)=>emit(Event::Interaction(client_core::interactions::Event::Submitted{nonce:request.nonce,result:Err(Failure::ProtocolAt("Upload unavailable; reselect the file to retry"))}))?,
                                    Command::Send{nonce,..}=>emit(Event::SendResult{nonce,result:Err(Failure::ProtocolAt("Upload unavailable; reselect the file to retry"))})?,
                                    Command::CreatePost{parent,request,..}=>emit(Event::PostCreated{parent,request,result:Err(Failure::ProtocolAt("Upload unavailable; reselect the file to retry"))})?,
                                    _=>{}
                                }
                                request.progress.send_replace(discord_api::upload::Status::Failed("Upload unavailable; reselect the file to retry"));
                                continue;
                            }
                            upload_cancel=Some(request.cancel.clone());
                            let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                            upload=Some(AbortTask(tokio::spawn(async move {
                                let mut updates=request.progress.subscribe();
                                let operation=async {
                                    if let Some(voice)=request.voice_message {
                                        api.upload_voice_message(request.command,voice,request.progress,request.cancel.subscribe()).await
                                    } else {
                                        api.upload_messages(request.command,request.source,request.progress,request.cancel.subscribe()).await
                                    }
                                };
                                tokio::pin!(operation);
                                let mut observing=true;
                                let event=loop {
                                    tokio::select! {
                                        event=&mut operation=>break event,
                                        changed=updates.changed(), if observing=>{observing=changed.is_ok();wake.request_repaint();}
                                    }
                                };
                                let failure=match &event {Event::Interaction(client_core::interactions::Event::Submitted{result:Err(f),..}) | Event::SendResult{result:Err(f),..} if f.ends_session()=>Some(*f),_=>None};
                                let error=emit(event).err().or(failure);
                                if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                wake.request_repaint();
                            })));
                        }
                        command=receive.recv()=>{
                            // Select can admit a queued command before the changed-watch branch.
                            if release_taken_over(&mut voice_request,*takeover_receive.borrow()) {drop(ringing.take());drop(recipient_ringing.take());recipient_call.lock().map_err(|_|Failure::Protocol)?.active=None;}
                            {
                                let latest=*lifecycle_changed.borrow_and_update();
                                let channels=dm_channels.lock().map_err(|_|Failure::Protocol)?;
                                let calls=recipient_call.lock().map_err(|_|Failure::Protocol)?;
                                if apply_voice_lifecycle(&mut current_lifecycle,latest,&mut voice_request,&calls,&channels) {drop(ringing.take());drop(recipient_ringing.take());drop(profile.take());drop(stream_preview.take());drop(search.take());if let Some(cancel)=&upload_cancel {let _=cancel.send(true);}}
                            }
                            let Some(command)=command else {break;};
							if let Some(key)=ReadKey::of(&command,generation) {
								let scopes=*admin_scope.borrow_and_update();
								admin_reads.retire(scopes);
								// A queued read can be superseded before the transport sees it.
								if !scopes.contains(key) {continue;}
								if let Command::Community {fence,action,..}=&command {
									let call=match action.as_ref(){model::community::Action::GetStage {channel,call_request:Some(request)}=>Some((*channel,*request)),_=>None};
									if *community_scope.borrow()!=*fence || call.is_some_and(|call|voice_request.map(|(c,r,_)|(c,r))!=Some(call)) {
										emit(key.rejected(Failure::ProtocolAt("Community read expired; refresh to continue")))?;
										continue;
									}
								}
								if !admin_reads.replace(key) {
									emit(key.rejected(Failure::ProtocolAt("Administration reads are busy; refresh to retry")))?;
									continue;
								}
								let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();let scope=admin_scope.clone();
								admin_reads.track(key,tokio::spawn(async move {
									let Some(event)=scoped_admin_read(api.execute(command),scope,key).await else {return;};
									let failure=admin_read_failure(&event);
									if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
									wake.request_repaint();
								}));
								continue;
							}
                            if let Command::ChannelPreferences(request)=command {
                                if request.generation!=generation || pending_preferences.is_some() {
                                    emit(Event::ChannelPreferences(client_core::preferences_sync::Finished{generation:request.generation,guild:request.guild,request:request.request,result:Err(Failure::ProtocolAt("Preference save expired before admission; retry"))}))?;
                                } else { pending_preferences=Some(request); }
                                continue;
                            }
                            if let Command::Mfa(request)=command {
                                if !mfa_job_available(&mfa_job,&request,api.mfa_resume_ready(request.id)) {
                                    emit(Event::Mfa(client_core::mfa::Event::Updated{id:request.id,result:Err(Failure::ProtocolAt("A security check is already in progress"))}))?;
                                    continue;
                                }
                                drop(mfa_job.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                mfa_job=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(Command::Mfa(request)).await;
                                    let failure=match &event {Event::Mfa(client_core::mfa::Event::Updated{result:Err(f),..}) if f.ends_session()=>Some(*f),_=>None};
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            // Community writes keep their existing call/fence cancellation path below.
                            if let Some(target)=client_core::mfa::Target::from_command(&command).filter(|_|!matches!(&command,Command::Community {..})) {
                                if admin_mutation_busy(&administrative,&community_job) {
                                    emit(target.rejected(Failure::ProtocolAt("Finish the current server action first")))?;
                                    continue;
                                }
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                administrative=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {
                                        Event::ServerAdmin(client_core::server_admin::Event{result:Err(f),..})|Event::ServerSettings(client_core::server_settings::Event{result:Err(f),..})|Event::ServerAction(client_core::server_actions::Event::Written{result:Err(f),..})|Event::ChannelAction(client_core::channel_actions::Event::Finished{result:Err(f),..})|Event::Onboarding(client_core::onboarding::Event::Submitted{result:Err(f),..}) if f.ends_session()=>Some(*f),
                                        _=>None,
                                    };
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }

                            if let Command::Community {guild,request,fence,action,..} = &command {
                                let (guild,request,fence)=(*guild,*request,*fence);
                                let call=match action.as_ref(){model::community::Action::StageVoice{channel,call_request,..}|model::community::Action::GetStage{channel,call_request:Some(call_request)}=>Some((*channel,*call_request)),_=>None};
                                if call.is_some_and(|call|voice_request.map(|(c,r,_)|(c,r))!=Some(call)) || *community_scope.borrow()!=fence || admin_mutation_busy(&administrative,&community_job) {
                                    emit(Event::Community(client_core::community::Event {guild,request,result:Err(Failure::ProtocolAt("Community action expired or is already pending; refresh to continue"))}))?;continue;
                                }
                                drop(community_job.take());
                                community_call=call.map(|(c,r)|(guild,request,c,r));
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();let mut scope=community_scope.clone();
                                community_job=Some(AbortTask(tokio::spawn(async move {
                                    let event=tokio::select! {
                                        biased;
                                        _=async {loop {
                                        if *scope.borrow_and_update()!=fence {break;}
                                        if scope.changed().await.is_err(){break;}
                                    }}=>return,
                                        event=api.execute(command)=>event,
                                    };
                                    let failure=match &event {Event::Community(client_core::community::Event{result:Err(f),..}) if f.ends_session()=>Some(*f),_=>None};
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::CancelSearch) {drop(search.take());continue;}
                            if matches!(command,Command::CancelReplyPreviews) {reply_previews.clear();continue;}
                            if let Command::ReplyPreview { channel, message, request } = command {
                                reply_previews.retain(|task| !task.0.is_finished());
                                if reply_previews.len() >= 2 {
                                    emit(Event::ReplyPreview(client_core::reply_previews::ResultEvent {channel,message,request,result:Err(Failure::ProtocolAt("Preview queue is full; retry"))}))?;
                                    wake.request_repaint();
                                    continue;
                                }
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                reply_previews.push(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(Command::ReplyPreview { channel,message,request }).await;
                                    let failure=match &event {Event::ReplyPreview(result)=>result.result.as_ref().err().copied().filter(|f|f.ends_session()),_=>None};
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::CancelMentions) {inbox.cancel_read();continue;}
                            if matches!(command,Command::Mentions{..}|Command::InboxAction{..}) {
                                let write=matches!(&command,Command::InboxAction{action,..} if action.write());
                                if !inbox.can_start(write) {
                                    if let Command::InboxAction{request,..}=command {emit(Event::InboxAction{request,result:Err(Failure::ProtocolAt("An Inbox change is still pending; refresh after it finishes"))})?;}
                                    continue;
                                }
                                if !write {inbox.cancel_read();}
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                inbox.track(write,tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Mentions{result:Err(f),..}|Event::InboxAction{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                }));
                                continue;
                            }
                            if matches!(command,Command::CancelGifs) {drop(gifs.take());continue;}
                            if matches!(command,Command::GifFavorites{..}) {
                                drop(gif_favorites.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                gif_favorites=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::GifFavorites{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::CancelAutocomplete) {drop(autocomplete.take());continue;}
                            if matches!(command,Command::Autocomplete(_)) {
                                drop(autocomplete.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                autocomplete=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Autocomplete(client_core::application_commands::CompletionEvent::Submitted{result:Err(f),..}) if f.ends_session()=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::ApplicationCommands{..}) {
                                drop(application_commands.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                application_commands=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::ApplicationCommands{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::StickerPacks|Command::Sticker(_)) {
                                let task=if matches!(command,Command::StickerPacks) {&mut sticker_packs} else {&mut sticker_detail};
                                drop(task.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                *task=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::StickerPacks(Err(f))|Event::Sticker{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if let Command::Soundboard(request)=&command {
                                use client_core::soundboard::{Event as E,Request as R};
								let scope=match request {R::Send {request,call,channel,sound,..}=>Some((*request,*channel,*sound,*call)),_=>None};
								if let Some((request,channel,sound,call))=scope && *soundboard_owner.borrow()!=Some((channel,call)) {
									emit(Event::Soundboard(E::Sent {request,channel,sound,result:Err(Failure::ProtocolAt("Call changed; the sound was not sent"))}))?;
									continue;
								}
                                let task=&mut soundboard[match request {R::Default{..}=>0,R::Guild{..}=>1,R::Send{..}=>2}];
                                drop(task.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
								let owner=soundboard_owner_changed.clone();let lifecycle=lifecycle_changed.clone();let revision=current_lifecycle.interruption;
                                *task=Some(AbortTask(tokio::spawn(async move {
									let event=if let Some((request,channel,sound,call))=scope {
										scoped_soundboard_event(api.execute(command),owner,(channel,call),lifecycle,revision).await.unwrap_or(Event::Soundboard(E::Sent {request,channel,sound,result:Err(Failure::Ambiguous)}))
									} else {api.execute(command).await};
                                    let failure=match &event {Event::Soundboard(E::Default{result:Err(f),..}|E::Guild{result:Err(f),..}|E::Sent{result:Err(f),..}) if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::Gifs{..}) {
                                drop(gifs.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                gifs=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Gifs{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::Search{..}|Command::Pins{..}|Command::Archives{..}) {
                                drop(search.take());
                                search_channel=match &command {Command::Search{channel,..}=>Some(*channel),_=>None};
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                search=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Search{result:Err(f),..}|Event::Archives{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::Reactions(client_core::reactions::Command::Read{..})) {
                                drop(reaction_read.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                reaction_read=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Reactions(client_core::reactions::Event::Read{result:Err(f),..}) if f.ends_session()=>Some(*f),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if let Command::Voice(control @ client_core::voice::Command::ChannelInfo { guild }) = &command {
                                if !*voice_availability.borrow() || voice_send.try_send(*control).is_err() {
                                    emit(Event::Voice(client_core::voice::Event::ChannelInfoUnavailable { guild: *guild }))?;
                                    wake.request_repaint();
                                }
                                continue;
                            }
							if let Command::Voice(control @ client_core::voice::Command::Sync { channel }) = &command {
								if *voice_availability.borrow() && dm_channels.lock().map_err(|_|Failure::Protocol)?.contains_key(channel)
									&& let Err(mpsc::error::TrySendError::Full(control))=voice_send.try_send(*control) {emit(Event::CommandRejected(Box::new(Command::Voice(control))))?;}
								continue;
							}
							if let Command::Voice(control @ (client_core::voice::Command::StartStream{..}|client_core::voice::Command::StopStream{..}))=&command {
								use client_core::{screen,voice::Event as E};
								let control=*control;
								let (channel,request,stream_request)=match control {
									client_core::voice::Command::StartStream{channel,request,stream_request}
									|client_core::voice::Command::StopStream{channel,request,stream_request}=>(channel,request,stream_request),
									_=>unreachable!(),
								};
								let owner=voice_request.map(|(channel,request,_)|(channel,request));
								let online=*voice_availability.borrow();
								let interruption=lifecycle_changed.borrow().interruption;
								stream_stops.acknowledge(*stream_ended_receive.borrow_and_update());
								stream_stops.reconcile(owner,online,interruption,*takeover_receive.borrow());
								let message=if matches!(control,client_core::voice::Command::StopStream{..}) {
									match stream_stops.request(&voice_send,(channel,request,stream_request),online) {
										Ok(true)=>None,
										Ok(false)=>{
											// No Start for this identity entered the Gateway queue,
											// or its call was already replaced/released.
											emit(Event::Voice(E::Stream{channel,request,stream_request,event:screen::Event::Deleted{reason:None}}))?;
											continue;
										}
										Err(message)=>Some(message),
									}
								} else if !online {
									Some("Voice signaling is disconnected; screen-share action was not sent")
								} else if owner!=Some((channel,request)) {
									Some("The call changed; screen sharing was not started")
								} else if pending_release.is_some() || stream_stops.pending {
									Some("The previous screen share is stopping; retry after it finishes")
								} else {
									match voice_send.try_send(control) {
										Ok(())=>{stream_stops.started((channel,request,stream_request),interruption);None},
										Err(mpsc::error::TrySendError::Full(_))=>Some("Screen-share action was not sent; the voice queue is full"),
										Err(mpsc::error::TrySendError::Closed(_))=>Some("Voice signaling is disconnected; screen-share action was not sent"),
									}
								};
								if let Some(message)=message {
									emit(Event::Voice(E::Stream{channel,request,stream_request,event:screen::Event::Failed(message)}))?;
								}
								continue;
							}
							if let Command::Voice(control @ (client_core::voice::Command::WatchStream{..}|client_core::voice::Command::StopWatching{..}))=&command {
								use client_core::{screen,voice::{Command as V,Event as E}};
								let control=*control;
								let (channel,request,stream_request,streamer)=match control {
									V::WatchStream{channel,request,stream_request,streamer}=>(channel,request,stream_request,Some(streamer)),
									V::StopWatching{channel,request,stream_request}=>(channel,request,stream_request,None),
									_=>unreachable!(),
								};
								let message=if !*voice_availability.borrow() {
									Some("Voice signaling is disconnected; the stream request was not sent")
								} else if voice_send.try_send(control).is_err() {
									Some("Stream request was not sent; the voice queue is full")
								} else {None};
								if let (Some(message),Some(streamer))=(message,streamer) {
									emit(Event::Voice(E::Watch{channel,request,stream_request,streamer,event:screen::Event::Failed(message)}))?;
								}
								continue;
							}
                            if let Command::Voice(client_core::voice::Command::RingRecipient{channel,request,recipient,stop})=&command {
                                use client_core::voice::Event as E;
                                let (channel,request,recipient,stop)=(*channel,*request,*recipient,*stop);
                                // Consume prior invalidations before starting a write for the current revision.
                                recipient_scope_changed.borrow_and_update();
                                let eligible=recipient_action(channel,request,recipient,user.id,voice_request,dm_channels.lock().map_err(|_|Failure::Protocol)?.get(&channel).map(Vec::as_slice))
                                    && recipient_call.lock().map_err(|_|Failure::Protocol)?.as_ref().is_some_and(|call|call.allows(channel,request,recipient,stop));
                                let message=if !*voice_availability.borrow() {Some("Recipient ringing unavailable while disconnected")}
                                    else if !eligible {Some("Recipient ringing expired; no request was sent")}
                                    else if recipient_ringing.as_ref().is_some_and(|(_,job)|!job.0.is_finished()) {Some("Recipient ringing is already pending; wait for the result")}
                                    else {None};
                                if let Some(message)=message {emit(Event::Voice(E::RingFailed{channel,request,message}))?;continue;}
                                drop(recipient_ringing.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let ring_wake=wake.clone();
                                let takeover=takeover_receive.clone();
                                let members=recipient_scope_changed.clone();
                                let available=voice_availability.clone();
                                let lifecycle=lifecycle_changed.clone();let revision=current_lifecycle.interruption;
                                let call_metadata=recipient_call.clone();let call_members=dm_channels.clone();
                                let control=client_core::voice::Command::RingRecipient{channel,request,recipient,stop};
                                recipient_ringing=Some((control,AbortTask(tokio::spawn(async move {
                                    let result=tokio::select! {
                                        biased;
                                        _=wait_for_takeover(takeover,(channel,request))=>return,
                                        _=wait_for_voice_interruption(lifecycle,revision)=>return,
                                        _=wait_for_recipient_invalidation(members,available,control,call_metadata,call_members)=>return,
                                        result=api.ring_call(channel,Some(recipient),stop)=>result,
                                    };
                                    if let Err(failure)=result {
                                        let _=emit(Event::Voice(E::RingFailed{channel,request,message:failure.label()}));
                                        if failure.ends_session(){api.stop();let _=finished.send(Some(failure));}
                                    }
                                    ring_wake.request_repaint();
                                }))));
                                continue;
                            }
							if let Command::Voice(control)=command {
                                use client_core::voice::{Command as V,Event as E};
                                if matches!(control,V::Join{..}|V::Leave{..}|V::AbandonSession{..}) && let Some((guild,request,_,_))=community_call.take() {
                                    drop(community_job.take());
                                    emit(Event::Community(client_core::community::Event{guild,request,result:Err(Failure::Ambiguous)}))?;
                                }
                                let (channel,request)=match control {V::AbandonSession{channel,request}|V::ConfirmSession{channel,request,..}|V::Join{channel,request,..}|V::Ring{channel,request}|V::Leave{channel,request}|V::SetMute{channel,request,..}|V::SetCamera{channel,request,..}=>(channel,request),V::Decline{channel}=>(channel,0),V::ChannelInfo{..}|V::RingRecipient{..}|V::Sync{..}|V::StartStream{..}|V::StopStream{..}|V::WatchStream{..}|V::StopWatching{..}=>unreachable!("sync and stream actions routed above")};
                                if let V::AbandonSession{channel,request}=control {
                                    let owner=voice_request.map(|(channel,request,_)|(channel,request));
                                    if voice_request.is_some_and(|(id,r,_)|id==channel && r==request) {
                                        let _=takeover_send.send_replace(Some((channel,request)));
                                        let _=release_taken_over(&mut voice_request,Some((channel,request)));
                                        drop(ringing.take());drop(recipient_ringing.take());
                                        recipient_call.lock().map_err(|_|Failure::Protocol)?.active=None;
                                    }
                                    queue_abandonment(&voice_send,&mut pending_release,control,owner);
                                    continue;
                                }
                                if matches!(control,V::ConfirmSession{..}) {
                                    if let Some(error)=queue_confirmation(&voice_send,control,voice_request,*voice_availability.borrow()) {report_confirmation_failure(&confirmation_report,generation,error,voice_request,&wake);}
                                    continue;
                                }
                                if abandonment_blocks_join(pending_release,control) {
                                    emit(Event::Voice(E::Failed{channel,request,message:"Previous call is releasing locally; retry joining after it finishes"}))?;continue;
                                }
                                if !*voice_availability.borrow() {
                                    emit(Event::Voice(E::Failed{channel,request,message:"Voice is disconnected; no call was started"}))?;continue;
                                }
                                if join_taken_over(control,*takeover_receive.borrow()) {
                                    emit(Event::Voice(E::Failed{channel,request,message:"Call attempt ended; join again to start a new attempt"}))?;continue;
                                }
                                if matches!(control,V::Join{..}) {drop(ringing.take());drop(recipient_ringing.take());}
                                if let V::Leave{channel,request}=control && voice_request.is_some_and(|(id,r,_)|id==channel && r==request) {drop(ringing.take());drop(recipient_ringing.take());recipient_call.lock().map_err(|_|Failure::Protocol)?.active=None;}
                                let previous_owner=voice_request.map(|(channel,request,_)|(channel,request));
                                let ring=match ring_action(control,user.id,&mut voice_request,dm_channels.lock().map_err(|_|Failure::Protocol)?.contains_key(&channel)) {
                                    Ok(action)=>action,
                                    Err(())=>{emit(Event::Voice(E::Failed{channel,request,message:"Call action expired; no ringing request was sent"}))?;continue;}
                                };
                                if matches!(control,V::Join{..}) {
                                    if let Some(error)=queue_join(&voice_send,control,(channel,request),&mut voice_request) {
                                        recipient_call.lock().map_err(|_|Failure::Protocol)?.active=None;
                                        emit(Event::Voice(error))?;
                                        continue;
                                    }
                                    let private=dm_channels.lock().map_err(|_|Failure::Protocol)?.contains_key(&channel);
                                    recipient_call.lock().map_err(|_|Failure::Protocol)?.join(channel,request,private);
                                } else if matches!(control,V::Leave{..}) {
                                    queue_leave(&voice_send,&mut pending_release,control,previous_owner)?;
                                } else {
                                    match voice_send.try_send(control) {
                                        Ok(())=>{},
                                        Err(mpsc::error::TrySendError::Full(control))=>emit(Event::CommandRejected(Box::new(Command::Voice(control))))?,
                                        Err(mpsc::error::TrySendError::Closed(_))=>return Err(Failure::Network),
                                    }
                                }
                                if let Some((recipient,stop))=ring {
                                    drop(ringing.take());
                                    let api=api.clone();let emit=emit.clone();let voice_send=voice_send.clone();let finished=finished.clone();let ring_wake=wake.clone();
                                    let takeover=takeover_receive.clone();
                                    let lifecycle=lifecycle_changed.clone();let revision=current_lifecycle.interruption;
                                    ringing=Some(AbortTask(tokio::spawn(async move {
                                        let result=tokio::select! {
                                            biased;
                                            _=wait_for_takeover(takeover,(channel,request))=>return,
                                            _=wait_for_voice_interruption(lifecycle,revision)=>return,
                                            result=api.ring_call(channel,recipient,stop)=>result,
                                        };
                                        if let Err(failure)=result {
                                            if !stop {let _=voice_send.try_send(V::Leave{channel,request});}
                                            let _=emit(Event::Voice(E::Failed{channel,request,message:failure.label()}));
                                            if failure.ends_session(){api.stop();let _=finished.send(Some(failure));ring_wake.request_repaint();}
                                        }
                                    })));
                                }
                                continue;
                            }
                            if matches!(command, Command::Invite {..}) {
                                drop(invite.take());
                                let api=api.clone(); let emit=emit.clone(); let finished=finished.clone(); let wake=wake.clone();
                                invite=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Invite{result:Err(f),..} if f.ends_session()=>Some(*f),_=>None};
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::CancelProfile) {drop(profile.take());drop(profile_note.take());continue;}
							if matches!(command,Command::StreamPreview{..}) {
								drop(stream_preview.take());
								let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
								stream_preview=Some(AbortTask(tokio::spawn(async move {
									let event=api.execute(command).await;
									let failure=stream_preview_session_failure(&event);
									let error=emit(event).err().or(failure);
									if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
									wake.request_repaint();
								})));
								continue;
							}

                            if matches!(&command, Command::UserAction { action: client_core::user_actions::Action::LoadNote(_), .. }) {
                                drop(profile_note.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                profile_note=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {
                                        Event::UserAction(client_core::user_actions::Event::NoteLoaded{result:Err(f),..}) if f.ends_session()=>Some(*f),
                                        _=>None,
                                    };
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if matches!(command,Command::Profile{..}) {
                                drop(profile.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                profile=Some(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(command).await;
                                    let failure=match &event {Event::Profile{result:Err(failure),..} if failure.ends_session() && *failure!=Failure::Capacity=>Some(*failure),_=>None};
                                    let error=emit(event).err().or(failure);
                                    if let Some(error)=error {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if let Command::MemberSearch(request) = command {
                                if request.valid() {
                                    member_query_send.send_modify(|queries| { let slot=request.slot; queries[slot]=Some(request); });
                                }
                                continue;
                            }
                            if let Command::Members {guild,channel,request,list_id,thread,ranges} = command {
                                if let Some(channel)=channel {
                                    member_query_send.send_if_modified(|queries| {
                                        let mut changed=false;
                                        for query in queries { if query.as_ref().is_some_and(|r| r.channel!=channel || Some(r.guild)!=guild) { *query=None; changed=true; } }
                                        changed
                                    });
                                }

                                let subscription=match (guild,channel,list_id) {
                                    (Some(guild),Some(channel),list_id) if thread || list_id.is_some() => Some(discord_gateway::MemberSubscription {guild,channel,request,thread,list_id:list_id.unwrap_or_default(),ranges}),
                                    _=>None
                                };
                                member_send.send(subscription).map_err(|_|Failure::Network)?;
                                continue;
                            }
                            if let Command::Prefetch(preload) = command {
                                if let Some(event) = reject_prefetch_at_capacity(&mut prefetches, &preload) {
                                    emit(event)?;
                                    wake.request_repaint();
                                    continue;
                                }
                                let history = match &*preload { Command::History{channel,request,..} => Some((*channel,*request)), _ => None };
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let wake=wake.clone();
                                prefetches.push(AbortTask(tokio::spawn(async move {
                                    let event=api.execute(*preload).await;
                                    let event=match history {Some((channel,request))=>scope_history_failure(event,channel,request),None=>event};
                                    let failure=match &event {Event::Failure(f)|Event::Profile{result:Err(f),..} if f.ends_session() && *f!=Failure::Capacity=>Some(*f),_=>None};
                                    if let Some(error)=emit(event).err().or(failure) {api.stop();let _=finished.send(Some(error));}
                                    wake.request_repaint();
                                })));
                                continue;
                            }
                            if let Command::History { channel, request, .. } | Command::HistoryAround { channel, request, .. } = &command {
                                reply_previews.clear();
                                retain_search_for_history(&mut search,&mut search_channel,*channel);
                                drop(reaction_read.take());
                                let (channel, request) = (*channel, *request);
                                drop(history.take());
                                let api=api.clone();let emit=emit.clone();let finished=finished.clone();let history_wake=wake.clone();
                                history=Some(AbortTask(tokio::spawn(async move {
                                    let event=scope_history_failure(api.execute(command).await,channel,request);
                                    if let Event::Failure(f)=&event&& f.ends_session(){api.stop();let _=finished.send(Some(*f));}
                                    if let Err(f)=emit(event){api.stop();let _=finished.send(Some(f));}
                                    history_wake.request_repaint();
                                })));
                            } else {
                                queue_write(&write_send,command,&*emit)?;
                            }
                        }
                    }
                }
                Ok::<(),Failure>(())
            }.await;
            if let Err(f)=result {let _=finished.send(Some(f));wake.request_repaint();}
        });
		Self {
			sync_api,
			sync_roster,
			sync_failure,
			commands,
			admin_scopes,
			member_watches,
			member_watch_key: std::cell::Cell::new(None),
			preference_scope,
			uploads,
			events,
			typing,
			terminal,
			confirmation_failure,
			share_activity,
			own_presence,
			presence_edits,
			account_presence,
			presence_error,
			game_activity,
			registered_games,
			running_game,
			custom_rich_presence,
			spotify_activity,
			rpc_invite,
			activity_observation,
			activity_sharing,
			activity_sharing_request,
			typing_channel,
			reconnect,
			diagnostics,
			diagnostic_revision,
			diagnostic_copied: std::cell::Cell::new(0),
			foreground: std::cell::Cell::new(super::ForegroundRecovery::new(Instant::now())),
			send_recovery_pending: std::cell::Cell::new(false),
			task,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn history_keeps_same_channel_search_running_but_retires_other_reads() {
		let (sender, mut receiver) = tokio::sync::oneshot::channel::<()>();
		let mut search = Some(AbortTask(tokio::spawn(async move {
			let _sender = sender;
			std::future::pending::<()>().await;
		})));
		tokio::task::yield_now().await;
		let mut channel = Some(model::Id(1));
		retain_search_for_history(&mut search, &mut channel, model::Id(1));
		assert!(search.is_some());
		assert!(matches!(
			receiver.try_recv(),
			Err(tokio::sync::oneshot::error::TryRecvError::Empty)
		));
		retain_search_for_history(&mut search, &mut channel, model::Id(2));
		assert!(search.is_none());
		assert_eq!(channel, None);
		assert!(receiver.await.is_err());
	}
	#[test]
	fn full_write_lane_rejects_the_unsent_message_and_keeps_the_account() {
		let (send, mut receive) = mpsc::channel(1);
		send.try_send(Command::CancelSearch)
			.unwrap_or_else(|_| panic!("empty fixture queue"));
		let mut state = test_support::demo_state();
		let command = state
			.prepare_text_send("Synthetic rejected message")
			.unwrap();
		let content = state.pending.last().unwrap().content.clone();
		let state = Mutex::new(state);
		queue_write(&send, command, &|event| {
			let mut state = state.lock().unwrap();
			let generation = state.generation;
			state.apply(Envelope { generation, event });
			Ok(())
		})
		.unwrap();
		let state = state.lock().unwrap();
		assert_eq!(state.auth, client_core::auth::AuthState::Authenticated);
		assert_eq!(state.pending.last().unwrap().content, content);
		assert_eq!(
			state.pending.last().unwrap().delivery,
			model::Delivery::Rejected
		);
		assert!(matches!(receive.try_recv(), Ok(Command::CancelSearch)));
		assert!(receive.try_recv().is_err());
	}
	use client_core::voice::Command as V;
	use model::Id;

	fn fill(sender: &mpsc::Sender<V>) {
		for _ in 0..8 {
			sender
				.try_send(V::SetMute {
					channel: Id(10),
					request: 2,
					mute: false,
					deaf: false,
				})
				.unwrap();
		}
	}

	#[tokio::test]
	async fn full_voice_queue_retains_one_stop_until_capacity_returns() {
		let (sender, mut receive) = mpsc::channel(8);
		fill(&sender);
		let scope = (Id(10), 2, 3);
		let mut stops = StreamStops::default();
		stops.started(scope, 7);
		assert!(stops.request(&sender, scope, true).unwrap());
		assert!(stops.pending);
		assert!(stops.request(&sender, scope, true).unwrap());
		assert_eq!(receive.len(), 8);

		let _ = receive.recv().await.unwrap();
		let permit = sender.reserve().await.unwrap();
		stops.reconcile(Some((Id(10), 2)), true, 7, None);
		permit.send(stops.take_pending().unwrap());
		assert!(!stops.pending);
		// A delayed server acknowledgement does not make another Stop necessary.
		assert!(stops.request(&sender, scope, true).unwrap());
		assert!(stops.take_pending().is_none());
		let mut deletions = 0;
		while let Ok(command) = receive.try_recv() {
			if let V::StopStream {
				channel,
				request,
				stream_request,
			} = command
			{
				assert_eq!((channel, request, stream_request), scope);
				deletions += 1;
			}
		}
		assert_eq!(deletions, 1);
	}

	#[tokio::test]
	async fn pending_stop_cannot_delete_a_rejoined_or_replacement_share() {
		let (sender, mut receive) = mpsc::channel(8);
		fill(&sender);
		let old = (Id(10), 2, 3);
		let replacement = (Id(10), 4, 5);
		let mut stops = StreamStops::default();
		stops.started(old, 7);
		assert!(stops.request(&sender, old, true).unwrap());

		// Match the production dispatch fence when capacity and a new owner
		// become observable in the same select turn.
		let _ = receive.recv().await.unwrap();
		let permit = sender.reserve().await.unwrap();
		stops.reconcile(Some((Id(10), 4)), true, 7, None);
		assert!(stops.take_pending().is_none());
		drop(permit);
		stops.started(replacement, 7);
		assert!(!stops.request(&sender, old, true).unwrap());
		assert!(stops.request(&sender, replacement, true).unwrap());
		assert!(receive.try_recv().is_ok());
		let controls: Vec<_> = std::iter::from_fn(|| receive.try_recv().ok()).collect();
		assert_eq!(controls.len(), 7);
		assert!(matches!(
			controls.last(),
			Some(V::StopStream {
				channel: Id(10),
				request: 4,
				stream_request: 5
			})
		));
		assert_eq!(
			controls
				.iter()
				.filter(|command| matches!(command, V::StopStream { .. }))
				.count(),
			1
		);
	}

	#[tokio::test]
	async fn pending_stop_expires_on_call_replacement_and_takeover() {
		for (owner, taken_over) in [
			(None, None),
			(Some((Id(10), 4)), None),
			(Some((Id(10), 2)), Some((Id(10), 2))),
		] {
			let (sender, _receive) = mpsc::channel(8);
			fill(&sender);
			let mut stops = StreamStops::default();
			stops.started((Id(10), 2, 3), 7);
			assert!(stops.request(&sender, (Id(10), 2, 3), true).unwrap());
			stops.reconcile(owner, true, 7, taken_over);
			assert!(stops.active.is_none());
			assert!(stops.take_pending().is_none());
		}
	}

	#[tokio::test]
	async fn requested_stop_survives_resume_once_and_matching_ack_finishes_it() {
		let (sender, mut receive) = mpsc::channel(8);
		let scope = (Id(10), 2, 3);
		let owner = Some((Id(10), 2));
		let mut stops = StreamStops::default();
		stops.started(scope, 7);
		assert!(stops.request(&sender, scope, true).unwrap());
		// The Gateway can consume a delete immediately before the socket dies.
		assert!(matches!(receive.recv().await, Some(V::StopStream { .. })));
		stops.reconcile(owner, false, 8, None);
		assert!(stops.pending);
		assert_eq!(stops.active, Some(scope));
		assert!(stops.request(&sender, scope, false).unwrap());
		assert!(receive.try_recv().is_err());
		stops.reconcile(owner, true, 8, None);
		sender
			.reserve()
			.await
			.unwrap()
			.send(stops.take_pending().unwrap());
		assert!(matches!(
			receive.recv().await,
			Some(V::StopStream {
				channel: Id(10),
				request: 2,
				stream_request: 3
			})
		));
		stops.reconcile(owner, true, 8, None);
		assert!(stops.take_pending().is_none());
		stops.acknowledge(Some((Id(10), 2, 1)));
		assert_eq!(stops.active, Some(scope));
		stops.acknowledge(Some(scope));
		assert!(stops.active.is_none());
		stops.reconcile(owner, false, 9, None);
		assert!(stops.take_pending().is_none());
	}

	#[tokio::test]
	async fn leave_under_queue_pressure_releases_stream_before_a_new_join() {
		let (sender, mut receive) = mpsc::channel(8);
		fill(&sender);
		let old = (Id(10), 2, 3);
		let mut stops = StreamStops::default();
		stops.started(old, 7);
		assert!(stops.request(&sender, old, true).unwrap());
		let leave = V::Leave {
			channel: Id(10),
			request: 2,
		};
		let mut pending = None;
		queue_leave(&sender, &mut pending, leave, Some((Id(10), 2))).unwrap();
		// Leave supersedes the still-unsent Stop and is kept in the ordered
		// release lane; its Gateway handler clears the old stream ownership.
		stops.reconcile(None, true, 7, None);
		assert!(stops.take_pending().is_none());
		let join = V::Join {
			channel: Id(10),
			request: 4,
			ring: false,
			mute: false,
			deaf: false,
		};
		assert!(abandonment_blocks_join(pending, join));
		let _ = receive.recv().await.unwrap();
		sender
			.reserve()
			.await
			.unwrap()
			.send(pending.take().unwrap());
		assert!(!abandonment_blocks_join(pending, join));
		let controls: Vec<_> = std::iter::from_fn(|| receive.try_recv().ok()).collect();
		assert!(matches!(
			controls.last(),
			Some(V::Leave {
				channel: Id(10),
				request: 2
			})
		));
		assert!(
			!controls
				.iter()
				.any(|command| matches!(command, V::StopStream { .. }))
		);
		queue_leave(&sender, &mut pending, leave, Some((Id(10), 4))).unwrap();
		assert!(receive.try_recv().is_err());
		assert!(pending.is_none());
	}

	#[tokio::test]
	async fn retained_leave_waits_for_resume_and_rechecks_a_reserved_slot() {
		let (sender, mut receive) = mpsc::channel(8);
		fill(&sender);
		let mut pending = None;
		queue_leave(
			&sender,
			&mut pending,
			V::Leave {
				channel: Id(10),
				request: 2,
			},
			Some((Id(10), 2)),
		)
		.unwrap();
		assert!(release_ready(pending, true));
		assert!(!release_ready(pending, false));
		// Model the Gateway draining controls during its reconnect setup.
		while receive.try_recv().is_ok() {}
		// Capacity was reserved while online, then the offline watch arrived.
		dispatch_release(sender.reserve().await.unwrap(), &mut pending, false);
		assert!(matches!(
			pending,
			Some(V::Leave {
				channel: Id(10),
				request: 2
			})
		));
		assert!(receive.try_recv().is_err());
		dispatch_release(sender.reserve().await.unwrap(), &mut pending, true);
		assert!(pending.is_none());
		assert!(matches!(
			receive.try_recv(),
			Ok(V::Leave {
				channel: Id(10),
				request: 2
			})
		));
		assert!(receive.try_recv().is_err());
		pending = Some(V::AbandonSession {
			channel: Id(10),
			request: 2,
		});
		assert!(release_ready(pending, false));
		dispatch_release(sender.reserve().await.unwrap(), &mut pending, false);
		assert!(matches!(
			receive.try_recv(),
			Ok(V::AbandonSession {
				channel: Id(10),
				request: 2
			})
		));
		assert!(pending.is_none());
	}

	#[tokio::test]
	async fn sent_stop_and_pending_leave_keep_fifo_order_and_closed_queue_is_explicit() {
		let (sender, mut receive) = mpsc::channel(8);
		fill(&sender);
		let scope = (Id(10), 2, 3);
		let mut stops = StreamStops::default();
		stops.started(scope, 7);
		assert!(stops.request(&sender, scope, true).unwrap());
		let _ = receive.recv().await.unwrap();
		sender
			.reserve()
			.await
			.unwrap()
			.send(stops.take_pending().unwrap());
		let mut pending = None;
		queue_leave(
			&sender,
			&mut pending,
			V::Leave {
				channel: Id(10),
				request: 2,
			},
			Some((Id(10), 2)),
		)
		.unwrap();
		let controls: Vec<_> = std::iter::from_fn(|| receive.try_recv().ok()).collect();
		assert!(matches!(
			controls.last(),
			Some(V::StopStream {
				channel: Id(10),
				request: 2,
				stream_request: 3
			})
		));
		sender
			.reserve()
			.await
			.unwrap()
			.send(pending.take().unwrap());
		assert!(matches!(
			receive.recv().await,
			Some(V::Leave {
				channel: Id(10),
				request: 2
			})
		));
		drop(receive);
		stops.started((Id(10), 2, 4), 7);
		assert!(
			stops
				.request(&sender, (Id(10), 2, 4), true)
				.unwrap_err()
				.contains("disconnected")
		);
	}
}
