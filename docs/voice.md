# DM and server voice

The standard build implements native audio calls in existing one-to-one and group Discord DMs and guild voice and Stage channels. It uses the owner's existing account, Discord signaling/voice servers, Opus, DAVE version 1 for ordinary voice, and authenticated transport encryption for Stage audio. There is no bot, project relay, separate account, recording service or webview call UI. **Live Discord interoperability and physical microphone/speaker behavior have not been tested; milestone 4 has not passed.**

### Independent media output

Settings → Voice & Video offers a saved **Media output** device independently of
the voice speaker device. New attachment audio/video playback uses that selection;
already playing attachments keep their current output until restarted. Watched
screen-share audio switches on its worker when the selection changes, without
rejoining the call. Stream volume/mute and deafen still apply; voice speaker gain
does not affect a separately routed stream. The existing received-stream mono
decode remains unchanged; attachment stereo is preserved.

**Default** keeps the existing routing: attachments use the system output and
watched streams mix into the voice speaker output. A missing or unusable selected
device stops media audio with a visible error and retains the preference; choose
another device to retry. An explicit selection never falls back to another output.
Discovery does not open
devices or capture a microphone. Separate stream playout starts only after the
owner explicitly watches a stream, retains eight 20 ms mono frames (30,720 bytes)
plus at most 60 ms stereo output PCM (23,040 bytes), and discards queued audio on
mute, deafen, device change and stop. Hardware routing and live stream compatibility
remain unverified by the offline tests.

```sh
cargo run --locked
cargo run --locked -- --demo  # offline UI; calling/device access disabled
cargo xtask package                     # standard artifact under dist
```

Voice is included in every build without a feature flag. Source builds require CMake for bundled static libopus; Linux needs ALSA development headers. See [platform requirements](platform-support.md) and the [voice adapter README](../crates/discord-voice/README.md) for dependencies, exact resource limits and protocol tests.

## Stage audience, speakers and reconnects (October 17, 2026)

Stage channels use an explicit media mode selected from the current authenticated
channel type. As specified by the [official DAVE scope](https://github.com/discord/dave-protocol/blob/1b2b706ad4a562ede5c2ef901f9d77f6dbe79446/protocol.md#introduction),
Stages require authenticated transport encryption, with DAVE version 0. Ordinary
voice and DM calls still require DAVE version 1, and a Stage allocation cannot
change that requirement. The Stage UI states that Discord can process its audio;
no end-to-end privacy code is displayed. Service acceptance remains unverified.

Join audience never enables microphone opening, capture or transmission. A current
voice state distinguishes audience, a pending request, invitation and speaker;
missing suppression state grants nothing. Accept invitation or Become speaker is
the explicit local consent gate, followed by a current service speaker state and
normal SPEAK/USE_VAD/server mute checks. Decline, audience demotion, permission loss,
Stage end, failed acceptance and a fresh-media reconnect retire consent and queued
PCM generations. A signaling outage uses the existing bounded fresh-media rejoin
path and returns to audience consent; ordinary voice WebSocket Resume is unchanged.

Requests, moderator invitation/demotion and instance topic/create/end actions use
current channel/call/account permissions and the bounded community worker. The
audience metadata budget stays 4,096 entries / 1 MiB, independently of the 63 remote
audio decoder/jitter resources. Authenticated speaking/source announcements admit
those sources; the audience list does not allocate per-person audio handles. Member
controls page 64 rows. No account or physical audio test follows from offline
loopback AEAD/Opus and device-free generation tests. See the detailed [community
compatibility and pinned references](discord-compatibility.md#scheduled-events-and-stage-channels--october-17-2026).

## Implemented behavior and limits

Resizing the DM or group call stage keeps a conversation following the newest message
anchored above its composer on every frame. Reading older messages retains the existing
scroll position instead of forcing the conversation back to the newest message.

Guild voice channels have a chat icon with a **Show chat / Hide chat** tooltip in the channel header.
Chat uses the existing message timeline, composer, drafts and permission checks without
requiring a voice connection. Wide windows place chat beside the stage; narrow windows
show chat in the main area until Hide chat restores the stage. The existing bounded
history/cache and message transports are shared. Normal-account interoperability remains
unofficial and live-unverified. The offline debug check is
`cargo run --locked -p ui --example voice_chat`.

The Audio menu provides saved Microphone gain and Speaker volume controls from 0% to
200%, initially 100%. Reset levels restores both to 100%. Changes apply to the active call
without reopening devices and survive restart/logout through device preferences. Demo edits
remain session-only. Zero silences that signal; values above 100% boost and may clip.
These are software levels, not system mixer settings or automatic gain control. Existing mute,
deafen, push-to-talk, permission and encrypted-readiness gates continue taking precedence.
Opening settings or changing a level never starts a call or opens a microphone.

Right-click another participant's voice-row name, avatar, or stage card for
**User volume**, from 0% to 200%, and **Reset volume** (100%). Keyboard users can focus
an avatar/name and press Shift+F10. This changes only that person's voice playback before
mixing; the global speaker level and deafen still apply. Speaking indicators remain based on
the received signal. Screen-share audio has its own mix and is unaffected. Overrides are saved
in device preferences and restored before the first UI frame,
so they carry across calls, device changes, restart and logout. Demo edits remain session-only.
At most 64 custom levels are retained; a full table replaces its first retained entry.
Nothing is sent to Discord. Boosting may clip; physical listening and
native slider interaction remain unverified.

Guild voice participant menus also offer **Server mute / Server unmute**, **Move to**,
and **Disconnect** when the account has the corresponding channel permissions.
Sidebar participants can be dragged onto another available voice channel; the Move to
menu provides the same action for keyboard users. Source membership and permissions
are checked again before dispatch. Moving requires Move Members in the source and
Connect in the destination; server mute requires Mute Members. These actions use the
existing single outstanding administration request and bounded REST response. Failures
remain visible in the status bar. Live participant state continues to come from Gateway
events; offline fixtures simulate those events locally. Live moderation remains unverified.

Device-free tests cover gain, clipping, invalid PCM, independent live changes and gates. Actual
gain perception, microphone/speaker hardware, native slider interaction and callback latency
remain unverified. Owner-controlled live checks should include 0/100/200% on each control,
Reset levels, mute/deafen/PTT precedence and changing devices while custom levels are selected.

Start calls the selected existing DM; incoming calls require Answer or Decline. One active call is retained while navigating text conversations. Start rings once after Discord voice transport allocation is confirmed; Answer never rings. Required DAVE group readiness and native device readiness precede the connected-audio state. An allocation with no endpoint waits within the deadline; incompatible states fail visibly. Hangup closes local audio immediately and sends departure; another call waits for the service's departure acknowledgment. No uncertain ring write or failed main Gateway session automatically starts another call.

Before the local voice transport authenticates, own-user voice state frames are
replaceable negotiation candidates: an existing client's state can arrive before the
acknowledgment of this device's Join. Session, token and endpoint changes revise one
bounded candidate. Only validated voice Transport Ready for that exact candidate,
followed by the main Gateway's scoped confirmation acknowledgment, establishes the
local transport session and enables media/ringing. Replacements retire local devices
before trying current credentials, preserve the original 30-second deadline, and do
not send another Join or a hangup. A failed unconfirmed candidate waits for changed
credentials within that same deadline rather than repeatedly retrying it. Timeout,
startup failure or scope loss abandons only this unconfirmed local negotiation;
Gateway acknowledges its release without sending a service hangup. If its bounded
control queue is full, one local release waits for queue space without ending text
signaling; a fresh Join is rejected locally until the release is queued ahead of it.
Stale release commands cannot displace cleanup for the current attempt. A later Join
that encounters the still-full queue fails only that unsent attempt, keeping text
signaling available.

If local transport confirmation cannot enter the bounded control queue, its
channel, attempt and candidate revision accompany a local failure. The desktop
receives it through one fixed-size latest-report watch, independent of the reliable
account event queue and its byte budget. Old-scope reports cannot replace the
current attempt's failure; an older candidate revision cannot replace a newer one.
The watch remains unread until queued reliable signaling is drained, including
replacement candidates or acknowledgments beyond the current frame's event batch.
The original negotiation deadline still bounds local failure under sustained load.
The desktop
consumes it only for the exact current unconfirmed candidate within its original
deadline, then abandons that negotiation through the existing local release path.
Old-candidate failures do not fail a replacement or established call. Text
authentication and signaling remain available.

After confirmation, a different owner session in the same voice channel, or movement
to another non-null channel/guild, clears the local call and closes media without
sending a hangup. A translated informational notice explains the move. Pending
initial ringing is cancelled and queued old ring commands are rejected without
disconnecting text signaling. Join explicitly after local device teardown to take the
call back; no old-client departure acknowledgment is required. This applies to DM and
guild calls. Secrets are bounded, redacted, zeroized and never persisted.

The voice protocol does not document an equality with the main Gateway READY session
ID or identify which physical client generated an own-user state frame. A server-accepted
voice transport establishes only the submitted candidate: if an existing client's
candidate authenticates before this device's newer Join acknowledgment arrives, its
later replacement cannot be proven to be a different physical client's action. The
informational notice describes an invalidated local transport, not verified actor
identity. Synthetic ordering and confirmation transitions are tested; cross-client
live takeover remains unverified.

Opening a one-to-one or group DM also requests its existing call state. An ongoing call shows a
**Call in progress** banner and **Join call**, even after ringing stops or this device leaves.
Join uses the existing connection flow without ringing again; browsing never joins or opens
audio devices. Incoming ringing retains Answer/Decline. Join is disabled while offline or in a voice-unavailable session. Joining or answering another
call asks **Switch calls?** before leaving the current call. Ended/unavailable calls disappear.
This uses the existing unofficial Gateway opcode 13 and CALL_CREATE/UPDATE/DELETE contract,
checked against [discord.py-self's Gateway implementation](https://github.com/dolfies/discord.py-self/blob/master/discord/gateway.py)
and [call dispatch handling](https://github.com/dolfies/discord.py-self/blob/master/discord/state.py)
on September 11, 2026. Local WebSocket and reducer/UI tests establish the implementation;
discovery of a real existing Discord call remains unverified.

`cargo run --locked -- --demo --demo-existing-call` shows a synthetic
ongoing DM call with no local media session. The preview Join button is deliberately disabled.
For the owner-controlled live gate, leave the peer connected in a private DM call, open that
DM in Serein, wait for the banner, then explicitly Join. Verify no new ring, actual two-way
audio, leaving/rejoining while the peer stays, and disappearance after the peer ends the call.

Mute/deafen, saved input/output selection and focused V push-to-talk are implemented. Remappable mute and deafen bindings use global native registration when supported and when a modifier is present; they fall back to focused input on Wayland or when registration is unavailable. Settings → Keybinds offers **Enable global keybinds**, saved on this device and enabled by default. Turning it off unregisters global voice shortcuts; mute, deafen and push-to-talk still work while Serein is focused, with the existing text-entry guards. Push-to-talk releases when focus is lost and is disabled while text entry has focus. It remains focused-only by default. Push to Mute is unassigned by default; once bound, holding it mutes the microphone during a call and releasing it restores the previous state, and it registers globally like push-to-talk. Voice and other remappable actions can also use mouse 3, 4 or 5, optionally with modifiers; left and right click cancel keybind recording instead. A bound mouse button no longer starts middle-click autoscroll or back/forward navigation. Mouse bindings work while Serein is focused on every platform and, with global keybinds enabled, are polled globally on Windows only; macOS, X11 and the Wayland portal have no global mouse-button registration. Devices are initialized only following an explicit call with authenticated empty-room waiting or encrypted readiness, an explicit local microphone test, or an explicitly started voice-message recording; no microphone test or recording runs at startup. Acoustic echo cancellation follows the selected input profile; see below for its limits. During a call, a microphone that fails to open or start, reports a fatal callback error, or delivers no audio callbacks for five seconds is disabled with a visible warning. The call and speaker playback remain connected, and the client periodically retries microphone setup in the background while selecting another input immediately retries. Transient buffer discontinuities and non-fatal stream glitches do not disable the microphone. Ordinary silence does not trigger the warning. Selected speaker failures can fall back to the default output; an unusable output can still fail the call.

One-to-one DM calls accept only their expected peer. Group DM and ordinary server voice calls support up to 64 total participants, with independent bounded decoder/jitter state and mixed mono playback. Ordinary voice accepts only DAVE version 1; encryption downgrades and group identities outside the authenticated participant roster fail closed. Stage mode and its separate audience/source bounds are described below. Call recording is unsupported. Explicit microphone clips use the separate voice-message recorder below. Outgoing screen sharing and macOS camera support is described below. Voice WebSocket resumption has a finite retry budget; failed voice resumption or a fresh main Gateway session requires an explicit new call. A transient main Gateway disconnect suspends controls and cancels pending call HTTP; successful Resume preserves confirmed DM call ownership. Voice credentials, ephemeral DAVE identities and audio stay in bounded session memory. The displayed privacy code applies to the current group epoch; identities are not remembered across calls. Comparing codes does not establish long-term identity verification or text-message encryption.

## Group DM calls

Connected one-to-one and group DM stages also show recipients who have not joined,
with **Ringing…** or **Not in call** status rather than an invented microphone state.
Right-click an absent recipient's avatar to **Stop ringing** that recipient or
**Ring again** after ringing has stopped. These explicit actions target one current
recipient of the joined call; they never join a conversation or start media.
The initial call still rings only once after transport allocation. Recipient write
failures remain visible in the stage while the call continues; no uncertain write
automatically retries. Leaving, changing calls, disconnecting, and changes to DM
membership cancel the one pending recipient write. Guild calls have no ringing
controls. This uses the existing unofficial normal-user call HTTP routes; synthetic
tests do not establish live Discord acceptance.

Existing group conversations expose the same Start/Answer/Decline/Join controls, call stage,
mute/deafen, audio device and gain controls, focused push-to-talk, noise suppression,
privacy code, camera, screen sharing and stream viewing as one-to-one calls. Opening the
conversation only requests call presence. Starting a new call rings the group once; joining
or answering an existing call does not ring again. Group avatars identify incoming calls.
The shared grid fits the participant tiles in the stage while text chat remains available.

Group membership uses the authenticated voice-server roster and the existing DAVE transition
validation, rather than pinning the first recipient as a one-to-one peer. Call metadata admits
only this account and known group recipients, rejects duplicate participants, and is bounded
to 64 participants per call. Recipient removal drops stale participant/watch UI state; removal
of this account closes local media and revokes subsequent call actions.

The offline debug command is `cargo run --locked -p serein --example group_call`.
Group signaling remains unofficial and live-unverified. For owner-controlled verification,
repeat the one-to-one gate below in a private group whose participating clients the owner
controls, including simultaneous speech, additions/removals, ringing/decline/join, camera,
sharing/viewing, last-peer departure/rejoin, and removal of this account. This local `!fast`
implementation pass does not establish production readiness or physical media behavior.

## Soundboard

In a connected server voice channel, the call bar's **Soundboard** button opens a panel with
that server's sounds and Discord's default sounds. Selecting a sound sends one explicit play
request; nothing plays or loads until the panel is opened. Sounds played by other participants
are audible: the service announces each one, and Serein downloads that sound and mixes it into
the call's playback. Your own sound is heard locally once the service accepts the request, so a
rejected request stays silent and shows its reason in the panel.

Playing requires SPEAK and USE_SOUNDBOARD in the channel and is unavailable while deafened or
server muted/deafened, matching the documented service rules. A sound owned by another server
additionally needs USE_EXTERNAL_SOUNDS and names its source server; the panel currently lists
only the connected server's sounds and the defaults, so there is no cross-server picker and no
favorites. One play request is outstanding at a time. Private and group DM calls show the
button disabled. Entrance sounds, emoji-only voice effects and animations are not
implemented. Server sounds are managed in [Server Settings](#managing-server-sounds).

Sound audio is local playback only: it is never encoded into the microphone stream, and the
service does not relay it as voice. Clips enter the same mix as participant voices, after
decryption and before the speaker volume, so the selected output device, **Speaker volume**,
deafen and the echo-cancellation reference all apply. The panel's **Soundboard volume**
(0% to 100%, session-only, initially 100%) scales every sound together with the level the
server configured for it. A participant's **User volume** and local mute also apply to the
sounds they play, and sounds from blocked users are skipped. These controls are re-applied
while a sound plays, so muting, blocking or turning someone down also affects a sound that is
already playing or still downloading. Sounds also play while waiting alone in a channel.

Limits: catalogs are session-only and bounded to 256 sounds and 128 KiB per catalog (one
server's catalog plus the defaults; responses over 256 KiB are rejected). A server sound
change marks that catalog stale and it reloads the next time the panel is shown; a change
that arrives while the catalog is loading triggers one more read. An open **Server Settings >
Soundboard** page reloads its list the same way. Sound files
are fetched without credentials from `cdn.discordapp.com/soundboard-sounds/{id}`, limited to
1 MiB and a 10-second transfer, decoded off the UI and audio threads (MP3, Ogg Vorbis or Ogg
Opus) to 48 kHz mono and truncated at six seconds. At most four downloads run at once and
sixteen decoded clips (at most ten full-length ones, about 11.5 MB) are kept in RAM for the
session; nothing is written to disk. Eight clips can overlap; a ninth replaces the oldest. A
sound that takes more than three seconds to arrive is cached but not played late. Plays of a
sound that is still downloading share that download and each still play when it arrives, up
to sixteen waiting plays; they are queued with the listener's mute, block and volume state at
that moment, not the state when the sound was played. Download or decode failures of another
participant's sound are silent.

The routes and events are [documented by Discord](https://docs.discord.com/developers/resources/soundboard)
for bots; acceptance from a normal account is unofficial and **live-unverified**. Offline
tests cover wire decoding, permission and call-state gates, the HTTP requests against a local
server, Gateway dispatch, the clip mixer, cache bounds, decoding of bundled fixtures and the
panel's click path. They do not establish that Discord accepts the request, that the effect
event reaches this client, or that any sound is audible on a physical device. For the
owner-controlled live gate: join a private server voice channel with a second client you
control, play a default and a server sound from each side, and confirm each is heard once on
both; then check deafen, server mute, a denied USE_SOUNDBOARD role, Soundboard volume at 0%
and 100%, a locally muted participant, and that leaving the call stops playback.
`cargo run --locked -p serein --features demo -- --demo --demo-voice` shows the panel with a
synthetic catalog; the preview never downloads or plays audio.

### Managing server sounds

**Server Settings > Soundboard** lists the server's sounds with their emoji, name and
uploader, and shows the free slots for the server's boost level (8, 24, 36 or 48; 96 with
the `MORE_SOUNDBOARD` feature) when that metadata is available. The page is offered to
members with CREATE_GUILD_EXPRESSIONS or MANAGE_GUILD_EXPRESSIONS. Uploading needs
CREATE_GUILD_EXPRESSIONS; editing or deleting needs MANAGE_GUILD_EXPRESSIONS, or
CREATE_GUILD_EXPRESSIONS for a sound you uploaded. **Upload Sound** is disabled when every
slot is used. Each write is one explicit request followed by a reload of the list; an
unexpected or uncertain outcome is shown and asks for a reload instead of retrying.

**Upload Sound** opens a file picker (MP3, Ogg or WAV up to 16 MB) and then a review with a
waveform, a name (2 to 32 characters), an optional related emoji and a volume. Choosing a
file uploads nothing. The two handles select the part to upload, between 0.2 and 5.2
seconds; dragging an edge past 5.2 seconds carries the other edge along, and dragging the
selected span moves it. On files longer than 20 seconds, holding a handle for one second
with at most 4 points of pointer movement zooms the waveform to a 15-second span around
that handle for fine adjustment; releasing zooms back out. The play button previews the
selection on the default output device at the chosen volume, and a playhead follows it.

A short MP3 or Ogg file (at most 5.2 seconds and 512 KB) whose whole length is selected is
uploaded unchanged. Any trimmed selection, and every WAV file, is re-encoded on a worker
thread as mono Ogg Opus at 96 kbit/s with a 5 ms fade at each cut, which stays far below
512 KB. Audio beyond the first five minutes of a file is not shown; longer files are
truncated rather than rejected (a high-rate stereo file may end sooner, at the decoder's
64 MiB sample limit). While the review is open the decoded audio (48 kHz mono, at most
57.6 MB for five minutes) and its waveform peaks (one per 10 ms, at most 30 KB) are held in
RAM; closing the review, or anything else that ends it, such as a permission change or
signing out, releases them.
Nothing is written to disk. The related emoji is a text field for one Unicode emoji: there
is no emoji picker, and a sound's existing custom emoji is kept on edit but cannot be
chosen. Sounds cannot be previewed from the list.

The routes are Discord's documented Create, Modify and Delete Guild Soundboard Sound;
acceptance from a normal account, and of an Ogg Opus upload in particular, is
**live-unverified**. Offline tests cover the wire shapes against a local server,
permission and reconciliation rules, selection limits, pointer-driven dragging and
hold-to-zoom, file preparation, and an encode/decode round trip of the trimmed clip. The
preview's audible output and the native file picker have not been exercised.
`cargo run --locked -p serein --features demo -- --demo --demo-server-settings
--demo-server-page=soundboard` shows the page with synthetic sounds; adding
`--demo-sound-upload` opens the review with a synthetic 77.8-second waveform and no audio.

## Protocol classification

| Area | Evidence / classification | Verification here |
|---|---|---|
| DM entry, incoming call events and ringing | [discord.py-self Gateway](https://github.com/dolfies/discord.py-self/blob/master/discord/gateway.py), [dispatch](https://github.com/dolfies/discord.py-self/blob/master/discord/state.py), [HTTP](https://github.com/dolfies/discord.py-self/blob/master/discord/http.py): unofficial normal-user behavior | Real local WebSocket op13/op4 join/leave and local HTTP ring/decline tests; no Discord call |
| Voice WebSocket, UDP discovery, RTP and codec negotiation | [Discord voice documentation](https://docs.discord.com/developers/topics/voice-connections): documented transport, not an approval of normal-user clients | Synthetic loopback voice event loop, authenticated RTP and Opus tests |
| Required end-to-end encryption for ordinary voice | [Discord DAVE protocol](https://daveprotocol.com/): documented; [Davey](https://github.com/Snazzah/davey): unofficial implementation, not an independent security-audit claim | Synthetic two-party MLS/DAVE exchange, tamper/replay rejection and encrypted audio across local sockets |
| Soundboard catalog, play request and effect event | [Soundboard resource](https://docs.discord.com/developers/resources/soundboard), [Voice Channel Effect Send](https://docs.discord.com/developers/events/gateway-events#voice-channel-effect-send): documented for bots; normal-account acceptance unofficial | Local HTTP server, synthetic Gateway dispatch and device-free mixer/decoder tests; no Discord request or audible check |
| Microphone, playback, resampling and devices | CPAL/native platform APIs | Device-free capture/resampling tests only; physical audio and permission dialogs unverified |

The [compatibility matrix](discord-compatibility.md) distinguishes this from restricted OAuth/RPC capabilities. No OAuth voice grant or bot connection substitutes for the user's session.

## Owner-controlled live gate

Run only when the owner explicitly elects to test and controls both sides of a private one-to-one DM. Ordinary tests/CI never access Discord or audio devices. Do not put credentials in chat, command-line arguments, screenshots, fixtures or reports.

1. Build `cargo run --locked`. Complete the [normal-user text gate](authentication.md) with the owner's Serein session and an official Discord client. Prefer Serein's own official-login webview; do not extract another application's credential.
2. With headphones on both sides, open the existing private DM and deliberately select Start. Verify that the official client rings, Answer there, and wait for encrypted audio readiness. Compare the displayed current-epoch privacy codes where available. If login, transport, DAVE or permissions fail, record the redacted failure and stop that attempt; do not bypass it.
3. Speak short test phrases in both directions. Confirm intelligibility, latency and absence of unexpected echo. A connected label, participant list or socket handshake alone is not success. Check mute, deafen, focused V press/release, focus loss and navigation to another text conversation.
4. Hang up and verify both clients leave, microphone access ends and another call can start after departure acknowledgment. Reverse direction: call from the official client, test Decline, then a separate Answer. Confirm there is no automatic answer or retry.
5. Deliberately test selected device changes/loss, network interruption and bounded resume, logout during a call, normal exit and repeated join/leave. Check audio-device/task cleanup and retained memory. An abrupt process exit still needs real service departure and platform-cleanup verification.
6. Record date, OS/hardware/build features, which cases passed, redacted failures and measured resource use. Do not retain voices or private conversation contents. Record only observed behavior in the task PR description; Windows, macOS and Linux require separate physical tests.

Until actual two-way official-client audio and the relevant encryption/teardown cases pass, the release voice gate remains blocked. Offline encrypted transport tests are useful implementation evidence, not that gate.


## Server channel workflow and live gate

Select an existing server voice channel to inspect its roster, then explicitly Join. Browsing alone never opens media devices. Participant rows show names/avatars and separate mute/deafen states; the connected channel shows elapsed local connection time. Mute/deafen, audio settings and Leave remain available while reading other channels. Server-enforced mute/deafen cannot be overridden locally. To switch rooms, select the next room and Join, then confirm **Switch call**. The current call closes immediately; the new call waits for the matching service departure acknowledgment and local audio teardown. Cancelling keeps the current call. The pending switch expires after 12 seconds and is cancelled on disconnect, account change or lost target access; it never retries automatically. A rejected/full/inaccessible room fails visibly after the bounded allocation deadline.

An authenticated empty room displays “Connected · waiting for others”; audio devices open for local microphone detection, respecting mute, deafen, push-to-talk and SPEAK permission. Captured audio is consumed locally while alone; transmission waits until another participant joins and DAVE is secured. The client does not transmit unencrypted microphone audio to make an empty room appear connected. A server move, changed voice endpoint/session or fresh main Gateway session requires an explicit rejoin. The roster is session-only, bounded to 4,096 entries and 1 MiB, and is cleared on fresh login/resync and relevant access invalidation; during a resumable disconnect it is labeled last-known until missed events replay. Missing user details use a fallback identity rather than fetching a whole guild directory.

Hovering a participant marked LIVE opens a compact still preview when Discord supplies one.
**Watch Stream** joins that voice channel when no call is active, then starts the existing
receive-only stream path after the call connects. It never silently switches an active call;
hidden or unavailable previews remain explicit. The preview request requires current roster
state plus VIEW_CHANNEL and CONNECT. This normal-user route remains unofficial and live-unverified.

`cargo run --locked -- --demo --demo-voice` shows a separately labeled synthetic roster/call scene, including long names and mute/deafen states. It cannot connect, ring, or access devices. The ordinary `--demo` fixture remains the before/after comparison scenario.

For live verification, the owner must explicitly enable `voice` and control a private guild voice channel and the participating official clients. In addition to the DM gate above: join empty then add two official-client participants; verify actual intelligible audio in every direction and simultaneous speech; exercise encrypted joins/leaves and the last peer leaving/rejoining; check self mute/deafen, server mute/deafen, denied Connect/Speak, full room, deliberate switching, a server move/disconnect and voice region migration; verify devices/keys/tasks are released on Leave/logout/exit. Never record participants or publish private account/channel data. None of these live outcomes is established by the synthetic roster screenshot.

Permission-aware continuation: known VIEW_CHANNEL and CONNECT are required for guild joining;
SPEAK is required before initial or sustained microphone capture, and missing/denied USE_VAD
requires enabled, focused, held push-to-talk. A listen-only join stays muted. Lost CONNECT
ends the active call; merely regaining access never rejoins. The failed-call state remains
visible after Gateway disconnect. These gates are covered by offline permission and
device-free capture tests; owner-operated live permission changes/audio remain unverified.


## Connection and playback recovery

Default-device polling and microphone retries reuse the audio host that opened the
active streams. In particular, the one-second poll no longer creates a new
PulseAudio connection and reactor thread. A failed default-device lookup leaves
healthy streams running; only a successfully identified different default triggers
a switch. Explicit selections and inputs that are not open skip default lookup.
Actual stream failure still uses bounded recovery and creates a replacement host.
This addresses a device-restart path consistent with issue #569's repeated audio
resets; the reporter's Linux/PipeWire call still requires a live retest.

A voice-server crash (WebSocket close 4015) uses the existing two-attempt resume budget,
retaining the UDP connection, acknowledged signaling cursor and encrypted group. Terminal
closes, including 4014, still require an explicit new call. Bounded proposals arriving before
DAVE has a local group are ignored as required by its initial-group procedure; established
groups retain strict proposal validation and no early proposal enables audio.

Device readiness belongs to the current device configuration. A rapid encryption pause while
devices are opening invalidates old readiness even when both events reach one UI frame;
late readiness cannot mark the call connected. Once opened, devices stay open across a brief
rekey while callbacks are silenced and old PCM is discarded.

A watched screen share keeps its separate transport during these call rekeys and voice
resumption. Another participant leaving no longer sends Stop Watching just because the
call briefly leaves the connected phase. Explicit Stop Watching, the streamer's departure,
call failure and a changed call/account still close the view. Synthetic lifecycle coverage
does not establish live Discord behavior.

Received short Opus packets are combined into the normal 20 ms playback frame. The encoded
reorder queue remains bounded to eight packets per speaker; a full queue starts playout early
instead of overflowing while waiting for its usual two-tick startup delay.

These behaviors have synthetic regression coverage. Physical devices and Discord calls still
require the owner-operated live gate above.

Denied SPEAK now opens playback without selecting or initializing a microphone. Regaining
SPEAK prepares input under the current encryption and mute/PTT gates; mute/PTT alone does not
reopen devices. Lost VIEW_CHANNEL drops the stored roster and hides participant rows, including
when no call is active; late updates cannot repopulate an inaccessible channel.


## Stream lag and viewer timeouts (October 3, 2026)

Discord identifies error 2012 as a [video viewer timeout](https://support.discord.com/hc/en-us/articles/30952914470807-Discord-Audio-and-Video-Error-Codes-Troubleshooting-Guide),
which does not identify whether capture, encoding, forwarding or viewer decoding failed.
The Linux-focused code audit corrected these reproducible failures:

- Call, camera and stream UDP writes never wait for socket capacity. A congested
  socket drops the current datagram; continuous send errors retain the existing
  ten-second failure policy. Signaling, audio ticks and feedback can keep running.
- An established stream that loses DAVE transition execution or its new group
  response gets a 30-second recovery deadline. Heartbeat acknowledgments cannot
  keep a stalled rekey waiting forever. Authenticated sole-member waiting remains
  unlimited; a pending transition is not treated as idle waiting.
- Audio resumes at the nearest buffered packet after three concealment packets,
  preserving valid successors across larger loss bursts and sequence wraparound.
  A call tick delayed by at least 80 ms discards old decoded audio, as stream
  playback already did.
- The Linux native video decoder admits at most four compressed access units /
  8,650,752 bytes before the native pipeline. Overflow enters the existing keyframe
  recovery path. GStreamer older than 1.20 uses the bounded software fallback.

Offline checks reproduce the old lost recovery packets, uncapped native queue,
indefinite rekey wait and successful fixed behavior. They do not establish that
Linux-to-official-client error 2012 is resolved. Native portal/GPU behavior and
physical or live media still require owner-controlled verification. A large
keyframe at a very low feedback bitrate can still take seconds to drain; the
current controller does not adapt resolution or implement sender-clock A/V sync.
Use the existing opt-in diagnostics on a deliberate failed attempt to distinguish
capture stalls, decoder failures, loss and transport timing before changing those
policies. No automatic capture, account test or diagnostic logging was added.

## Diagnosing a call that never opens audio

Windows call playback uses the selected speaker's default shared-mode mix format,
with the existing resampler converting 48 kHz call audio when needed. This avoids
choosing a converted format solely by enumeration order. Speaker-open errors
distinguish busy, disconnected, unsupported-format and permission failures when
the audio backend identifies them. This fast local change still needs a retry on
an affected Windows device; it does not establish that every reported failure is fixed.

Call progress distinguishes requesting allocation, connecting to the voice server, checking the
UDP network path, securing audio, and opening audio devices. Negotiation errors identify the
missing server Hello/Ready, transport key, DAVE group or transition execution. These are bounded,
static status messages; no identifiers, tokens, audio or raw signaling are logged.

Audio-device opening has a 20-second deadline after encrypted readiness, including device changes.
If a system device API stalls, local audio is disabled and departure is requested. The existing
worker must retire before another call can open devices; a driver that never returns can require
restarting Serein. The watchdog cannot forcibly cancel an operating-system driver call.

The outgoing DAVE key-package encoding was corrected to match reference implementations; see
[the adapter's source comparison](../crates/discord-voice/README.md#key-package-interoperability-correction).
Actual two-way audio still requires the owner-operated test above.

## Voice connection details

Hover over the connection icon or **Voice Connected** text in the account card to see
voice-path ping, the negotiated voice server and its region when recognized. Click the
same control, or focus it and press Enter/Space, to open connection details and a recent
ping graph. **Voice Debug** shows call, outgoing screen-share and watched-stream
measurements separately. **Copy diagnostics** copies a bounded local diagnostic summary;
it excludes account/participant identifiers, tokens, session identifiers, privacy codes,
keys, signaling contents and media. The debug view also shows the safe call failure reason.

Voice ping matches replies to the existing UDP keepalives; signaling ping measures the
separate WebSocket heartbeat. Missing, expired, duplicate or unrelated replies never
become zero-millisecond samples. Region names are inferred from recognized media-host
conventions, not independently verified locations. Unsupported measurements stay unavailable;
reported outgoing video loss is not an audio-loss estimate. UDP byte rates include control
and repair packets, and decoder deliveries are distinct from configured encoder FPS.

In **Voice Debug**, the Statistics selector switches between the connection and a remote
participant. Names come from the current local roster; selecting a person sends no request.
Incoming audio shows packet/byte counts, receive rate, RFC 3550 arrival jitter, decoded
packets, actual concealment requests and decoder errors. Incoming video shows its codec,
resolution, receive rate, reassembly outcomes, delivered FPS, decoder errors and keyframe
wait. Missing or unforwarded media remains unavailable. There is no individual peer ping:
participants share the voice server route.

Per-source RTP loss is an estimate finalized after a fixed 64-packet reorder window;
duplicates cannot hide gaps, and in-window repairs close gaps. It is
not a sender RTCP report or proof of where loss occurred. Audio bytes count authenticated
UDP media datagrams admitted to the audio path; video bytes count authenticated RTP
payloads, including inner encryption. Concealment includes the bounded tail after speech
stops and must not be interpreted as a packet-loss count. Decoder deliveries can precede
a display/synchronization drop. Copying a selected person's statistics adds only numeric
media measurements, without their name, ID or the roster.

This view does not require an environment variable or write a log. It retains one latest
snapshot and at most 60 ping samples for each of three connection scopes. Participant
counters share the existing speaker and video-source budgets; removed/reassigned sources
lose their counters, and no per-user history or payload is retained. Outstanding latency
probes have four fixed slots and expire after 15 seconds. Once-per-second snapshots
use separate latest-value handoffs, so they cannot fill call-control queues or stop a call
when the UI is slow. Resumes retire old latency samples; source retirement, call teardown
and account changes release their diagnostic state. Synthetic loopback and UI checks cover
these paths; native/live Discord route names and keepalive replies still need owner checks.

## Investigating high CPU during a call

Failed calls show their first safe failure reason, with a **Copy failure reason**
button, in both the sidebar and call stage. The reason remains until the call is
dismissed, including after departure acknowledgments or a later gateway disconnect.
Audio/transport worker failures use a separate fixed slot so a full progress queue
cannot discard the cause. Copying includes only the failure text, not participants,
channel identifiers, credentials or media. The timing summaries below do not explain
a terminal failure; copy the failed-call reason as well when troubleshooting.
`cargo run --locked -p serein -- --demo --demo-voice-failed` previews a synthetic
failure and checks that subsequent cleanup/progress events retain its original reason.

Set `SEREIN_VOICE_DIAGNOSTICS=1` before launching Serein to get aggregate voice
timings on stderr every five seconds and a best-effort final summary on teardown.
`StreamSend` reports the active screen encoder; `Transport` reports camera encoding
and call video decoding; `StreamReceive` reports its own video decoding. Values are
`hardware`, `software`, `mixed` for simultaneously active backends, or `unknown`
when no backend is active. A fallback replaces the hardware indication with software.
For AMF, `amf_split_requested` counts live sessions that requested split encoding;
`amf_split_accepted` counts those whose split property was accepted and encoder
initialization succeeded. For a single screen share, `amf_split_requested=1
amf_split_accepted=1` confirms acceptance. A retry without split reports `1` and
`0`. These developer-only counters require the diagnostic environment variable;
normal controls and the automatic split policy are unchanged.
On macOS, live VideoToolbox sessions require hardware acceleration; failures use
the H.264 software fallback after keyframe recovery; H.265/AV1 have no bundled
software fallback.
For example, launch an already-built macOS app from a terminal:

```sh
SEREIN_VOICE_DIAGNOSTICS=1 SEREIN_FRAME_DIAGNOSTICS=1 /Applications/Serein.app/Contents/MacOS/serein 2> serein-voice.log
```

On Windows PowerShell, set `$env:SEREIN_VOICE_DIAGNOSTICS="1"` and
`$env:SEREIN_FRAME_DIAGNOSTICS="1"`, then launch `serein.exe 2> serein-voice.log`.
On Linux, use the same environment assignments as macOS with the installed executable.
Quit an already-running instance first. Join/leave the call yourself; diagnostics never
enable capture, join a call or send media. Quit normally to obtain the existing UI frame
summary. Remove the environment variables to disable diagnostics on the next launch.

Each voice stage reports `[calls, total_us, max_us]` over `window_ms`:
`echo_render` processes speaker reference; `echo_capture` includes the selected microphone
processing; `noise` isolates RNNoise inference within `echo_capture`
(WebRTC suppression remains inside the combined processor timing);
`encode` includes Opus and outgoing encryption; `mix` includes remote Opus decoding; `receive` measures accepted packet decryption/queueing.
`noise_frames` identifies capture frames processed with suppression enabled.
Audio `wakes` counts worker iterations; Transport `wakes` counts 20 ms timer ticks.
`resets` counts AEC resets from mute transitions or callback overruns; `drops` counts
full capture/playback worker queues; `stalls` counts transport gaps of at least 80 ms.
Stage timings exclude device callbacks, socket waits, device opening and UI rendering.
These are elapsed times, including scheduler preemption, **not process CPU percentages**.
`debug=true` identifies a build with debug assertions. Development builds optimize the
Sonora echo-processing crates, RNNoise (`nnnoiseless` and its FFT chain) and libopus
while keeping application code unoptimized and debuggable; release builds remain the reference for overall performance. Rebuild and
restart to apply this change. Compare speaking, muted and noise-suppression-on/off windows to narrow
the cause; UI frame diagnostics help identify excessive rendering separately.

Logging is off by default. Fixed numeric reports go through an eight-slot queue to a
separate writer; media workers never wait for stderr. Output stops after 8,192 reports
or 8 MiB per process, shared by all calls, so restart for another capture. A full queue
drops summaries. No IDs, device names, endpoints, keys, audio or signaling payloads
are logged; upstream cryptographic tracing remains disabled. No files are created by
Serein. Shell redirection is owner-managed and may include unrelated framework logs.
The device-free check is `cargo run --locked -p discord-voice --example voice_diagnostics`.
Instrumentation alone does not establish the cause of a reported CPU spike or a speedup.

## Acoustic echo cancellation

When enabled by the input profile, Sonora 0.2.0 (a Rust port of WebRTC AEC3) runs on
the audio worker, before denoising, microphone gain and Opus encoding. It uses mixed
speaker output after software volume and resampling, including silence on
underrun/deafen, as the echo reference. The audio worker resets processing history
after opening audio streams, on microphone failure or recovery, when microphone
availability changes or the microphone is unmuted, when audio readiness is
revoked, and after capture or echo-reference queue overruns.
Capture and playback have separate privacy generations, so muting the microphone
preserves speaker playback. Echo-reference frames carry the capture generation;
partial references reset whenever that generation changes.
Echo cancellation can be changed independently in Custom and is bypassed in Studio.
No SDK account, model downloads or additional device access are needed.
Optional digital automatic gain control follows denoising, with a maximum 20 dB gain;
it never changes system microphone gain. Krisp SDK embedding requires a
[commercial license](https://sdk-docs.krisp.ai/docs/licensing-information).

The device-free debug command is `cargo run --locked -p discord-voice --example echo`.
It checks synthetic delayed/reflected echo reduction and preservation of a local
signal. It does not establish real-room quality or Discord interoperability.
AEC needs time to adapt after resets. Bluetooth latency, separate device clocks,
very loud/clipped speakers, simultaneous speech and non-48 kHz hardware need
owner-operated listening checks. The existing linear resampling fallback remains.

## macOS microphone permission

Before opening a microphone for an explicitly joined, secured call, Serein checks
AVFoundation authorization and requests access if undecided. Denied/restricted
access produces a visible error directing the owner to System Settings > Privacy
& Security > Microphone. The worker waits at most 20 seconds, checks call/device
cancellation while waiting, and never opens input after an obsolete grant.
Listen-only calls do not request microphone access. Windows/Linux are unchanged.

Voice-enabled macOS executables embed the same Info.plist used by the packaged
app, including NSMicrophoneUsageDescription, so cargo-run builds also supply the
required explanation. Terminal-launched builds can have permission attributed to
the launching terminal; an existing denial must be changed by the owner in macOS
settings. Restart a rebuilt app before retrying. Actual prompt/capture behavior
still requires the owner-operated check; a successful build is not that check.

## Microphone packet pacing

Outgoing audio preserves callback batches in order instead of retaining only
the newest frame each network tick. One 20 ms lookahead frame smooths normal
callback/worker scheduling variation. Mute, deafen, encryption pauses and a
transport stall of at least 80 ms discard queued capture rather than replaying
stale speech. The existing eight-frame capture channel plus lookahead retains
at most nine frames (34,560 PCM bytes). This adds 20 ms of intentional buffering.
The offline echo example also checks alternating two-frame/no-frame arrivals
and mute/stall flushing; physical cutout resolution still needs a listening check.

Speaking indicators use post-processing outgoing microphone audio and each remote
participant's decoded playout audio. Local activity follows the configured sensitivity,
or −70 dBFS for an open microphone; remote activity retains its display-only −45 dBFS
threshold. Both hold for 200 ms. These detect sound, not speech. Activity snapshots
contain at most 64 IDs (512 bytes), update at most ten times per second, and replace
the previous value rather than queueing UI events. Mute, deafen, permission and call
lifecycle gates hide ineligible activity, including while alone.

## Input profiles and noise suppression

Voice & Video settings offers three saved profiles:

- **Voice Isolation:** RNNoise suppression, AEC3 echo cancellation, digital automatic gain
  control (maximum 20 dB), and −55 dBFS input sensitivity.
- **Studio:** bypasses processing and sensitivity gating. Manual gain, mute, deafen,
  push-to-talk and permission/security gates still apply.
- **Custom:** Off, RNNoise, WebRTC (four suppression strengths), or LavaSRv2;
  independent echo cancellation and automatic gain controls; and optional manual
  sensitivity from −80 to 0 dBFS. The gate uses 3 dB hysteresis, a 200 ms release
  hold and a 5 ms ramp. This controls transmitted audio, not just the speaking glow.

Switching profiles retains Custom settings; editing a preset starts from its visible
values. Saved preferences without a profile migrate to Custom with their prior
RNNoise/Off choice, echo cancellation enabled, and gain control/sensitivity gating off.

The worker processes AEC/WebRTC suppression, then optional RNNoise,
then digital automatic gain, manual gain, the local meter, and sensitivity gating.
Calls and microphone preview share this path; playback audio is not denoised.
Explicit voice-message recording reuses suppression, automatic/manual gain and
sensitivity, with no speaker echo reference and no playback device.
No DSP runs in rendering or native audio callbacks. Settings replace one fixed-size
worker snapshot and do not add a queue. Native PCM queues remain eight frames each.

RNNoise uses bundled nnnoiseless 0.5.2. These processors keep bounded session state,
without retaining call or microphone-preview audio or using remote processing.
The explicit voice-message recorder below retains one bounded microphone clip in
RAM. Quality, CPU cost, physical latency and
cross-platform behavior require owner-operated checks; this local implementation does
not establish production readiness or superiority over Discord's processing.

**LavaSRv2** is an optional local microphone enhancer with its ULUNAS denoiser
always enabled. Select it under **Custom → Noise suppression**. It uses 16 kHz
denoising and restores a 48 kHz output, after echo cancellation and before gain,
metering, sensitivity and Opus. Default profiles remain unchanged. Playback,
screen audio and the separate RNNoise voice-message option are unaffected.

The normal package bundles three hash-pinned models and CPU ONNX Runtime;
there are no model downloads during a call, Python dependency or remote audio
service. A disposable helper owns inference and fixed history, with at most two queued
input blocks and one output block, at most four retained 20 ms output frames, epoch retirement
on privacy/device/settings changes, up to one second for retiring-helper
handoff, a five-second initialization deadline and a 150 ms per-request deadline. Sustained queue pressure or helper failure switches
to RNNoise and shows a short fallback hint. Toggle suppression off and on to
retry. Initialization warms the models on synthetic audio while using RNNoise;
enhancement starts only from fresh PCM after initialization finishes.

Streaming uses 40 ms blocks, 20 ms overlap and 40 ms lookahead; it adds voice
delay and CPU/memory cost. The upstream 44.1 kHz mel configuration is retained
even though waveform output is 48 kHz. This streaming adaptation is not a claim
of quality equivalence to whole-file inference. Pinned assets, licenses and
source provenance are in `packaging/lavasr/bundle.py` and
`assets/licenses/voice/lavasr/PROVENANCE.md`. `serein --self-test-lavasr`
checks packaged model initialization, reset and teardown using synthetic silence,
without requiring real-time inference speed from shared CI machines. Nonzero
model regressions and separate helper benchmarks cover processing; these checks
do not establish physical microphone quality.

## Voice-channel status

Guild voice-channel custom status is displayed beneath the channel name, with
the full text on hover. Initial metadata and live status/clear updates are
independent of joining a call. Text is ephemeral, permission gated and bounded
to 512 entries/1 MiB, with 500 characters per status; disconnect, logout,
channel removal and guild selection retire obsolete state. The status gateway
dispatch/opcode is an unofficial client protocol path; offline fixtures do not
prove live Discord behavior. Editing the server's status is not exposed here.

## Screen sharing

In a connected call, select **Share your screen**, choose a display/window, **Source**, 480p, 720p, 1080p, 1440p, 4K (2160p) or 8K (4320p), 15/30/60 fps, cursor visibility and optional **Share system audio**, then select **Share screen**. The screen button changes to **Stop sharing** while starting/sharing; it also remains available in the compact call controls. Call microphone controls remain independent. Quality selection does not require Nitro, but Discord acceptance and sustained frame rate are not guaranteed. Source uses physical pixels, including Retina/DPI scaling. Presets preserve the actual source aspect ratio: the preset targets height in landscape, width in portrait, and both axes for square sources. Oversized presets are disabled. Switching to a smaller source replaces an oversized selection with the highest fitting preset, or Source when none fits. The existing default preset and 15/30/60 fps controls remain unchanged. The 4K/8K presets need an encoder that accepts those dimensions and target up to 50 Mbps at 60 fps.

For a 16:9 source, 480p retains 854×480 pixels. The selected bandwidth profile determines average and peak rates. Medium is independent of FPS and follows the short edge for custom ratios; High retains the earlier pixel-count/FPS interpolation, capped at 50 Mbps. For example, 6144×2560 resolves to 2592×1080 at 1080p; 5120×1440 resolves to 3840×1080. Custom ratios use the same calculation. Odd dimensions align to even encoder sizes with uniform image fitting and at most minimal alignment padding, never stretching. Camera resolution settings are independent.

Capture uses macOS 14+ ScreenCaptureKit or Windows Graphics Capture. On macOS,
Share Screen offers the native `SCContentSharingPicker` to choose one display or
window. Source discovery only advertises that choice; it does not enumerate private
window titles, request screen-recording permission or open the picker. The owner
confirms quality/audio, then explicitly shares to open the system picker. Cancel,
call teardown and a two-minute choice deadline stop the pending request; no source
is silently selected or retried. Legacy direct display/window filters retain their
existing screen-recording permission requirements. When audio is enabled for a
window, ScreenCaptureKit captures its owning application's audio, which may include
other windows in that application; this is not isolation of one window's audio.
Serein's own playback remains excluded. No microphone is captured by screen sharing.
The native picker has not been opened during synthetic verification. Source dimensions remain unknown until permission and selection complete. Signaling heartbeats and cancellation continue while the picker is pending; unresolved or zero-sized output is never announced. A source resize retires previous-size queued pictures, announces validated new geometry, and resumes with a fresh keyframe under the existing security-generation checks. On macOS the capture configuration refreshes using information for the approved filter, without enumerating other sources or reopening the picker.

Source discovery alone does not start streaming. Closing or minimizing a selected source may pause frames or end capture, according to the native API. Raw screen capture retains the 8K allocation envelope in either orientation: a maximum long edge of 7680, short edge of 4320, and 33,177,600 pixels (132,710,400 tightly packed BGRA bytes; high-bit-depth capture retains its existing separate byte budget). Changes to screen-server metadata, lost video permission, leaving the call and logout stop sharing. The sender never starts itself after reconnection.

Video startup and progress are supervised independently of the native encoder.
After secure output is enabled, a complete keyframe must enter the local UDP socket
within 15 seconds. Locally dropped fragments retire the incomplete picture and
request a fresh keyframe. Audio and heartbeats do not satisfy this video deadline;
an idle source with no required picture does not count as a stall. Local transmission
does not establish that another Discord client decoded the video.

Stop cancels Windows capture, readback and system audio directly. A server stop
request survives queue pressure and a same-call Gateway Resume; a different call
or account cannot inherit it. Local resource retirement and server acknowledgement
are tracked separately, preserving the original failure. AMF's native polling loop
checks cancellation and has a 15-second blocked-call limit. A GPU-driver call already
executing may still fail to return; its owner remains tracked rather than allowing
repeated shares to accumulate abandoned capture/encoder resources. The interface
identifies the stalled operation while release is pending.

Windows capture requests border removal when the native `IsBorderRequired` API is
supported (Windows 11), and keeps the system's default border on Windows 10 builds
without it or if the capability check fails. This avoids an unsupported border
request aborting capture startup. Native Windows verification of this correction is pending.

Linux uses the desktop ScreenCast portal and PipeWire. Share Screen opens the system
screen/window picker after the quality dialog; source discovery never opens that picker.
The default is 720p30. GStreamer captures bounded raw BGRA/P010 pictures.
Encoding uses FFmpeg as
described below. The call stage identifies the active encoder and software fallback.
On Niri, portal frames receive pipeline running-time timestamps before frame-rate
filtering. This handles Niri 26.04's constant presentation timestamps, which otherwise
freeze the preview and prevent video from reaching a viewer who joins later. Detection
uses the colon-separated `XDG_CURRENT_DESKTOP` list; other desktops retain their source
timestamps. FFmpeg backend selection is independent of capture timestamp handling.
Synthetic coverage reproduces the timestamp failure; native/live delivery still needs
verification. Run the offline regression without capturing a desktop or joining a call:

```sh
cargo build --locked -p discord-voice -p platform --features winit/wayland --example linux_screen
target/debug/examples/linux_screen
target/debug/examples/linux_screen --niri-timestamps
```

Outgoing encoding uses CPU BGRA readback, bounded scaling and I420 conversion.
NVENC and AMF accept that I420 picture; Quick Sync receives bounded NV12 packing
from the same input before upload. This path does not provide GPU-only scaling or
zero-copy capture. Capture/conversion cost requires native measurement. Local preview
is capped at 640×360/10 fps and suspended when minimized or viewing another channel.
The system picker requires a ScreenCast-capable portal backend. Native X11 sessions
also offer an explicit “Entire X11 desktop · all monitors · no portal” source using
GStreamer's `ximagesrc` (Good plugins). This shares the whole desktop, not an individual
window; cancelling or failing the portal never selects it automatically. The existing
7680×4320 source caps and bounded encoding/preview queues apply. Native X11 capture
remains unverified. H.265/AV1 sending requires compatible hardware and
explicit codec negotiation. H.264 retains software fallback.

System audio defaults off on Linux and Windows. It shares other applications' playback,
even when sharing one window, and excludes Serein's own audio, including call playback
and watched streams. Exclusion happens at capture on the sender: a viewer cannot remove
their voice once another sender has mixed it into stream audio. Other apps' notifications
and audio remain included. macOS retains ScreenCaptureKit's current-process exclusion.

Linux captures individual playback streams through PulseAudio's per-stream monitor API,
also implemented by PipeWire's PulseAudio server. It excludes Serein and streams whose
application identity cannot be established; it never falls back to a whole-output monitor
or microphone. The ScreenCast portal grants video only; audio uses the existing desktop
audio access, including Flatpak's PulseAudio socket permission. A separate bounded worker
handles discovery, capture and mixing outside video encoding and rendering. No applications
are moved between outputs and no virtual device is installed.

Windows uses native process loopback with `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE`,
excluding Serein and its child processes across outputs. This requires Windows build 20348+
(Windows 11 or Windows Server 2022; ordinary Windows 10 22H2 is older). Unsupported systems
or failed isolation report an audio error; turn audio off to share video alone. There is no
whole-output fallback. Neither adapter records to disk.

Native contracts: [PulseAudio per-stream monitoring](https://www.freedesktop.org/wiki/Software/PulseAudio/Documentation/Developer/Clients/WritingVolumeControlUIs/)
and [Microsoft process-loopback capture](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/).

Both feed the existing stream RTC connection with 48 kHz stereo Opus at 128 kbps and
DAVE/transport encryption, independently of microphone mute. Capture output is gated
by secure readiness. The transport keeps at most 100 ms pending, sends one 20 ms frame
per tick, and clears queued/pending PCM on encryption transitions or a 100 ms stall.
Capture buffers carry their encryption generation; late buffers from an earlier
generation are discarded even if a pause and restart happen between worker iterations.
Stop sharing and call teardown release audio together with the screen capture.

The offline debug command is `cargo run --offline --locked -p discord-voice --example linux_screen`.
It compiles the actual portal/pipeline/worker modules on Linux with GStreamer,
checks pre-cancellation without D-Bus, and exercises synthetic preview, the secure-readiness
gate, stereo audio, bounded slow-consumer behavior, oversized-buffer rejection and software
H.264. It never captures a desktop or opens an audio device. Native Linux portal interaction,
NVENC/AMF/Quick Sync, native performance and Discord viewing remain unverified.
Windows process exclusion, Linux application selection and actual remote sound still
require owner-controlled tests; synthetic samples do not establish those outcomes.
Use two clients with headphones, enable audio on the sender, play another app and speak
from the viewer: the app should be audible without the viewer's voice returning in the
stream. Repeat while starting/stopping apps, changing outputs, rekeying and stopping sharing.

The native demo (`cargo run --locked -p serein -- --demo --demo-voice`) exposes a synthetic picker without OS source discovery or capture. Live screen sharing requires the same owner-controlled login gate as voice testing. See [compatibility and limits](discord-compatibility.md#outgoing-screen-sharing--september-11-2026).

## Camera in calls (macOS, Windows and Linux)

### Video backend and codec settings

FFmpeg is the only camera/screen encoder engine. Voice & Video → Video encoding →
**Advanced Video Encoding** saves codec, bandwidth profile and quality preset locally.
Camera resolution and FPS remain independent: 480p through 8K and 15/30/60 fps,
with the unchanged 640×480/15 fps default. Native modes must supply the requested
size; smaller modes are never enlarged. Delivery is capped at the chosen FPS.
Legacy backend preferences migrate without resetting known codec/resolution/FPS.
Calls and shares snapshot settings at startup; changes do not renegotiate an active
stream. A camera preview stops when its settings change.

**Medium (Recommended)** uses these targets at every codec and frame rate:

| Resolution | Adaptation floor | Average target | Peak ceiling |
|---|---:|---:|---:|
| 480p | 200 kbit/s | 600 kbit/s | 1 Mbit/s |
| 720p | 200 kbit/s | 2 Mbit/s | 3 Mbit/s |
| 1080p | 200 kbit/s | 6 Mbit/s | 8 Mbit/s |
| 1440p | 200 kbit/s | 10 Mbit/s | 14 Mbit/s |
| 4K | 200 kbit/s | 14 Mbit/s | 16 Mbit/s |
| 8K | 200 kbit/s | 24 Mbit/s | 28 Mbit/s |

Low halves Medium's average/peak, retaining the floor. High restores the previous
screen pixel/FPS policy (1440p30 12 Mbit/s, 1440p60 24 Mbit/s), capped at 50 Mbit/s
and never below Medium. Medium/Low interpolate custom sizes by the short edge,
including portrait and ultrawide sources. These are encoder targets, not guaranteed
wire rates or equal visual quality across codecs/FPS. Authenticated feedback can
reduce average/peak to the floor. Announcements carry the selected peak ceiling.

Only positively reported hardware codecs can be newly selected; unknown or failed
queries do not enable choices. Saved selections remain visible with a warning.
H.264 defaults to FFmpeg OpenH264 software fallback when hardware is unavailable.
If a saved H.265/AV1 selection cannot switch to H.264 through the hardware choices,
**Restore H.264 default** explicitly restores H.264/Automatic, including while a query
is pending or has failed. It preserves bitrate, camera resolution and frame rate.
H.265/AV1 have no software encoder and fail visibly without changing codec.
Windows ARM64 is H.264 software-only. macOS supports H.264/H.265 VideoToolbox,
without an AV1 encoder. Incoming FFmpeg decoding and native capture stay independent.

Opening Advanced Video Encoding queries the renderer's exact physical GPU.
Discovery never initializes an encoder or submits pictures: NVENC enumerates codec
and preset GUIDs, AMF reads writable quality enums and validates property values,
oneVPL enumerates codec metadata and queries TargetUsage, and VideoToolbox lists
hardware encoders. Named quality choices intersect driver results with this FFmpeg
build's options. Automatic preserves existing vendor defaults. Incomplete preset
queries expose no new named choices; a complete report for an available codec
removes unsupported saved presets. VideoToolbox has no named quality presets in this
interface.
Encoder startup uses Automatic for a saved preset owned by a different backend,
including H.264 software fallback, without changing codec or bitrate. Explicit
encoder tests reject foreign-vendor presets instead of testing a substituted default.

**Test encoder** explicitly tests the selected codec and quality preset using bounded
640×480/15 fps camera and 1280×720/30 fps screen fixtures (at most 48 pictures each).
It uses no software/other-vendor fallback. Reports label the tested codec/preset and
never override discovery. They do not independently decode output or establish native
capture, higher-resolution performance or live Discord interoperability.

Each query/test runs sequentially in a disposable helper with a ten-second deadline.
Closing/collapsing settings, logout and exit cancel and reap it. Reopening restarts
cancelled discovery; tests always require another click. Reports are bounded,
process-local and unpersisted. Missing APIs, crashes and timeouts remain Unknown.
Offline demos never access drivers, capture, credentials or network sessions.

The media worker uses native FFmpeg contexts without a CLI, bound to the running
renderer GPU (Windows LUID, Linux Vulkan PCI identity, macOS Metal registry ID).
GPU preferences take effect after restart. Unknown identities exclude hardware
instead of selecting another GPU. The old platform encoder stack is removed.
Media Foundation and VA-API encoders are disabled; Quick Sync retains its Linux
VA device interface. Packages retain complete libraries, licenses and rebuild sources.

AMF needs the installed AMD runtime: D3D11 on Windows, Vulkan through AMF on Linux.
Quick Sync uses the static oneVPL dispatcher and an installed Intel GPU runtime,
with the renderer-selected Intel D3D11 device on Windows or its exact PCI-derived
DRM render node and iHD VA driver on Linux.
Packages do not install or redistribute GPU drivers. Flatpak needs matching runtimes
inside its sandbox. Initialization, frame, packet-limit or bounded output-delay
failure first retries with fewer advanced features on the same physical GPU.
H.264 can then retire that backend for software; H.265/AV1 fail visibly.
Bitrate restarts retain the active backend and do not retry earlier failed features;
H.264 software fallback stays software. Native SDK calls and teardown may block;
the bounded queue is not a driver-call deadline. Hardware remains unverified here.

FFmpeg requests no explicit low-latency video mode. Automatic uses
NVENC P5 with high-quality tuning, AMF balanced transcoding for H.264/AV1 and
high-quality usage with the highest `quality` preset for H.265, without a latency
requirement, and Quick Sync's medium preset with its default four-task
parallelism. Explicit supported quality presets replace only those preset options.
VideoToolbox's real-time hint remains disabled. FFmpeg requests sixteen-picture
look-ahead when supported: NVENC uses
`rc-lookahead=16` with 24 surfaces; AMF uses preanalysis with a 16-picture buffer;
QSV uses external bitrate control with `look_ahead_depth=16`, retaining requested CBR/VBR and
the existing bitrate/buffer/frame limits. NVENC driver caps are queried on the exact
adapter. The pinned QSV wrapper rejects driver-corrected unsupported look-ahead
and exposes its effective reference distance. Unsupported feature combinations
fall back on that same GPU before H.264 software fallback. Reordered modes let
the preset/driver choose its reference-buffer structure instead of imposing the
single-reference setting retained for non-reordered modes.

H.265/AV1 NVENC and QSV request up to two B frames where supported; macOS
VideoToolbox also permits H.265 reordering (it exposes no look-ahead control). AMF's
bundled HEVC/AV1 wrappers do not expose B-frame encoding. H.264 B frames remain
disabled: the receiver rewrites H.264 SPS frame-reordering metadata before DAVE
decryption, which makes ordinary reordered H.264 incompatible with that contract.
Encoded access units stay in decode order while their packet PTS selects the
original RTP presentation timestamp from a bounded 48-entry timeline. Camera and
screen prediction chains restart at a fresh keyframe after encoded loss, staleness
or security changes. Separate security reset generations reject older pending or
queued pictures even if another keyframe request is already waiting. Local camera
preview updates while look-ahead is pending. Look-ahead and reordering increase
buffering and driver memory; physical-GPU latency/quality still requires
owner-operated measurement. OpenH264 2.6 only accepts camera-real-time and
screen-real-time usage types; selecting its advertised non-real-time types
fails initialization, so its supported usage types remain in software fallback.

Linux keeps the Rust static OpenH264 and FFmpeg's shared OpenH264 symbols private.
The shared encoder uses `libopenh264-serein.so.8` and prefixed public APIs; native
static archive symbols are hidden in the desktop and voice-test executables.
Host GStreamer plugins must use their own OpenH264 ABI, including Ubuntu's older
2.4 parameter structures. The library patch is included with packaged rebuild
sources; see [the encoder recipe](../packaging/ffmpeg/README.md).

Quick Sync receives NV12 from bounded I420 input. AMF restarts its context for
requested camera/screen keyframes after producing output because FFmpeg 7.1 does not
forward forced picture types. Repeated requests coalesce while the first new keyframe
is pending, so look-ahead can fill. The pinned
FFmpeg OpenH264 wrapper uses camera-realtime/low-complexity tuning for H.264 software
fallback, including screens; active desktop text quality and throughput need native
validation. H.264 software encoding uses the existing FFmpeg/OpenH264 path.

Official builds use the pinned minimal LGPL recipe in
[`scripts/build-ffmpeg.py`](../scripts/build-ffmpeg.py), with GPL/nonfree components,
networking and CLI programs disabled. Explicit decoder/demuxer allowlists cover
H.264, HEVC, AV1 and supported anonymous video attachments. Replaceable shared
libraries, license and corresponding source/patch/recipe travel with each package.
Private library names and ELF symbol versions keep the system GStreamer FFmpeg
plugin independent. The source patch also supplies HEVC QSV's missing internal SEI
helper dependency and bounds dav1d's separate picture allocator. dav1d 1.5.1
provides software AV1 decoding with its BSD-2-Clause notice and corresponding source.

HDR capture preserves half-float or P010 pixels when the approved native source
provides usable color metadata. Outgoing Discord codec negotiation does not
establish receiver 10-bit compatibility, so these sources currently tone-map to
SDR; H.264 remains SDR. The native HEVC/AV1 encoder supports PQ/HLG P010 input,
with 10-bit driver queries separate from ordinary codec support. No HDR signaling
is invented. Incoming PQ/HLG streams and supported HDR attachments share the same
color conversion: native HDR on an opaque compatible display/surface and automatic
tone mapping elsewhere. SDR UI brightness remains unchanged.

The receive queue allows 32 MiB across at most eight active decoders and sixteen
announced sources, with an 8 MiB complete-access-unit limit. Shared media uses a
3 GiB aggregate reservation for conservative surface estimates and retained
pictures; it is an admission budget, not a measured process/GPU memory ceiling.
Only the latest picture is delivered for display. Unsupported profiles, resource
limits and repeated malformed complete frames produce playback errors; transient
loss, rekeying and congestion retain the bounded keyframe recovery path.

Camera output preserves its default 128 KiB encoded-picture budget, rising to
256 KiB at 720p, 512 KiB at 1080p, 1 MiB at 1440p and 2 MiB at 4K/8K. Screen
output remains bounded at 2 MiB per encoded picture. Camera and screen encoders use a
two-second GOP with periodic or requested keyframes and supported reordering for
H.265/AV1. H.264 keyframes repeat SPS/PPS; H.265 repeats VPS/SPS/PPS;
AV1 repeats a sequence header. Rate changes retain the existing 15% reduction /
25% recovery thresholds and restart only the encoder. Capture, portal selection,
audio, secure-readiness gates and queue bounds continue through those restarts.

The sender offers the selected codec, checks the explicit server choice, and uses
matching DAVE encryption and RTP packetization: H.264 RFC 6184, H.265 RFC 7798,
and AV1 OBU fragmentation. Mismatches disable camera while preserving audio, or
reject screen sharing. H.264 incoming decode is advertised separately. Synthetic
fixtures cover codec signaling, encryption, reconstruction, MTU/count bounds,
malformed inputs and software H.264 decoding. Physical GPUs, capture and live
Discord compatibility remain unverified. AV1's handling of DAVE's final OBU size
field also needs official-client interoperability validation.

### Camera capture and delivery

The voice build can send a native camera after an explicit camera-on click in a
connected DM or guild call. The camera button remains available in narrow call controls.
A local preview replaces your avatar in both DM and guild call tiles, including when
you are alone or the participant roster has not arrived. Camera and screen-share
controls are available in the connected, waiting-for-others state.
Permission/device errors appear in the call stage.
Guild camera use requires STREAM permission. Camera-off, permission loss, call failure,
leave and logout stop capture. Peer join/leave rekeys preserve an enabled local camera;
outgoing media remains disabled until DAVE is ready. A transport reconnection stops
capture and requires another click. Demo mode never requests camera or microphone access.

An explicitly started screen share also shows a separate local tile while alone.
The worker retains at most one 640×360 RGBA preview (921,600 bytes), updated at
most ten times per second, plus the bounded UI texture and upload copy. Preview
resizing and color conversion run on the capture worker. Native raw-frame limits
still apply; encoding and network transmission wait for secure media readiness.
Stop, source failure, permission loss, leaving and logout release the preview.
These paths have synthetic coverage; native camera/screen capture and live Discord
viewing still require owner-operated validation.

At the default 480p preset, native capture prefers the closest supported size to
the 640×480 encoder, with a 1280×720 input ceiling (DirectShow preserves its
1920×1080 fallback). Higher presets rank native modes at or above the selected
dimensions, within a 7680×4320 ceiling.
macOS and Windows rank native modes by dimension
distance, then distance from the selected frame rate; macOS explicitly locks the device format
and supported frame duration; AVFoundation aspect fitting preserves nonmatching
native ratios with black bars before the callback. Windows drivers without
frame-rate metadata retain their bounded fallback and rank after known rates at
the same resolution. Linux probes each candidate with its effective
V4L2 interval before ranking it and reapplies the selected interval after the final
format change. Drivers without interval metadata rank last at the same resolution.
Capture is converted to the selected output dimensions, capped at the selected
15/30/60 encoded frames/second, and encoded on a worker. Camera and screen share
use the same selected bandwidth profile. H.264 uses Main profile and a two-second
GOP; H.265/AV1 use Main profile with supported frame reordering;
periodic and requested keyframes recover from dropped frames. See
[backend presets](#video-backend-and-codec-settings).
Invalid output advances through compatible backends; only H.264 has
software fallback. macOS retains one pending
BGRA frame; Windows requests one source buffer and queues at most one RGB frame.
Native camera buffers are capped at 7680×4320 plus at most 4096 stride-padding
bytes per row (150,405,120 bytes). Linux requests two mapped buffers, accepts at
most four within that byte cap, and decodes YUYV or MJPEG on the worker with the
same bounded JPEG allocation limit. Full-resolution conversion/encoder buffers
remain on the worker; the UI retains only a preview at most 640×480 RGB.
One RGB preview, one encoded frame (2 MiB), and up to 2048 RTP
packets from one bounded frame are retained. A one-picture async camera channel wakes
the transport independently of the 20 ms audio tick, allowing delivery above 50 fps.
Packet pacing uses elapsed time at
2.5× the selected peak ceiling, with a bounded catch-up window. FFmpeg uses the GOP and keyframe
recovery described above. Codec-specific DAVE frame encryption precedes RTP fragmentation and the existing authenticated
UDP transport. No camera recording or cache is created; the codec is included on supported platforms.

Video SSRC assignment, codec selection and opcode 12 announcements follow the
[public interoperability implementation](https://github.com/dank074/Discord-video-stream/blob/master/src/client/voice/BaseMediaConnection.ts)
(checked September 11, 2026); these normal-user video extensions remain unofficial and
live-unverified. This initial sender has no adaptive
bitrate or RTP retransmission. Physical
permission/device behavior, delivery to the official client and network-loss performance
require the owner-controlled live gate; an offline launch does not establish those results.

Windows, macOS and Linux expose a camera picker in Voice & Video settings and beside both call
camera controls. Discovery runs on a worker without starting capture. macOS uses AVFoundation device discovery; Linux queries up to 64 V4L2 nodes without configuring or streaming them. Up to
32 device IDs (4 KiB each) and names (256 bytes each) are retained. The selected
ID is session-local. Refresh discovers added/removed devices; a missing selected
device is reported rather than silently opening another camera. Changing selection
stops active capture and requires another camera-on click.

Voice & Video also offers an explicit **Preview camera / Stop preview** control outside
calls. It reuses the bounded camera worker and latest-frame texture; frames have no
network sender and are never recorded. Closing the voice settings page, changing
camera, joining a call, logout, or an error stops the preview. During a camera-enabled
call, settings show the existing call preview. Demo mode never opens a camera.
Physical capture and native permission behavior still require owner verification.

Media Foundation selects a native mode for the requested preset and uses its
video processor to resize/convert to that output geometry in RGB32.
DirectShow discovery/capture additionally covers virtual cameras such as OBS and
NVIDIA Broadcast, which may not appear in Media Foundation enumeration.
Its native input is limited to 1920×1080 at the default preset and up to 8K for
higher presets, converted to RGB24 and fitted into the selected dimensions with
aspect-preserving nearest-neighbor scaling and black bars. It retains one bounded
callback frame and validates the negotiated input allocator against at most eight
bounded buffers. Vendor-driver and
upstream decoder allocations remain separate from those application limits.
Default selection falls back to DirectShow when Media Foundation lists no devices.
Allow desktop camera access in Windows
Settings > Privacy & security > Camera; Windows N may require the Media Feature
Pack. Linux tries `/dev/video0` through `/dev/video63` and uses the first accessible
progressive, single-plane YUYV/MJPEG streaming camera that supports the selected
resolution. Linux fits nonmatching frames into the selected canvas with black
bars; each temporary RGB image remains bounded by 7680×4320×3 bytes. The session or sandbox
must already permit access to its device node; this implementation does not request
camera access through a desktop portal or change device permissions. Cameras
with no supported mode within the input ceiling are rejected before streaming.
The device-free format-selection check is
`cargo run --locked -p discord-voice --example camera_format`; native negotiation
and performance still require owner-operated hardware validation.

Frame waits time out after five seconds without a usable frame; stop is checked at
most every 100 ms while waiting. Native driver initialization/teardown has no hard
deadline. The process-wide worker slot stays occupied until cleanup finishes so rapid
toggles cannot accumulate blocked capture workers. Native driver/codec allocations
are separate from application queue limits and are not a measured whole-process bound.

Windows uses the documented [asynchronous source reader](https://learn.microsoft.com/en-us/windows/win32/medfound/using-the-source-reader-in-asynchronous-mode)
and [bounded 2D buffer locking](https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imf2dbuffer2-lock2dsize).
The virtual-camera fallback uses the documented DirectShow
[device enumerator](https://learn.microsoft.com/en-us/windows/win32/directshow/selecting-a-capture-device)
and [sample grabber](https://learn.microsoft.com/en-us/windows/win32/directshow/using-the-sample-grabber).
Linux follows the kernel's [V4L2 capture interface](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/capture.c.html).
Both feed the existing camera transport; no Discord wire behavior changed in this extension.

### Screen-share audio diagnostics

The existing opt-in `SEREIN_VOICE_DIAGNOSTICS=1` reporter now also emits `StreamSend`
and `StreamReceive` summaries. `StreamSend` encode calls count captured 20 ms audio
frames encoded, encrypted and sent. `StreamReceive` receive calls count accepted
DAVE audio packets; mix calls count decoded frames offered to the parent call's output.
StreamReceive drops count audio/video DAVE decryption failures or a full playback/
decoder handoff queue. `video_send` counts complete encrypted video frames sent;
`video_receive` counts decrypted frames accepted by the decoder queue, not displayed frames.
Each stream report also counts ticks with the transport key, DAVE ready, group ready,
pending transition, waiting, announced, capture ready and audio enabled flags set,
plus audio chunks observed in the capture queue before draining. These distinguish
closed security gates, missing capture data and active outbound video. Tick counts
reset every report; receiver capture-ready and queued-audio counts are always zero.
Windows also emits `ScreenAudio`: `capture_read` counts successful native buffer reads,
`capture_queue` counts chunks handed to the stream, `stalls` counts 50-ms event
timeouts, and `drops` counts packets rejected by the ready/epoch/queue gates. Initial
start and epoch restarts emit bounded checkpoints before/after the native calls;
`capture_restart` counts completed starts/restarts, while `resets` counts attempted
epoch restarts. This separates missing wakeups, empty native buffers and blocked
restart calls without logging audio. Dropped packets before stream readiness are expected.
On an encryption epoch change, Windows recreates the process-loopback client instead
of restarting it with Stop/Reset/Start. Each client's captured audio retains its original
epoch until disposal, so queued samples cannot cross the encryption transition.
Zero sender encode calls means no PCM reached the secure sender; sender activity with
zero receiver receive calls narrows the failure to forwarding, mapping or decryption.
Successful receives/mixes with no sound narrows it to silent source PCM or parent
playback/device gates. These counts do not prove audible sound and do not contain PCM,
participant IDs, credentials or signaling contents. The existing eight-report queue and
8,192-report / 8-MiB process-lifetime limits still apply. Enable it on both endpoints
only for the owner's deliberate test, then disable it after collecting the summaries.

### Remote video diagnostics

Every report line carries `at_ms`, milliseconds since the first reporter started, so
lines from different scopes can be ordered. `Transport` (call camera video) and
`StreamReceive` (watching a screen share) lines append a `video:` group whenever any
remote video counter is non-zero. Each counter names one place a picture can be lost
between the UDP socket and the display, so a frozen viewer is diagnosed from one line:

- `packets` / `rtx`: accepted video RTP packets (including restored retransmissions),
  and received retransmission packets (payload 102). Announced RTX sources can repair
  gaps in the current picture; Serein does not yet send NACK requests.
- `open_failed`: packets of any payload rejected by the transport AEAD.
- `not_ready`: video packets received before the DAVE session was ready or from a
  user outside the group; expected briefly after joining or an epoch change.
- `unknown_ssrc`: packets on a video SSRC no sender announced.
- `incomplete` / `complete`: pictures the RFC 6184 depacketizer discarded (sequence gap
  or missing marker) versus intact encrypted access units.
- `decrypt_failed`: access units that failed DAVE decryption.
- `gated`: predicted pictures rejected because a keyframe is still owed after loss.
- `queue_full`: frames dropped because the decoder thread was behind.
- `decode_queue_ms` / `stale_frames`: maximum decoder queue wait and pictures discarded
  after waiting over 150 ms. Discarding requests a fresh keyframe instead of replaying
  stale video. The native macOS pipeline drains every three submissions.
- `keyframes` / `keyframes_without_params`: keyframes handed to the decoder, and how
  many lacked inline SPS/PPS. A rebuilt decoder cannot start from those.
- `pli_sent`: Picture Loss Indications sent (at most one per owed sender per 500 ms).
- `awaiting_ticks`: 20 ms ticks spent waiting for at least one sender's keyframe.
- `decoder_errors`: decoder failures reported by the decoder thread.
- `pictures`: decoded pictures delivered to the display sink.
- `picture_gap_ms`: the longest gap between delivered pictures in the window (a
  maximum, not a sum). Zero until the first picture is delivered.
- `stall_ticks`: ticks where a source was announced but no picture had arrived for a
  second. Each of those ticks asks every announced sender for a keyframe.

A `signal:` group appears on the same lines whenever any signaling counter is non-zero.
It records opcodes only, never their contents, so a handshake that never completes names
its own missing step: `text` and `binary` count all received events, `other` counts text
opcodes without a dedicated slot, and `ready` (2), `session` (4), `clients` (11),
`sender` (12), `prepare_transition` (21), `execute_transition` (22), `prepare_epoch` (24),
`external_sender` (binary 25), `proposals` (binary 27) and `commit` (binary 29/30) count
the negotiation. Outbound messages are `key_package_sent`, `transition_ready_sent` (23),
`subscribe_sent` (12) and `sink_wants_sent` (15). Only `execute_transition` makes DAVE
ready, so a stream that reports a transport key with `dave_ready=0` and no
`execute_transition` stalled in the MLS handshake rather than in media.

Two recovery paths depend on these counters. A viewer refreshes its video sink wants
every five seconds, and every second while stalled, because Discord stops forwarding
video when that subscription lapses. Separately, video that stops cleanly leaves nothing
marked lost, so no per-picture signal would ever request recovery; after a second without
a decoded picture every announced sender is asked for a keyframe until one arrives.
Both are visible as `sink_wants_sent`, `stall_ticks` and `pli_sent`.

When Discord itself ends a stream, its `STREAM_DELETE` carries a `reason`. Both the
sharer's status and the viewer's notice now show a message derived from it (for example
"Discord reported the stream as ended") instead of the same text a local stop shows, and
an unrecognised value still reads "Discord ended the stream". A `user_requested` deletion
stays silent because it is the local stop acknowledging. With the diagnostics variable set,
the bounded raw value is also printed as `[Serein voice Stream] discord_delete_reason=…`.

Linux application audio reports under `ScreenAudio` as well: `capture_restart` counts
completed application enumerations, `capture_read` monitor reads, `capture_queue` chunks
handed to the stream, `resets` epoch or roster resets, `stalls` 100 ms stalls, and
`app_inputs`, `app_captures`, `app_excluded`, `app_chunks` count the applications the
enumeration allowed, monitor captures started, applications excluded after a failed
capture, and mixed chunks sent. `app_inputs=0` across a share means no other application
exposed a PulseAudio stream with `application.process.id` and
`application.process.binary`, so there was nothing to capture; `app_excluded` rising with
`app_captures` means the server refuses the isolated monitor for that application.
`app_captures` rising while `app_chunks` stays zero means captures start but the
enumerate/verify handshake never completes, which `resets` confirms when it matches `wakes`.

`app_ready` counts captures whose monitors all connected, the step before the confirming
enumeration; `app_captures` rising while `app_ready` stays zero means the monitors never
connect, and both rising while `app_chunks` stays zero means the confirmation keeps
restarting.

Per-application capture attaches to one sink input through PulseAudio's monitor-stream
interface. Sink-input indices are recycled as applications restart their streams, so an
attachment can name an index that is already gone; such an attachment never connects and
never fails either. Each attachment is therefore tracked on its own: it is dropped after a
second without connecting and retried from the next listing under a freshly listed index,
and it is only mixed once a later listing shows the same application still holding that
index. Applications are listed every 500 ms regardless of what the server reports, because
a roster that changes continuously must not stop the confirmations. `app_dropped` counts
attachments discarded this way; a steady trickle is normal for an application that keeps
restarting its stream, while `app_dropped` matching `app_captures` with `app_ready` at zero
means no attachment ever connects. Compare against
`parec --monitor-stream=INDEX -d SINK.monitor`, which uses the same interface.

Linux screen capture reports under `ScreenVideo`: `receive` counts pictures taken from the
pipeline, `encode` times the active video encoder, `drops` counts pictures left in the pipeline
because the transport had not drained the previous one, and `stalls` counts passes where the
pipeline offered nothing. A share that freezes with `stalls` high and `drops` at zero means
the desktop stopped producing pictures; `drops` rising instead means the encoder or the
transport is behind and the pipeline is being held back, which throttles the capture source
until it drains. Software encoding a desktop is the usual reason for the latter, so compare
`encode` against the frame interval before concluding anything about the network.

A capture source with nothing new to send still emits a keepalive picture once a second, so
a frozen share shows as a run of seconds carrying only that keepalive rather than a gap
between pictures. Such a run is reported once it ends as
`[Serein voice Screen] capture_slow_ms=… withheld=…`. A `withheld` of zero means the desktop
stopped producing pictures while the transport was keeping up, which is a compositor or
portal matter; a non-zero `withheld` means the pipeline was held back waiting for the
transport to drain, which throttles the capture source until it catches up.

Three lines name why a share ended, because the status only shows the most recent message
and a later stop overwrites it: `[Serein voice Screen] capture_stopped=…` from the capture
worker, `stream_transport_stopped=…` from its RTC connection, and `share_stopped=…` for the
message the desktop finally acted on. Read them in that order; the first is the cause and
the others usually follow from it.

Reading a freeze: `packets` rising with `pictures` at zero and `picture_gap_ms` growing
confirms the viewer is starved, not the display. High `incomplete` with `gated` and
`awaiting_ticks` near the full window and `pli_sent` rising but `keyframes` at zero
means the sender is not honoring keyframe requests. `keyframes` rising alongside
`keyframes_without_params` and `decoder_errors` means the decoder cannot restart from
the sender's keyframes. `packets` at zero with `awaiting_ticks` high means the media
server stopped forwarding. These are counts only; no video, identifiers or signaling
contents are recorded.

On Windows, use `Start-Process` to attach stderr to the GUI executable; direct shell
redirection can leave an empty file. After closing the previous test instance, run
the intended build on each endpoint with separate output files:

```powershell
$env:SEREIN_VOICE_DIAGNOSTICS="1"
Start-Process .\dist\serein.exe -RedirectStandardError "$PWD\stream-debug-retest.log" -Wait
```

Start sharing promptly after launch so the bounded diagnostic budget covers the test.
Keep these local logs out of commits.

For an owner-operated Windows capture-overhead investigation, use the optional
[`windows_capture` diagnostic tool and game sampler](pr-evidence/capture-overhead/README.md).
They distinguish native capture without readback, readback/conversion, local encoding and
normal full sharing while holding encoder quality settings fixed. The source is always
explicitly selected; ordinary startup and CI never run live capture. Game present intervals,
GPU-engine counters and receiver delivery are separate measurements.

## Explicit voice messages

**Voice Messages** is an opt-in bundled Starter plugin under Settings → Extensions.
Install/enable it and grant its declared voice-message and settings-storage access;
opening its card or saving preferences never opens the microphone. The plugin
configures the native host and never receives microphone samples, encoded audio,
credentials or device access. Settings offer a maximum duration of 5–120 seconds
(default 120), Bars/Line waveform style and RNNoise suppression on/off (default on).
Changes apply to new recordings.

Choose **Record voice message** from the composer's **+** menu in an eligible
conversation, then **Record**. A timer and 64-point waveform show capture progress.
**Stop** closes the microphone before final encoding and presents the duration and
waveform for review. **Listen before sending** plays the clip locally through the
selected media output; **Stop listening** stops it. **Send** uploads one
`voice-message.ogg` voice attachment.
The typed text draft remains available for a separate send. This requires fresh
channel access, ordinary send/attachment permission and Send Voice Messages;
active uploads, message edits and selected ordinary attachments block recording.
Calls, microphone tests and camera previews cannot start conflicting capture.

The recorder uses the selected microphone and existing native permission path,
including the bounded macOS authorization wait. Mute, deafen, Push to Mute and
push-to-talk apply before capture and during recording; a closed gate records
silence while the duration continues. The selected profile's gain and sensitivity
still apply, with the recorder's suppression choice overriding profile suppression.
Capture opens no speaker device or playback echo reference. Review playback
requires its separate explicit Listen action and stops on Send or discard.

One worker encodes mono 48 kHz Opus at a 32 kbit/s target into an Ogg container,
with a 120-second duration ceiling and 8 MiB encoded-byte ceiling. DSP, encoding
and device work stay off rendering and audio callbacks. Audio stays in session
RAM; no audio file, draft blob or recording cache is created. Discard, closing the
dialog, navigation, permission/session loss, disabling the plugin, logout and
call teardown cancel the current capture/review/upload. Native retirement is
fenced before another microphone owner starts. A blocked OS driver can still
delay retirement. Upload cancellation after submission can be ambiguous; check
the conversation before recording again. Sends are never automatically retried.
The recorder receives only the explicitly requested microphone clip; no call
audio or participant stream is connected to it.

`cargo run --locked -p serein --features demo -- --demo --demo-chat --demo-recorder` exposes a
synthetic recorder with generated timer/waveform data, no microphone access and
Send disabled. Device-free checks cover Ogg/Opus output, waveform and resource
bounds, initial mute/control gates, scope cancellation and local HTTP upload
metadata. The attachment uses Discord's documented voice-message fields through
the existing signed-upload path; normal-account acceptance, physical devices,
permissions and audible delivery remain live-unverified. Only the owner may elect
an explicitly controlled private-conversation test; ordinary automation never
captures a microphone or sends a Discord voice message.

## Local microphone preview

Voice & Video settings has an explicit Start testing / Stop testing control, a live RMS
input meter and local playback through the selected speaker. Use headphones to avoid feedback.
The preview shares native device selection, microphone gain, speaker volume and the
selected processing profile with calls. It opens no Discord transport and records nothing. Opening settings
alone never opens streams. Leaving the voice settings page, joining a call, logout and exit
stop the preview. Testing is disabled during calls and in offline demo mode.
The existing eight-frame rings bound loopback PCM; meter state is one atomic value and is not
persisted. The meter is measured before the sensitivity gate; playback follows the gate.
`cargo run --locked -p serein --features demo -- --demo --demo-check-mic-preview`
checks settings rendering and capture guards without opening devices. Physical loopback and
microphone permission prompts remain owner-verified behavior.

Rapid mute/unmute invalidates partial callback PCM.

Recipient ringing consumes the confirmed local call scope. Session replacement or
local abandonment cancels initial and targeted ringing, clears dispatcher ownership
and rejects queued actions from the previous request. Targeted HTTP work checks the
latest ownership, membership revision and connection availability before polling
the network future; already accepted service writes cannot be undone.

Recipient actions also recheck latest validated service ringing and observed peer
presence when dequeued: Ring again requires an absent, non-ringing peer; Stop
ringing requires current service-confirmed ringing. Unknown ringing disables both
actions until call metadata arrives; relevant target/scope changes cancel pending writes.

Pending targeted ringing revalidates the requested recipient when call metadata
changes. An unrelated participant’s mute/camera update preserves an eligible
request; a target joining during a start, being removed, reaching the requested
service state, or losing its current call scope cancels obsolete work. The UI
continues to show service-confirmed state instead of reporting successful state
changes as failures.

Microphone preview restarts and call audio wait for the prior native audio worker to finish shutting down. Incoming video recovery tracks delivered pictures per participant, so an active sender cannot hide another sender’s stalled decoder; source replacement retires its previous progress.

## Stream quality and synchronization audit — October 17, 2026

Screen senders already adjust their bitrate from authenticated RTCP loss reports
and REMB estimates, with bounded pacing and encoder reconfiguration. The selected
source size, aspect ratio, frame rate, codec, GPU and HDR/SDR policy remain intact.
Quality settings explain automatic adjustment; a simple status appears while the
network target is reduced. Automatic resolution/FPS switching and simulcast layer
selection are not implemented.

Watched streams now preserve audio RTP timestamps through the bounded reorder
and decode path and correlate them with the decoded video's FFmpeg presentation
timestamp using authenticated RFC 3550 sender reports for current announced SSRCs.
Only one timestamped audio source supplies a synchronization anchor; mixed,
unknown, stale or inconsistent clocks use the existing immediate-picture path.
No RTP-origin or wall-clock offset is guessed from packet arrival times.

Correlation retains at most 16 user entries with one report per medium and one
software audio-playout anchor each; compound RTCP holds at most eight reports.
Reports expire after 15 seconds and audio anchors after 250 ms. A decoded picture
may wait at most 120 ms on its decoder worker, in cancellable 2 ms increments;
rendering and audio callbacks never wait or take this clock's mutex. Stop,
rekeying and source replacement revoke old waits and decoder lifetimes. The
jitter window retains eight packets as before, now with their optional timestamps.

This schedules video against the audio frame handed to software playout. Native
output queues, device buffer latency, clock drift at physical devices and remote
sender-report availability still require owner-controlled measurements. Received
stream audio remains mono; stereo attachment playback and stereo outgoing screen
audio are separate existing paths. This change consumes sender reports; it does
not synthesize outgoing reports from unknown capture-clock origins. Receivers
retain PLI recovery without emitting NACK requests; the sender's bounded NACK/RTX
repair remains supported. Offline scheduling/reorder/crypto tests do not
establish audible lip sync or current Discord/SFU interoperability.

The screen-audio audit confirms existing Linux per-stream monitors and Windows
process-loopback capture exclude Serein's own playback while sharing other eligible
applications. The native control says **Share system audio**; Linux/Windows do not
offer selected-application-only audio for a selected window. macOS captures the
selected application's audio with current-process exclusion. None of these paths
captures a microphone or writes audio to disk. Permission, isolation across actual
applications and output-device changes still require native owner validation.

The call-layout audit covers DM/group/guild grids, focused video/share, fullscreen,
Show chat and narrow stages. Focused participant strips now scroll to every
remaining tile rather than permanently truncating the roster to the first row.
Existing explicit join/answer/recipient-ring, bounded group membership, mute/deafen,
per-user levels and encrypted readiness remain the admission path. Physical calls,
portal capture, output routing and official-client interoperability remain unverified.
