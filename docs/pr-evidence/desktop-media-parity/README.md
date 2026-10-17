# Native media parity — October 17, 2026

This review record covers voice-message recording, independent media output,
native proxied-video embeds, focused call strips and receive-side stream clock
correlation. It supplements [Voice & Video](../../voice.md),
[storage limits](../../storage-policy.md) and the Preview SDK documentation.
It does not establish current Discord or physical-device interoperability.

## Source and attribution

The starting fork head is `0762818370be90905715dd10734bea6bf325fc6b`
(`feat/desktop-parity-media`, fork PR #12), including the existing FFmpeg-first,
GPU/HDR/source geometry/preset work and soundboard/playback controls.
The following upstream work by Atsukoro1 was reviewed and adapted:

| Upstream source | Immutable source head | Integrated scope |
| --- | --- | --- |
| ViceVerse-cz/Serein#566 | `a91eae65f0a5371542fbb3bc2969c148b710d631` | Opt-in native voice-message recorder and settings-only extension capability |
| ViceVerse-cz/Serein#533 | `e223b171745eec36c7599b97c847c3a4e4d332be` | Saved independent media device and native watched-stream route |
| ViceVerse-cz/Serein#439 | `f3717c9bed767a07064a3ce109e0f05452bd170f` | Discord-proxied MP4/MOV/M4V embed playback through the existing native player |

Historical upstream screenshots and performance reports were excluded. Provider
webviews were excluded. All network reads remain in the bounded anonymous source
adapter; FFmpeg receives local read/seek callbacks and no network access.
Source changes preserve selected GPU, HDR metadata, source aspect ratio, presets
and Stable codec restrictions. The bundled extension remains labeled Preview.

## Added admission and lifecycle behavior

Record, Stop, Listen and Send are separate gestures. The native host owns encoded
bytes and devices; extension code can configure presentation but cannot capture,
inspect or send audio. Calls, microphone previews and recording share a process
capture lease held until native retirement completes. Recorder PCM preserves its
callback privacy generation through dequeue and DSP. Navigation, account/session
loss, hidden/minimized windows, disabled permission/plugin and call ownership
cancel capture/review/upload. Review and upload share bounded encoded bytes;
ambiguous submissions are never automatically retried.

Explicit media-device selection never falls back to a different device. Worker
revision gates silence retired native callbacks across coalesced mute/unmute and
device changes. Focused strips expose all bounded call tiles through horizontal
scrolling. Existing DM/group/guild join, answer, call-switch and ring admission
remain in place. Service attachment spoiler/explicit/gore/self-harm concealment
continues through the existing conservative spoiler path; no local classifier was
introduced.

Receive clock correlation uses authenticated compound RTCP sender reports only
for current DAVE participants and unambiguous announced media SSRCs. RTP timestamps
survive audio jitter/decode and FFmpeg video reordering. At most 16 source entries
and eight reports per compound are retained. Reports expire after 15 seconds,
audio anchors after 250 ms; decoder-worker waits are capped at 120 ms and check
cancellation every 2 ms. Missing, reordered, stale, mixed or conflicting clocks
fall back immediately. Stop, rekey and source replacement revoke old work.
Rendering and audio callbacks neither wait nor lock this clock.

## Offline verification

Checks use synthetic messages, local HTTP/WebSocket peers or device-free native
components. Rust is pinned to 1.98.1; Cargo uses the committed lockfiles, one
build job and a separate task target directory.

| Check | Result |
| --- | --- |
| `cargo test --locked -p client-core voice_message_send` | 1 passed |
| `cargo test --locked -p discord-api voice_` | 5 passed |
| `cargo test --locked -p extensions --test voice_messages` | 3 passed |
| `cargo test --locked --manifest-path extensions/Cargo.toml -p voice-messages` | 3 passed |
| `cargo test --locked -p ui --features demo --lib recorder_requires_separate` | 1 passed |
| `voice::tests::`, `embeds::tests::` in the headless UI | 35 and 11 passed, including real wheel input reaching the last synthetic focused-strip avatar |
| `cargo test --locked -p local-store app_preferences_round_trip_and_reject_invalid_replacement` | 1 passed, including legacy/default media output and invalid device-ID replacement |
| Exact `stream_sync.rs` compiled with `rustc --edition=2024 --test` | 5 passed: wrap/origins, missing/stale/reordered/conflicting SR, stop/rekey/source change, worker cancellation and source cap |
| Exact `jitter.rs` compiled with `rustc --edition=2024 --test` | 3 passed |
| Existing `Rate` block plus new unit tests extracted unchanged and compiled with `rustc --test` | 3 passed: congestion, REMB expiry/recovery, ceilings/floor |
| `python3 extensions/test_catalog.py` | Passed |
| SDK wiki `scripts/test_sync.py` | Passed; wiki publication deferred until reviewed source is pushed |
| `cargo fmt --all`, `git diff --check` | Passed |
| Scoped Clippy: `ui`, `extensions`, `discord-api`, `client-core`, `local-store`; all targets, `ui/demo`, `-D warnings` | Passed after correcting the new focused-strip test initializer |
| `node tests/license-policy.cjs` | Blocked: `cargo-deny` is absent from the task environment |

The standalone std checks exercise the production source, but are not a substitute
for compiling the complete native crate. Integrated native validation includes
the authenticated RTCP parser, Opus timestamp continuity, recorder bounds/privacy,
shared capture lease, media routing, source probe and native callback tests.
Complete check/package, current native before/after screenshots and matched
performance measurements are coordinated in the integration worktree to avoid
duplicate native builds within the environment's memory/disk budget. They are
pending in this source commit; historical evidence is not substituted for them.

## Acceptance still requiring the owner

The supplied official desktop reference is Stable build 641004 (`c9ac31f`), Host
1.0.9262 x64 (`94186`). No live account, microphone, speaker, capture portal or
physical display was accessed during this task.

The clock path schedules against the audio frame admitted to software playout;
native queue/device latency and physical A/V drift remain unmeasured. Remote
sender-report availability and current SFU behavior remain unverified. Received
stream audio remains mono. Automatic bitrate adaptation already exists and now
has a simple quality status; automatic resolution/FPS changes and simulcast-layer
selection are not implemented. This receive-clock change adds no synthesized
outgoing sender reports. Receiver recovery emits PLI without NACK requests; the
sender accepts bounded NACK/RTX repair. Linux/Windows screen audio shares other
eligible applications rather than only the selected window's application; macOS
retains application-filtered capture. Provider pages requiring browser playback remain
confirmed external links. Stage/event functionality is owned by the separate
community slice. Games and Activities are excluded.
