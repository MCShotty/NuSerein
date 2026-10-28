# Connection recovery evidence

## Member identity follow-up — October 28, 2026

The follow-up starts from clean `dbf6725c` (application source `1a688389`) on the
same PR. It fixes dropped guild nicknames in typing events, ignored live member
changes, accidental nickname/role clearing in partial member-list updates and
mention-result eviction while another query is pending. Sparse live subscriptions
retain updates for resolved authors and mention candidates within the existing
Gateway scheduler. Historical installer and measurements below predate this fix.

Application source is `1607575e9725f0f8609ebde72e0d5d52ce35a6c3`.
Focused Gateway verification passes 91 tests, including a real local WebSocket
that waits for the member subscription before sending identity changes through
the production decoder and reducer. Protocol tests cover omitted/cleared fields,
invalid optional metadata and mismatched identities. Core/UI regressions cover
delayed queries, nickname searches with spaces, role removal, departed members,
typing labels without a sidebar, cache bounds and recovery/account scope.
Local `cargo xtask check` passes with pinned Rust 1.98.1: formatting, strict
workspace Clippy, workspace tests, minimal application and policy checks. Relevant
suites pass 373 core, 139 REST, 91 Gateway, 145 protocol, 346 desktop and 825 UI
tests. Login-handoff and cached-workspace/packager fixtures pass.
[Source CI](https://github.com/MCShotty/Serein/actions/runs/40608770724) passes
fuzz, licensing, SDK and all three release comparison jobs. All four native
application/package jobs (Windows x64/ARM64, macOS and Linux) pass, including
replay and 120-second soaks. Windows clean and cached extracted packages launch
successfully with development FFmpeg paths removed.

[Source Nix CI](https://github.com/MCShotty/Serein/actions/runs/40608770706)
passes on Linux. macOS repeats the inherited `___isPlatformVersionAtLeast` link
failure from `video_gpu.o`, also present in the frozen baseline. Normal native
macOS packaging succeeds. The unchanged dependency-audit findings listed below
also remain; licensing passes separately.

The new release comparison uses the same frozen `dff256ff` baseline and pinned
nightly. All six reducer/wide-channel medians are within the 3% target. Historical
macOS results and their unisolated cause remain recorded below.
See [current measurements](../../performance.md#live-member-identity-follow-up).

The UI reuses existing layouts; this changes displayed names and candidate
availability. No native screenshot or owner-account session has been captured for
this follow-up. Acceptance should rename a member and change/remove their roles
in another client, then check the sidebar, nickname completion and typing label,
including after reconnecting with the sidebar closed.

### Follow-up package provenance

Native packages record GitHub's synthetic merge `8feae4d3`, whose tree exactly
matches source `1607575e`:
`3b62e268614e7390ff257afa43e0b9b394568036`.
Downloaded [Windows x64](https://github.com/MCShotty/Serein/actions/runs/40608770724/artifacts/12784498983),
[Linux](https://github.com/MCShotty/Serein/actions/runs/40608770724/artifacts/12784566540)
and [macOS](https://github.com/MCShotty/Serein/actions/runs/40608770724/artifacts/12783345870)
package archives match their GitHub SHA-256 digests. All 13 inspected Windows
executables/DLLs have the expected x64 PE type.

The [Windows clean/cached evidence](https://github.com/MCShotty/Serein/actions/runs/40608770724/artifacts/12784229719)
records launch exit code 0 for both extracted packages. Both ZIPs are 151,325,261
bytes; their digests differ, so archive reproducibility is not claimed.

[Download the updated Windows x64 installer](https://github.com/MCShotty/Serein/actions/runs/40615927624/artifacts/12785301213).
The installer workflow verifies the source/native-job provenance, builds NSIS,
silently installs it and compares every payload file. Its x64 job passes:
289 matching files, correct PE architecture and an uninstaller present. The
downloaded ZIP matches GitHub's artifact digest; the executable matches its
packaged `SHA256SUMS.txt`. No application/account session is launched by this check.

- File: `serein-0.1.0-1607575-Windows-X64-Setup.exe`
- Source: `1607575e9725f0f8609ebde72e0d5d52ce35a6c3`
- Size: 137,049,712 bytes
- SHA-256: `cac060dbc16182c730ca5427f160b88bb886a8b3f13d8e6ab64286d1045d4fc1`

The companion ARM64 installer repeats the inherited `vcruntime140_1.dll`
architecture mismatch. Its native application/package job passes, but ARM64
installer verification is not claimed.

## Historical connection implementation

Implementation source is `1a688389f668c90c7cdd8a392ea6039866c1d6b6`;
the frozen baseline is `dff256ff` on #30. The task uses a separate worktree
and ordinary commits. Neither main branch nor #30 was changed.
[Source CI](https://github.com/MCShotty/Serein/actions/runs/40591903694)
passed all four native application/package jobs (Windows x64/ARM64, macOS and
Linux), release replay and 120-second soaks. Windows clean and cached extracted
packages launched successfully with development FFmpeg paths removed.

Local `cargo xtask check` passed formatting, strict Clippy, workspace tests,
the minimal application check and policy checks. Relevant suites include 370 core,
139 REST, 86 Gateway, 143 protocol, 346 desktop and 822 UI tests. Login-handoff,
cached-workspace/packager and FFmpeg/Flatpak/Linux packaging fixtures passed locally.
Source CI fuzz, licensing, SDK and replay jobs passed. Three unchanged local
GStreamer tests needed the retained SDK decoder plugin restored after a workspace
restart; the final suite passed without weakening assertions or plugin requirements.

### Regression boundaries

- Actual local WebSocket fixtures exercise cooldowns across RESUME, duplicate/older
  dispatches, partial delivery, coalesced delivery-pressure notifications and fresh
  synchronization after capacity returns. Heartbeat fixtures distinguish old ACKs
  from newer probes and retain actual transmission receipts.
- Local HTTP fixtures exercise independent routes, global throttling, bounded
  retained penalties, nested guild-resource keys, safe startup retries and
  authentication rejection. Oversized history is applied to the real reducer,
  which stays connected and accepts a subsequent history request.
- Desktop fixtures retain FIFO/byte/startup budgets and exercise retired snapshots,
  stale UI batches, fresh-snapshot barriers, delayed pressure notifications, scoped
  unsent-command rejection and reliable Stop/Leave controls.
- Account-event loss revokes uncertain access and retires media while obtaining a
  fresh snapshot. Writes wait for it. This exceptional path may interrupt a
  call/share; ordinary RESUME retains existing voice behavior.

New transport outcomes are appended after existing core enum variants. No storage
schema, serialized preferences, dependency versions, native media ABI or encoder/
capture quality settings changed. Diagnostics retain fixed labels and queue counts
in the existing 32-entry / 8-KiB session ring.

### Package and installer provenance

The Windows package evidence records GitHub's synthetic merge `cda2bd86`.
Its tree exactly matches `1a688389`:
`d2b2e5629ad19d38e9fecc8c14a440b877823708`.
Clean/cached extracted launches exited successfully. Their ZIPs are each
151,290,952 bytes but have different digests; archive reproducibility is not claimed.
All 13 packaged Windows executables/DLLs inspected have the expected x64 PE type.

[Download Windows x64 installer](https://github.com/MCShotty/Serein/actions/runs/40600229668/artifacts/12778986059).
The separate installer workflow verifies the source/native-job provenance,
compiles NSIS from that source, silently installs it and compares every packaged
file. Its x64 job passed: 289 matching payload files, correct PE architecture and
an uninstaller present. This test does not launch the application or use an account.

- File: `serein-0.1.0-1a68838-Windows-X64-Setup.exe`
- Source: `1a688389f668c90c7cdd8a392ea6039866c1d6b6`
- Size: 137,027,863 bytes
- SHA-256: `e64c1c2f1231756925ba944f825656eb3202a59bf43475554503cd0cff0492b6`

Downloaded package and installer ZIPs matched their GitHub artifact digests.
The delivered installer is a separate NSIS build from the normal-packager size
comparison in [performance](../../performance.md#connection-recovery-and-queue-pressure).
Its ARM64 verification still fails on the inherited `vcruntime140_1.dll`
architecture mismatch; ARM64 installer verification is not claimed.

### Measurements and limitations

The performance record compares release workloads with the frozen baseline and a
commit-pinned published nightly, with separate platform results. These measure core
work, not recovery latency, UI presentation, process RSS or media throughput.

**The macOS performance gate is not met.** Wide-channel medians are +3.8% in the
first current-source batch and +4.2% in the repeat; the repeat reducer is +3.2%.
Both batches use byte-identical benchmark executables. The cause is unisolated;
overlapping samples do not establish a pass. An older `02f5b468` batch was
+35.1% on a different host and cannot establish a cross-run improvement.
The benchmark workloads are unchanged. No speculative optimization was applied;
the execution service was disconnected during that measurement pass. It has since
recovered; native macOS profiling is still unavailable from this Linux workspace.

Existing layouts are reused. No native fault-injection/screenshot session or
Windows/macOS input session was available from the Linux workspace. Idle/minimized
CPU/RSS and real network/sleep transitions remain unmeasured. No owner account,
conversation, camera or microphone was used.

The unchanged dependency audit reports crypto-bigint 0.5.5 (RUSTSEC-2026-0387),
ctutils 0.4.2 (RUSTSEC-2026-0385), Tokio 1.53.1 (RUSTSEC-2026-0386) and a yanked
clipboard-win 5.4.1 warning. Licensing passes separately. macOS Nix still fails to
link `___isPlatformVersionAtLeast` from `video_gpu.o`; this also occurs at
`dff256ff` in [baseline Nix CI](https://github.com/MCShotty/Serein/actions/runs/40546161543).
Normal native macOS application/package checks pass. No unrelated dependency or
capture changes were made to hide inherited failures.

Owner acceptance must cover ordinary use, idle/minimized operation, sleep/wake and
network interruption. If a dropout remains, use **Copy Issue Diagnostics** after
recovery. Fixtures do not establish the cause of the owner's reported dropouts.
