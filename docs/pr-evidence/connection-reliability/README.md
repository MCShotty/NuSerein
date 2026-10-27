# Connection recovery evidence

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

## Regression boundaries

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

## Package and installer provenance

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

## Measurements and limitations

The performance record compares release workloads with the frozen baseline and a
commit-pinned published nightly, with separate platform results. These measure core
work, not recovery latency, UI presentation, process RSS or media throughput.

**The macOS performance gate is not met.** Wide-channel medians are +3.8% in the
first current-source batch and +4.2% in the repeat; the repeat reducer is +3.2%.
Both batches use byte-identical benchmark executables. The cause is unisolated;
overlapping samples do not establish a pass. An older `02f5b468` batch was
+35.1% on a different host and cannot establish a cross-run improvement.
The benchmark workloads are unchanged. No speculative optimization was applied;
further local profiling is blocked by the disconnected execution service.

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
