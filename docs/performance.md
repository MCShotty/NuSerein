# Performance findings

Recent synthetic/offline measurements are workload-specific. They do not establish live Discord
performance, universal device results or application-wide memory bounds. The older upstream PR screenshot,
log and per-run evidence archive has been removed; the summaries below retain the useful results.

## Name styles and bundled name fonts - October 15, 2026

Compared baseline `5f83077e` (`main`) with the name-styles branch on Linux (CachyOS, x86_64),
both built with `cargo xtask package` in separate worktrees and target directories. On this
host the `.deb` stage fails in `dpkg-shlibdeps` for both revisions, so the measurement uses
the release executable and the staged `dist` payload produced before that stage. Compressed
size is `zstd -19` of the executable alone.

| Metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Release executable bytes | 88,266,920 | 91,822,792 | +3,555,872 (+4.03%) |
| Staged `dist` payload bytes (no archives) | 92,445,973 | 96,065,437 | +3,619,464 (+3.92%) |
| Executable, `zstd -19` bytes | 42,744,679 | 44,296,091 | +1,551,412 (+3.63%) |

The growth is the 14 embedded display-name fonts (3,493,132 bytes) and their license texts.
The faces are embedded but parsed only when a visible styled name first needs one; with name
styles off (the default) none is parsed. CPU, process memory and frame timings were not
sampled, and no runtime improvement is claimed.

## Game mention catalog and review fixes - October 14, 2026

Compared baseline `7759f5b5` with game mentions and review fixes at `75c4dfe0`.
Both unsigned Windows portable packages use Rust 1.98.1 and `cargo xtask package`
(`--release --locked --no-default-features`, voice included). The baseline `dist`
was preserved in its separate worktree. The after measurement uses a clean copy
of the generated package, excluding an obsolete `ruzstd-0.9.0-LICENSE` left in the
reused output directory; that original file was preserved. Both measured packages
contain 216 files. ZIP uses Python deflate level 9 with sorted relative paths.

| Metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Executable bytes | 87,321,088 | 87,412,736 | +91,648 (+0.105%) |
| Full portable package bytes | 91,514,377 | 91,605,995 | +91,618 (+0.100%) |
| ZIP distribution bytes | 50,312,736 | 50,338,891 | +26,155 (+0.052%) |

NSIS was unavailable, so installer size was not measured. Native UI testing was
left to the owner at their request; CPU, process memory and frame timings were
not sampled. These package sizes do not measure catalog latency or live Discord
behavior, and no runtime performance improvement is claimed.

## Hover preload review repairs - October 14, 2026

Compared the original PR #636 head `08a9ae01` with runtime repair `d4b27dd3`, which also
integrates `main` at `cf5452cd`. This comparison does not isolate the five fixes from main changes.
Both fresh unsigned Windows portable packages use Rust 1.98.1 and `cargo xtask package`
(`--release --locked --no-default-features`, voice included), on Windows 11 Home 10.0.26200,
Ryzen 7 7800X3D, 33,410,678,784 bytes visible RAM. Separate worktree `dist` outputs were
preserved. Installed bytes sum all 216 files; ZIP uses Python deflate level 9 with sorted paths.
The built release replay ran directly: one warmup and five measured runs of 100,000 events.

| Metric / method | Original PR head | Review repair | Delta |
| --- | ---: | ---: | ---: |
| Package executable | 86,960,640 B | 87,321,088 B | +360,448 B (+0.41%) |
| Installed tree (216 files) | 91,153,929 B | 91,514,377 B | +360,448 B (+0.40%) |
| ZIP, deflate level 9 | 50,180,156 B | 50,312,732 B | +132,576 B (+0.26%) |
| Reducer replay, median of 5 | 73.1705 ms | 68.7076 ms | -4.4629 ms (-6.1%) |
| Retained timeline estimate | 393,492..393,977 B | 393,492..393,977 B | 0 |
| Navigation revalidation requests, 10,000 reselections | 0 | 0 | 0 |
| Hover first visits opened without waiting / requests | 12/12 / 12 | 12/12 / 12 | 0 |
| 28-row hover sweep requests | 12 | 12 | 0 |

The replay run ranges overlap (before 69.0651..76.5154 ms, after 66.9713..71.3841 ms); no speedup is claimed. Navigation retains 2 dormant windows / 150 rows / 328,148 estimated bytes on both revisions; the hover pass retains 8 windows / 983,809 estimated bytes. Core selection median/p95 is 300/300 ns on both, near timer resolution. These are bounded allocation estimates, not RSS.

NSIS is unavailable, so no installer was produced. The existing OpenH264 linker debug-info
warning appears in both builds. Native interaction capture and CPU/RSS sampling were unavailable
(no native computer-use API, no `orca` executable). No live Discord or microphone test was used.
The original feature screenshots are synthetic framebuffer exports, not native interaction proof.
[Per-run outputs and executable hashes](pr-evidence/hover-preload/measurements-review.json)
and [reproduction scenarios](pr-evidence/hover-preload/README.md) retain the evidence boundaries.

## Hover preloading — October 13, 2026

Release builds of baseline `2df8256c` (main) and the hover-preload branch with default features
(voice included), Rust 1.98.1, Windows 11 (build 26200), Intel Core i9-13900K, 32 GiB RAM. The
built `replay-bench` ran directly: one warmup and five measured runs, medians shown. These are
synthetic reducer workloads; they count requests and core selection time, not network latency,
display latency or RSS.

| Metric / method | Baseline | Hover preload | Delta |
| --- | ---: | ---: | ---: |
| Package executable (`xtask package`) | 86,803,968 B | 86,869,504 B | +65,536 B (+0.08%) |
| Installed tree (`dist/`, 216 files) | 90,997,257 B | 91,062,793 B | +65,536 B (+0.07%) |
| Installed tree, zip deflate level 9 | 50,161,038 B | 50,183,621 B | +22,583 B (+0.05%) |
| `replay-bench`: 100,000 events | 58.1 ms | 58.5 ms | noise (+0.6%) |
| `replay-bench`: retained timeline | 393,492..393,977 B | 393,492..393,977 B | 0 |
| `--navigation`: revalidation requests, 10,000 reselections of three 50-message conversations | 10,000 | 0 | −10,000 |
| `--navigation`: select median / p95 | 200 / 300 ns | 200 / 200 ns | at timer resolution |
| `--navigation`: dormant windows / rows / estimated bytes | 2 / 150 / 328,052 B | 2 / 150 / 328,148 B | +96 B |
| Hover pass: first visits that opened without waiting | — | 12/12, 12 requests | new pass |
| Hover pass: requests for a 28-row sweep without clicks | — | 12 | capped at 12/min |

The baseline has no hover pass; there a first visit sends the same one request on click and waits
for it. In the hover pass synthetic responses arrive before the click, so it shows that a click
uses the preload without a second request, not how much latency is hidden; that depends on the
dwell and the network round trip. The three-conversation workload keeps two dormant windows on
both revisions, so the larger eight-window limit is not exercised here. The profile-extras cap
(2 s to 600 ms), image connection reuse and off-coordinator disk reads were not measured: they need
a network or disk-cache fixture this bench does not have.
## Embed styling review - October 14, 2026

Compared the original PR #650 head `c01461ed` with the review fixes in `9354335e` on
Windows 11 Home 10.0.26200, Ryzen 7 7800X3D and 33,410,678,784 bytes physical RAM.
Both standard unsigned portable packages were built with Rust 1.98.1,
`cargo xtask package` (`--release --locked --no-default-features`), including voice.
The installed-tree metric sums file lengths; both ZIPs use .NET `ZipFile` with
`CompressionLevel.Optimal`. Separate worktree `dist` directories were used. Baseline package
artifacts were later removed during a build-cache cleanup; the figures below were recorded
before that removal, and the changed portable tree was rechecked afterwards.

| Metric | Original PR head | Review fixes | Delta |
| --- | ---: | ---: | ---: |
| Package executable | 86,917,632 B | 86,925,824 B | +8,192 B (+0.0094%) |
| Installed portable tree | 91,110,921 B | 91,119,113 B | +8,192 B (+0.0090%) |
| Portable tree, ZIP | 50,316,564 B | 50,320,128 B | +3,564 B (+0.0071%) |

Release `profile_preview --demo --page=embeds --width=1120 --height=900 --interactive`
used the same synthetic fixture in both revisions, wgpu and 125% display scaling. Each
run had a five-second warmup and 20 one-second `Get-Process` samples. Memory is the observed
peak/final sample; CPU is process CPU time divided by elapsed wall time (100% = one core).
The console helper is separate. Other compilation was active during sampling, and there
is only one run per revision, so these small differences are not an improvement claim.

| Native process metric | Original PR head | Review fixes | Delta |
| --- | ---: | ---: | ---: |
| Preview peak / settled working set | 174,346,240 B | 174,428,160 B | +81,920 B |
| Preview peak / settled private bytes | 388,874,240 B | 389,730,304 B | +856,064 B |
| Preview idle CPU, one core | 0.162% | 0.405% | +0.243 percentage points |
| `conhost` peak / settled working set | 10,629,120 B | 10,670,080 B | +40,960 B |
| `conhost` peak / settled private bytes | 1,622,016 B | 1,740,800 B | +118,784 B |
| `conhost` idle CPU, one core | 0% | 0% | 0 percentage points |

Both release builds emit the existing OpenH264 `LNK4255` debug-info warning. NSIS is
unavailable on this host, so packaging skips the installer executable. These measurements
do not establish live Discord interoperability, startup latency or frame percentiles.
The [synthetic native fixture and reproduction procedure](pr-evidence/embed-styling-review/README.md)
describe the capture environment and package method. Dark/light and narrow native PNGs
were inspected. Manual native clicks and keyboard interaction were stopped by the user's
physical Escape key; source destinations and media actions have synthetic interaction tests.

## Profile cosmetics — October 11, 2026

Release builds of baseline `178bde35` and the cosmetics branch on Linux 7.2.9 (CachyOS),
AMD Ryzen 7 7800X3D, 30 GiB RAM, under headless gamescope at 1440 x 900 and scale 1.0.
Default features were used for the package; `--features demo` for the processes. Each run
had a 5 s warmup followed by 20 s sampled from `/proc/<pid>/status` at 1 s intervals; the
table shows the median of three runs. Both binaries were freshly copied first, so their
pages were equally cached. An earlier run against the just-linked branch binary showed
18 MiB more resident file pages from page-cache warmth. Heap and anonymous mappings were
identical in that run, so it is a measurement artifact.

| Metric | Baseline | Cosmetics | Delta |
| --- | ---: | ---: | ---: |
| Package executable | 87,314,600 B | 87,555,752 B | +241,152 B (+0.28%) |
| Installed tree (`xtask package --format dir`) | 91,848,634 B | 92,089,786 B | +241,152 B (+0.26%) |
| Installed tree, `tar` + `zstd -19` | 46,156,864 B | 46,280,238 B | +123,374 B (+0.27%) |
| `serein --demo` settled / peak RSS | 134 / 149 MiB | 134 / 149 MiB | 0 |
| `serein --demo` idle CPU (one core) | 0.25% | 0.25% | noise |
| `profile_preview --page=collectibles` settled / peak RSS | 94 / 110 MiB | 95 / 111 MiB | about +1 MiB |
| `profile_preview --page=collectibles` CPU | 0.25% | 0.25% | noise |

CPU did not change on the fixture page, which suggests the headless window was not playing
the effect (playback pauses while unfocused). Playback CPU was therefore not measured. Decoding two real Discord effect layers (450 x 880
APNG) through the worker's effect budget took 96–138 ms and produced 25–31 frames at
261 x 512. That is 13–16 MiB of frames per layer, against a 24 MiB layer budget and the
existing 128 MiB animation pool.

## GPU colour conversion for decoded video — October 11, 2026

Decoders now hand over native 4:2:0 planes (VideoToolbox NV12, GStreamer NV12/I420,
openh264 I420, Windows live NV12) and one render pass converts them on the GPU. Baseline
`178bde35` (main) and the changed tree were compared with Rust 1.98.1 on the same machines.
Native samples used the synthetic `--demo-video-playing` fixture replaced by a generated
clip for the run only, release builds with LTO off, 5–6 s warm-up, then 10 s of process CPU
time and peak RSS, alternating builds three times.

These measurements cover the parent project's native decoder path. Shared FFmpeg
decoding retains SDR RGBA or HDR P010 with explicit color metadata; that path is
outside these measurements.

| Workload | Before | After | Delta |
| --- | ---: | ---: | ---: |
| macOS M1 Pro, 1080p30 H.264 High + AAC, inline playback CPU | 38.9% | 32.5% | −16% |
| macOS same run, peak RSS | 203.9 MiB | 177.0 MiB | −26.9 MiB |
| Asahi Linux (M1, Vulkan, labwc), 720p30 H.264 Baseline via openh264dec, CPU | 97.0% | 60.4% | −38% |
| Asahi same run, peak RSS | 294.9 MiB | 269.7 MiB | −25.2 MiB |
| openh264 decode + handoff, 120 alternating 1080p/720p frames (release, median) | 473.2 ms | 208.1 ms | −56% |
| openh264 shared scratch capacity | 8,294,400 B | 3,110,400 B | −62% |
| macOS standard executable (`cargo xtask package`) | 68,706,864 B | 68,723,648 B | +16,784 B |
| macOS dist directory / zipped app | 74,814,816 / 48,280,440 B | 74,831,600 / 48,288,340 B | +16,784 / +7,900 B |

Rendered colour bars matched VideoToolbox's own BGRA conversion byte for byte on macOS and
were identical on Linux. The macOS inline gain is smaller because VideoToolbox already
converted in hardware; Windows live NV12 previously converted per pixel on the CPU, but
Windows CPU, GPU memory and frame latency were not measured. Software numbers come from the
ignored `compare_alternating_software_decode` workload. Live calls and streams were not used.

## Long-session live memory inspection — October 9, 2026

Read-only `vmmap -summary`, `footprint` and `heap -s` inspected the owner's already-running
macOS process after 11 hours 33 minutes. No messages, calls, microphone use, UI interaction,
restart, payload dump or credential access occurred. Host: Apple M1 Pro, 16 GiB RAM,
macOS 27.0.1 (26A434). The installed executable's timestamp was October 7;
its source commit and enabled features were unavailable. It therefore appears to predate
main's October 8 idle-animation cleanup (`1b3e4a7b`), but this is not verified build provenance.

Physical footprint was 382.0 MiB, with a lifetime peak of 528.6 MiB. Ordinary `ps` RSS was
about 70.5 MiB; compressed/swapped private pages explain much of the discrepancy. The
697.2 MiB resident-region sum includes shared code/library pages and is not the app's
private footprint. The roughly 390 GiB virtual total is mostly reserved guard address
space, not RAM consumption.

| Allocation/category | Live snapshot | Interpretation |
| --- | ---: | --- |
| Allocated heap across malloc zones | 215.3 MiB | Includes Rust buffers and native objects; not all message state |
| Heap dirty/swapped space beyond allocated bytes | 42.7 MiB | Allocator fragmentation/free space, as reported by `vmmap` |
| Owned graphics backing | 84.4 MiB | Driver/GPU resources charged to the process |
| IOSurface | 26.9 MiB | Shared graphics surfaces |
| IOAccelerator graphics | 1.6 MiB | Additional mapped graphics resources |

These categories leave roughly 11 MiB for other footprint charges. Heap inspection found
205.7 MiB of untyped allocations, including hundreds of blocks around 208–800 KiB; this
is consistent with decoded image/frame sizes, not proof of their owner. Allocation stack
logging was not enabled, so exact attribution to Rust types/functions is unavailable.
One snapshot and a lifetime peak do not demonstrate a leak or a growth rate. Aggregate
sizes only are recorded here; no account data or heap payloads are published.
A second `footprint` sample at 11 hours 54 minutes reported 362 MiB (about 20 MiB less),
with graphics categories unchanged. The original production executable was still running;
no fix had been installed. Host builds were active, and app interaction was not controlled,
so this is an observation of ordinary variation, not an improvement from this PR.

Current main additionally keeps inline still textures in a 96-MiB / 384-item pool until
capacity eviction. The change expires renditions not painted for 60 seconds, using the
existing maintenance wakeup. Visible renditions refresh their deadline, while viewer-close
and five-second unplayed-frame cleanup retain their existing behavior. The pool's byte
accounting describes retained texture/frame resources, not whole-process memory.

Compared baseline `481a0c41` with this change on the same host, pinned Rust 1.98.1,
locked dependencies and release profile (fat LTO). The ignored `ui` library workload
`still_memory_workload` admits 160 synthetic 512×288 stills through the media library,
consumes upload deltas, then advances the maintenance clock by 61 seconds without painting.
It has no window, GPU, network or account. One warmup and three direct test-binary runs per
revision produced identical retained-byte counts:

| Metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Peak retained inline texture bytes | 94,371,840 (90 MiB) | 94,371,840 (90 MiB) | 0 |
| Retained inline texture bytes after expiry | 94,371,840 (90 MiB) | 0 | −94,371,840 (−100%) |

These are texture-size accounting values, not measured GPU allocations or process RSS.
Regression tests additionally verify a renderer free delta, the exact expiry boundary,
painting renewal, reload eligibility and viewer-close cleanup. Native desktop automation
was not exposed in this session, so a scripted native populated-cache comparison was not
obtained. Native GPU reclamation, before/after process footprint, day-long soak, frame
latency and other platforms remain unmeasured. The production app was not replaced.

Both revisions passed standard `cargo xtask package` with voice included, no default
features and the locked release profile. Each app was saved separately; installed bytes
sum all app files, and distribution ZIPs use `ditto -c -k --sequesterRsrc --keepParent`.

| Package metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Executable | 68,657,520 B | 68,657,536 B | +16 B (+0.000023%) |
| Installed app | 74,695,855 B | 74,695,871 B | +16 B (+0.000021%) |
| Distribution ZIP | 48,255,410 B | 48,255,390 B | -20 B (-0.000041%) |

These package deltas are negligible. Both packages were locally ad-hoc signed, not notarized.

## Allocation-stack follow-up and idle artwork — October 10, 2026

The owner's restarted current-main build (`0d9c6772`, optimized release with
matching Rust symbols and `MallocStackLogging=lite`) was inspected after normal
server, image, profile, call and streaming activity. No agent-driven messages,
calls or microphone capture were performed. At roughly four minutes the process
footprint was 290.4 MiB (503.3 MiB peak), with 67.0 MiB live malloc allocations.
Graphics regions included 150.6 MiB owned unmapped backing, about 27 MiB IOSurface
and 1.6 MiB IOAccelerator. The malloc-zone report also included 69.6 MiB dirty
fragmentation/slack and 16.2 MiB profiling-tool data. These categories use
different accounting rules; they must not be added to derive footprint.

Allocation stacks identified 18.6 MiB for `ui::fonts::install_cjk` and 16.0 MiB
for epaint's CPU glyph atlas. The two large worker groups from the earlier
October 7 binary were absent. The previous 501.3-MiB and current 290.4-MiB live
captures had different activity and duration: they are diagnostic snapshots,
not a controlled improvement benchmark. Stack logging adds observer overhead.
Only addresses/stacks and summaries were captured, never allocation contents;
the sorted allocation report was capped at 16 MiB.

The leak scanner flagged 40,016 bytes in 440 blocks. Most belonged to macOS
AppIntents/XPC cycles; about 13 KiB traced to RNNoise's easyfft/generic_singleton
thread-local FFT planner cache. Its dependency deliberately uses `Box::leak`;
this small per-worker retention remains and is not fixed by artwork expiry.

Avatar/banner/activity/sticker/picker textures and custom/high-resolution emoji
textures previously stayed until their item/byte pools filled. They now expire
after 60 seconds without use, using the existing host maintenance deadline.
Visible and cached composer artwork refreshes the deadline. Composer layout only
reads cached still textures; it neither refreshes nor requests clipped assets.
Timeline custom/shared artwork and jumbo Unicode emoji resolve only for visible
slots, so leading overscan rows cannot retain or repeatedly reload them. Expired
artwork uses the normal bounded worker/disk-cache path when painted again.
Existing 64-MiB artwork and 16-MiB emoji ceilings are unchanged.

The ignored `avatars::tests::idle_artwork_workload` compares optimized release
unit-test builds with the pinned Rust 1.98.1 toolchain and lockfile on macOS
27.0.1 / Apple M1 Pro / 16 GiB RAM. The new harness was first run on unmodified baseline source before applying
the runtime change. The same workload submits 932 synthetic
64×64 RGBA textures without draining the existing 128-request batch, so only
128 avatar textures are admitted. It advances the maintenance clock by 61
seconds, with one warmup and three measured runs per build. All runs agree.
This measures retained texture payload accounting without a window or network,
not GPU allocation, process RSS, peak decoding memory or latency. Release samples
are reused from initial implementation `2b60dd8c`; the review follow-up changes
layout/paint access, not expiry accounting. One run of the final debug test binary
confirmed the same 2,097,152 B peak and zero settled payload. A redundant optimized
UI-test rebuild was stopped during compilation; it produced no new release sample.
Separate UI
regressions cover both cache pools, clipped/visible artwork, reloads and cached
composer image painting. Native before/after process and GPU measurements are
unmeasured; attempted standard-package `--demo` processes exited before a valid
native sample could be captured. Their exit cause was not established and no
zero-RSS result is used as evidence.

| Texture payload metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Peak admitted payload | 2,097,152 B | 2,097,152 B | 0 B |
| Payload after idle maintenance | 2,097,152 B | 0 B | -2,097,152 B / -100% |
| Retained cache entries after idle maintenance | 128 | 0 | -128 |

GPU drivers and allocators may reserve freed resources; these payload reductions
are not an equal whole-process RAM reduction guarantee. Returning after expiry
can re-decode cached artwork or fetch it again after a disk-cache miss. Font
storage and the shared Unicode emoji atlas retain their existing lifetimes.

The standard voice-inclusive package was rebuilt without demo/developer features.
The preserved baseline is the verified PR #618 package at `c16cb2df`, whose source
tree matches starting main `0d9c6772`; its executable hash was checked against
the baseline copy before comparison. This is a reused baseline, not a simultaneous
fresh package build. Both use the pinned release profile and lockfile.

| Package metric | Baseline | After | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| Executable | 68,657,536 B | 68,657,536 B | +0 B | Mach-O file length |
| Installed bundle | 74,695,871 B | 74,695,871 B | +0 B | Sum of bundle file lengths |
| ZIP | 48,255,390 B | 48,261,871 B | +6,481 B / +0.0134% | ditto -c -k --sequesterRsrc --keepParent |

The ZIP difference is small compression/signing noise, not a meaningful package
improvement. The local package is ad-hoc signed, not notarized. Full workspace
checking passes (1,234 tests, zero failures), including formatting, strict Clippy
and policy checks. Linux/Windows runtime behavior and a controlled live-account
soak after this new artwork change remain unmeasured.

## Gateway resume fallback — October 8, 2026

Compared baseline `2164f52e` with implementation `2ebc48a7` on Windows 11 x64,
AMD Ryzen 7 7800X3D (16 logical CPUs), 32 GB RAM, Rust 1.98.1 and the locked
dependencies. The recovery change adds one fixed-size failure counter, with no
new queue or persistence. Synthetic localhost tests now return to the original
gateway after three failed resume attempts; the baseline keeps selecting the
failed resume route. Manual recovery and successful resumes preserve their
existing behavior. These are protocol-policy checks, not live recovery timings.

Both revisions passed `cargo xtask package` with voice included, release fat LTO
and no default features. Each output directory was retained separately. Installed
bytes are the sum of all files in `dist`; archives use PowerShell
`Compress-Archive -CompressionLevel Optimal` over that directory.

| Metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Executable | 86,008,832 B | 86,010,368 B | +1,536 B (+0.0018%) |
| Full portable directory | 90,199,948 B | 90,201,484 B | +1,536 B (+0.0017%) |
| Portable ZIP | 50,002,123 B | 50,002,724 B | +601 B (+0.0012%) |

These are unsigned Windows portable packages. NSIS was unavailable, so no local
installer was produced. Both builds emitted the same OpenH264 duplicate-object
debug-info linker warning; packaging exited successfully.

The release reducer control used `cargo replay`, then one warmup and five direct
`replay-bench` runs per revision. It processed 100,000 events and retained 500
records / 331,992–332,477 estimated timeline bytes in both cases. Median times
were 159.8291 ms before and 117.5277 ms after. The five samples were
173.1020/159.8291/125.8932/152.4096/181.1192 ms and
128.2427/118.3857/106.1182/109.4493/117.5277 ms respectively. Other builds shared
the host, and the replay executables were byte-identical: this workload does not
include the changed gateway transport. The 42.3014 ms difference is not evidence
of a speed improvement. Process RSS, UI timing and live network recovery were
not measured.

## Animation frame retention — October 8, 2026

Decoded GIF and animated-avatar frames were the largest bounded RAM consumer. Each frame is held as
RGBA, so one 498x280, 40-frame GIF is about 22 MB and a 128 px, 40-frame avatar about 2.6 MB. Inline
GIFs kept their frames after scrolling away until a 128 MiB pool filled. Animated avatars and banners
kept theirs in another 128 MiB pool, although avatars only play on hover or in an open profile.

Frames not played for 5 s (scrolled away, not hovered, or shown in an unfocused window) are now
released while the still texture stays. Hover-only avatars keep frames only when they are about to
play. Released artwork asks the worker for frames again only once it can play; the encoded source comes from
the disk cache, so this costs a re-decode, not a download.

The ignored `ui` workload `animation_memory_workload` (release build, Apple M1, Rust 1.98.1, no window,
GPU, network or account) scrolls past 12 GIF embeds with 20 of 60 animated avatar rows on screen, then
settles for 6 s. Retained bytes were identical across three runs per revision (six on the change):

| Metric | Base | Change |
| --- | ---: | ---: |
| Peak retained decoded frames | 237.3 MiB | 111.7 MiB |
| Settled retained decoded frames | 237.3 MiB | 46.3 MiB |
| Peak process RSS | 265.7–265.8 MiB | 145.2–157.1 MiB (5 of 6 runs ≤ 145.4) |

Retained bytes come from the pools' own accounting. RSS did not fall after settling, because the
macOS allocator keeps freed ~0.5 MB blocks resident for reuse. Later decodes reuse them rather than
growing the process. GPU playback textures, which were also released, were not measured, and the
real app's scroll speed and media mix will differ. Standard macOS package: executable
68,526,000 B (+16,416), installed app 74,564,335 B (+16,416), `ditto` ZIP 48,183,136 B (+3,269).

## Active-call repaint cadence — October 8, 2026

A code audit of the UI, desktop wiring, client state and network/voice crates found one
always-on cost: while any call was active, `logic()` requested a repaint every 50 ms, running the
whole UI pass at 20 Hz even though speaking, notices, remote video, devices, hotkeys, screen share
and deadlines each already wake the UI themselves. The request is now a 1 s heartbeat.

The synthetic `--demo --demo-call` fixture (no credentials, no audio device, no network) was sampled
on an Apple M1 (16 GiB, macOS 27.0, Rust 1.98.1) with release `--features demo` builds of the base
commit and the change. Each sample waited 8 s, then summed process CPU time and polled RSS every
0.5 s for 30 s; base and change were alternated.

| Workload (30 s) | Base | Change |
| --- | ---: | ---: |
| Active-call fixture, CPU, 3 runs each | 6.27%, 5.77%, 6.17% | 0.50%, 0.50%, 0.50% |
| Active-call fixture, peak RSS | 126.3–126.5 MiB | 126.3–126.4 MiB |
| Plain `--demo` idle, CPU | 0.000% | 0.000% |

The 0.50% remaining is the 1 s heartbeat plus the call timer. CPU time has 10 ms resolution, so
treat the figures as approximate. They cover one fixture and display, not a live call: remote video,
screen share, audio threads and GPU work were not measured. A `footprint`/`heap` look at the idle demo
showed 68 MB physical footprint and 11.6 MB of live heap, so no idle-memory regression was found.

The same change set also avoids work that was not benchmarked, so no speedup is claimed for it:
permission decisions are no longer discarded when an event leaves the guild and channel records
equal or when an unrelated channel is removed, notification/read-state lookups use the indexed
channel map instead of a linear scan, and the composer thumbnail reads at most 64 MiB (the decode
allocation limit) instead of up to the 500 MB upload limit plus a second copy.

Standard no-default-features macOS packages built from both revisions: executable 68,509,584 B in
both, installed app 74,547,919 B in both, `ditto` ZIP 48,179,547 vs 48,179,867 B (+320 B).


## Soundboard — historical upstream evidence, October 7, 2026

The following measurements were recorded by upstream #579, archived at `aa9c1dca`.
They do **not** measure this fork integration. The original synthetic screenshots remain
in the [immutable upstream evidence directory](https://github.com/ViceVerse-cz/Serein/tree/aa9c1dca6d24920ea7ef7d3a5c0836d54a43eabd/docs/pr-evidence).

Baseline `884fb73f` plus the one-line lockfile correction that lets it build on Windows,
compared with the soundboard playback, panel and server management changes. Windows 11 Pro
10.0.26200, AMD Ryzen 7 7800X3D, 32 GB RAM, Radeon RX 9070 XT, pinned Rust 1.98.1, locked
dependencies. Both revisions were built in separate target directories.

| Metric / method | Baseline | After | Delta |
| --- | --- | --- | --- |
| `serein.exe`, `cargo xtask package` | 85,350,400 B | 85,810,688 B | +460,288 B (+0.54%) |
| Installed `dist/` tree | 89,524,013 B | 89,984,421 B | +460,408 B (+0.51%) |
| `dist/` as one deflate-9 zip | 49,631,916 B | 49,767,265 B | +135,349 B (+0.27%) |
| Idle CPU, median of 3 | 1.25% of one core | 1.30% of one core | within run-to-run noise |
| Working set after sampling, median of 3 | 122.2 MB | 122.6 MB | within run-to-run noise |
| Private bytes after sampling, median of 3 | 152.2 MB | 148.3 MB | within run-to-run noise |

The process rows sample a release build with the `demo` feature running
`--demo --demo-voice` with no input: 15 seconds of warmup, then 30 one-second samples of
total processor time, working set and private bytes. Individual runs ranged over 1.15-1.41%
CPU and 147.9-156.1 MB private bytes across both builds, so no difference is claimed. The
zip is a local measurement of the package directory, not a published artifact.

Not measured: playback mixing cost in a live call, download and decode time of a real
sound, waveform rendering frame time and trim encoding time. None of those paths run in the
idle demo; the mixer and the Ogg Opus encoder have device-free tests but no timing
workload. Bounded additions: up to eight overlapping clips of at most six seconds in the
call mixer, a session cache of at most sixteen decoded clips (about 11.5 MB), and, only
while the upload review is open, up to 57.6 MB of decoded audio plus 30 KB of peaks.

## Voice default-device polling — October 7, 2026

An offline probe compared creating a fresh PulseAudio client for every metadata poll with reusing
one client. Across five measured runs after warmup, 300 polls created 300 clients and left 300 peer
sockets connected in the fresh-client pattern; reuse created one client and left one socket. The
probe used synthetic socket pairs and a 2 ms pause per query. This identifies dependency resource
retention in the polling pattern; it does not establish application socket behavior or whole-process
memory use.

Both standard voice-enabled macOS packages were the same size: executable 68,180,992 B, installed
app 74,201,013 B, distribution 74,265,778 B. ZIP sizes differed by 37 B (48,039,907 vs 48,039,944 B),
which is packaging variation, not a runtime change. No Discord call, microphone, physical device,
callback latency, frame timing or CPU/RSS comparison was measured.

## CPU and memory audit — October 2, 2026

Release workloads compared a 100,000-event synthetic timeline, cursor scans, and synthetic video
frame ownership on an Apple M1 with Rust 1.98.1:

| Workload | Before | After | Result |
| --- | ---: | ---: | ---: |
| Reducer replay median | 153.985 ms | 53.504 ms | −65.25% |
| Full-history cursor query batch | 263.428 ms | 6.948 ms | −97.36% |
| Cursor batch with deleted tail rows | 341.132 ms | 16.075 ms | −95.29% |
| Coalesced 1080p frame peak RSS | 34.406 MiB | 26.516 MiB | −22.93% |
| Standard executable | 62,006,096 B | 62,006,112 B | +16 B |
| Installed bundle | 68,016,525 B | 68,016,541 B | +16 B |

The reducer retained 500 rows in both builds. Cursor timings isolate hot lookup work and do not
predict whole-UI gains. Frame conversion time was effectively unchanged; the lower peak RSS came
from reusing the undisplayed pending frame. With uploads every third frame, peak RSS was unchanged.

A focused native idle sample showed mean CPU of 1.180% before and 1.275% after, and settled RSS of
108.766 MiB and 111.484 MiB. This small, noisy sample supports no idle-performance improvement
claim. Startup latency, full-frame p95, GPU memory, live traffic and other platforms were unmeasured.

These workloads can be repeated with the pinned toolchain using `cargo replay` for the reducer and
the existing ignored client-core, desktop-frame and replay-soak workloads for detailed memory work.
The delivery skill documents how to compare a task baseline with the changed build. Do not compare
results from different machines or claim live-client behavior from synthetic fixtures.

## Voice participant controls (#592): Windows integration evidence - October 8, 2026

Fresh standard Windows x64 voice-enabled packages compare main `1b3e4a7b` with `d14edbb4` (measured 2026-10-08). Baseline/current file counts: 216/216.

| Metric | Main `1b3e4a7b` | Current integration | Delta |
| --- | ---: | ---: | ---: |
| Standard executable | 86,008,832 B | 86,089,216 B | +80,384 B (+0.0935%) |
| Installed directory | 90,199,948 B | 90,280,332 B | +80,384 B (+0.0891%) |
| Distribution ZIP | 50,001,798 B | 50,025,305 B | +23,507 B (+0.0470%) |

Method: `cargo xtask package`, Rust 1.98.1, standard release flags without demo; Windows 11 build 26200, Ryzen 7 7800X3D, 32 GiB RAM. Runtime workspace artifacts were invalidated before each feature build. Installed bytes sum every file in `dist`; ZIP uses whole-directory .NET Optimal compression. NSIS was unavailable, so no installer executable was built.

Alternating synthetic reducer replay (100,000 events): baseline median 76.5049 ms, range 74.0754-79.5180 ms; this change median 79.6071 ms, range 66.3717-83.4725 ms; median delta +3.1022 ms (+4.05%). One warmup each, then five baseline/controls/inbox rounds. All measured runs retained 500 records and 331,992-332,477 estimated timeline bytes (limit 4 MiB). The binaries were retained separately and workspace reducer artifacts rebuilt between variants. Concurrent compilation remained uncontrolled noise; overlapping ranges establish neither a performance improvement nor a regression.

Current native CPU, memory, frame/startup latency and affected-device behavior remain unmeasured because the native automation bridge is unavailable. Package size and synthetic reducer timing do not establish live Discord performance.

## Window geometry minimum — October 7, 2026

Baseline `c5e50e77` and fixed `c5ace4dd` were built on Windows 11 Home build
26200, Ryzen 7 7800X3D, 32 GiB RAM, Rust 1.98.1. Both standard voice-enabled
packages used `cargo xtask package` with no default or demo features. The installed
size sums all 216 files in `dist`; .NET `ZipFile.CreateFromDirectory` compressed
each complete directory with the same default settings. NSIS was unavailable, so
these measurements cover the package directory and ZIP, not an installer binary.

| Metric | Baseline | Fixed | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 85,922,304 B | 85,922,304 B | 0 B |
| Installed package | 90,113,420 B | 90,113,420 B | 0 B |
| Distribution ZIP | 49,970,177 B | 49,970,277 B | +100 B (+0.00020%) |

For native idle samples, each source was also built with
`cargo build --release --locked -p serein --no-default-features --features demo`
and run with `--demo`.
Both runs used the same 1120×760 synthetic fixture, Windows DPI 120 (125% scale),
the default DX12 renderer and no child processes. After a 15-second warmup,
`Get-Process` sampled cumulative CPU time and private bytes every second for ten
seconds, with no further interaction. CPU is the mean one-core percentage; settled
private memory is the median of the final five readings.

| Native demo metric | Baseline | Fixed | Delta |
| --- | ---: | ---: | ---: |
| Idle CPU, 10 samples | 0% | 0% | Below sample resolution |
| Peak private memory | 192,835,584 B | 192,643,072 B | -192,512 B (-0.10%) |
| Settled private memory | 192,835,584 B | 192,643,072 B | -192,512 B (-0.10%) |

The tiny ZIP and memory differences do not establish a performance improvement.
Startup latency and frame timing were not measured. Demo ignores persisted geometry;
the offline window-geometry check covers restoration of undersized saved values.

## Window minimum and restoration (PR #583): Windows integration evidence - October 8, 2026

Fresh standard Windows x64 voice-enabled packages compare main `1b3e4a7b` with `371632f1` (measured 2026-10-08). Baseline/current file counts: 216/216.

| Metric | Main `1b3e4a7b` | Current integration | Delta |
| --- | ---: | ---: | ---: |
| Standard executable | 86,008,832 B | 86,009,344 B | +512 B (+0.0006%) |
| Installed directory | 90,199,948 B | 90,200,460 B | +512 B (+0.0006%) |
| Distribution ZIP | 50,001,798 B | 50,002,093 B | +295 B (+0.0006%) |

Method: `cargo xtask package`, Rust 1.98.1, standard release flags without demo; Windows 11 build 26200, Ryzen 7 7800X3D, 32 GiB RAM. Runtime workspace artifacts were invalidated before each feature build. Installed bytes sum every file in `dist`; ZIP uses whole-directory .NET Optimal compression. NSIS was unavailable, so no installer executable was built.

Current native CPU, memory, frame/startup latency and affected-device behavior remain unmeasured because the native automation bridge is unavailable. Package size and synthetic reducer timing do not establish live Discord performance.

### Window minimum after monitor changes — review follow-up (2026-10-08)

Fresh standard Windows x64 voice-enabled packages compare `666d7d4a` with this
review fix. Both use Rust 1.98.1, the pinned lockfile, standard release flags
without demo, Windows 11 build 26200, Ryzen 7 7800X3D and 32 GiB RAM. Each package
contains 216 files. Installed bytes sum all files in `dist`; ZIP uses the complete
directory with .NET `ZipFile.CreateFromDirectory`, Optimal compression.

| Metric | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Standard executable | 86,009,344 B | 86,011,904 B | +2,560 B (+0.0030%) |
| Installed directory | 90,200,460 B | 90,203,020 B | +2,560 B (+0.0028%) |
| Distribution ZIP | 50,002,090 B | 50,003,472 B | +1,382 B (+0.0028%) |

Both `cargo xtask package` runs passed; NSIS is unavailable, so no installer
executable was produced. The final `cargo xtask check` passed formatting, strict
workspace Clippy, workspace tests, demo compilation and policy checks. Four
focused application-settings tests passed, including small displays, scale and
decoration bounds for the recalculated minimum.

Native capture/interaction remains unavailable (the native helper reports a
missing pipe). Moving a real window between monitors, native CPU/memory, startup
and frame latency were not measured. These package measurements and synthetic
tests do not establish native multi-monitor or live Discord behavior.
## Watched-stream recovery — October 8, 2026

Standard Windows x64 voice-enabled packages (`cargo xtask package`, no demo feature,
Rust 1.98.1) compared baseline `1b3e4a7b` and fixed runtime `5b446a45` on Windows 11
build 26200, Ryzen 7 7800X3D, 32 GiB RAM. Executable size stayed 86,008,832 B;
the 216-file installed directory stayed 90,199,948 B. Whole-directory .NET Optimal
ZIP size changed from 50,001,798 B to 50,001,523 B (-275 B), packaging variation.
NSIS was unavailable, so no installer binary was built. Native CPU/memory/frame
measurements and live stream continuity remain unmeasured because the native
automation bridge is unavailable. The lifecycle regression checks retained worker
ownership during recovery; it does not measure network quality or throughput.

## Channel and message pills — October 7, 2026

Compared baseline `38d919d7` with runtime `78de8d15` on Windows 11 Home
10.0.26200, Ryzen 7 7800X3D (16 logical CPUs), 33,410,678,784 bytes RAM,
Rust 1.98.1. Both use the same new synthetic channel-links fixture and the
same Windows allocator lockfile correction, required to compile the baseline.
The release preview uses WGPU with the demo feature and 125% display scale;
the selected GPU/backend was not logged.

After five seconds warmup, 21 process-counter samples were taken about one
second apart per build. This was a static idle fixture, with no scripted input.
Baseline elapsed sample time was 20.536 s and after was 20.252 s. Settled memory
is the mean of the final five samples.

| Metric | Baseline | After | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| CPU, percentage of one core | 0.00% | 0.31% | +0.31 percentage points | Windows process CPU delta / actual elapsed time |
| Peak / settled working set | 196,276,224 B | 198,307,840 B | +2,031,616 B / +1.04% | Windows WorkingSet64; maximum / mean of last five samples |
| Settled private bytes | 414,261,248 B | 415,698,944 B | +1,437,696 B / +0.35% | Windows PrivateMemorySize64; mean of last five samples |

One series per build on a shared machine with compiler activity is observational;
it supports no performance improvement claim. Native frame/startup latency and
live-service performance were not measured. Both standard voice-inclusive release
packages passed, without demo/developer features:

| Package metric | Baseline | After | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| Executable | 85,449,728 B | 85,464,064 B | +14,336 B / +0.017% | serein.exe file length |
| Installed files | 89,623,341 B | 89,637,677 B | +14,336 B / +0.016% | Sum of all dist file lengths |
| ZIP distribution | 49,775,655 B | 49,781,438 B | +5,783 B / +0.012% | Compress-Archive, Optimal; ZIP file length |

Installed bytes sum all files in `dist`; ZIPs use PowerShell `Compress-Archive`
with Optimal compression. NSIS was unavailable, so no installer executable was
generated locally. Both unsigned packages emitted the existing OpenH264
duplicate-object debug-info linker warning.

Inspected dark/wide and light/narrow images are eframe framebuffer exports. The
native computer-use helper was unavailable, so OS screenshots, native input and
screen-reader operation remain unverified. Workspace checking reached 1,154
passing tests, eight failures also reproduced on the baseline, and 23 ignored
tests. Seven focused pill tests and strict Clippy passed after the final visual
correction. Commands, raw counters, baseline failure names and image provenance
are in [the evidence directory](pr-evidence/channel-message-pills/README.md).

### Review follow-up package

The standard voice-inclusive package was rebuilt at `7d9c0d05` after the review
fixes, using the same Windows host, pinned toolchain, no demo/developer features,
and compression method. It passed; NSIS remains unavailable locally. Comparison
with the same `38d919d7` baseline:

| Package metric | Baseline | After review | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| Executable | 85,449,728 B | 85,464,576 B | +14,848 B / +0.017% | serein.exe file length |
| Installed files | 89,623,341 B | 89,638,189 B | +14,848 B / +0.017% | Sum of all dist file lengths |
| ZIP distribution | 49,775,655 B | 49,781,661 B | +6,006 B / +0.012% | Compress-Archive, Optimal; ZIP file length |

Raw sizes and the executable hash are in
[review-package.json](pr-evidence/channel-message-pills/review-package.json).
The process samples and screenshots above remain evidence for `78de8d15`; they
were not repeated for the review fixes and support no claim about changed CPU
or memory usage. Full workspace checking at `f9e568c9` reached 1,157 passes,
the same eight baseline failures, and 23 ignored tests. After the final Chinese
label correction, all ten focused pill tests, formatting and strict workspace
Clippy passed. Native input/capture remain unverified; merge blockers persist.

### Reply message-link preview follow-up (2026-10-08)

Compared `d2299412` with the reply-preview fix, using the new synthetic
`channel-link-replies` fixture in both builds. The Windows host, WGPU renderer,
125% scale and five-second warmup match the previous section. Each build has one
series of 21 process-counter samples over about 20 seconds. Baseline and changed
preview executables were rebuilt and preserved separately. Concurrent builds
and OS residency add noise; these idle results are not a performance improvement
claim and do not measure frame latency or live Discord behavior.

| Metric | Baseline | After | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| CPU, one-core percentage | 0.692% | 0.000% | -0.692 percentage points | Process CPU delta / elapsed time |
| Peak working set | 200,089,600 B | 213,258,240 B | +13,168,640 B / +6.58% | WorkingSet64 maximum |
| Settled working set | 200,089,600 B | 213,257,421 B | +13,167,821 B / +6.58% | WorkingSet64 mean of last five samples |
| Settled private bytes | 416,260,096 B | 416,219,136 B | -40,960 B / -0.010% | PrivateMemorySize64 mean of last five samples |

Working set increased while private bytes were approximately flat. Raw samples,
the inspected light/dark framebuffer pairs and reproduction details are in
[the reply evidence](pr-evidence/channel-message-pills/README.md#reply-message-links-2026-10-08).
Native input and OS screenshot capture remain unavailable; a successful
framebuffer export does not establish them.

The fresh standard voice-inclusive package passes without demo/developer
features. Compared with the historical `7d9c0d05` package, executable and
installed payload grow by 512 B; ZIP grows by 214 B. The runtime source at
`7d9c0d05` is identical to pre-fix `d2299412` (the intervening changes are four
documentation files), but this is not a fresh simultaneous package baseline.

| Package metric | Historical baseline | Reply fix | Delta | Method |
| --- | ---: | ---: | ---: | --- |
| Executable | 85,464,576 B | 85,465,088 B | +512 B / +0.0006% | serein.exe file length |
| Installed files | 89,638,189 B | 89,638,701 B | +512 B / +0.0006% | Sum of dist file lengths |
| ZIP distribution | 49,781,661 B | 49,781,875 B | +214 B / +0.0004% | Compress-Archive, Optimal; ZIP file length |

[reply-package.json](pr-evidence/channel-message-pills/reply-package.json) records
the new measurements and executable hash. NSIS remains unavailable; this is an
unsigned directory and ZIP. Final serial workspace checking reached 1,158
passes, eight recorded baseline UI failures and 23 ignored tests. Formatting,
strict workspace Clippy and the independent policy check passed.

## Source-resolution regression checks — October 12, 2026

Baseline `574a8d2` and the source/aspect-ratio implementation use pinned Rust
1.98.1, the committed lockfile and separate release target directories. One warmup
per binary precedes five alternating measured runs of the 100,000-event synthetic
reducer. The release replay executables are byte-for-byte identical (SHA-256
`0d6060167876cc35dba30a368e3c36504420c869f42d8f3a4faa6d66c6491035`): screen
settings are outside this reducer workload.

| Metric / method | Baseline | Source implementation | Interpretation |
| --- | ---: | ---: | --- |
| Release replay median, five alternating pairs | 161.129 ms | 158.551 ms | Overlapping ranges; no performance claim |
| Retained timeline estimate / records | 331,992–332,477 B / 500 | 331,992–332,477 B / 500 | Unchanged reducer limits |

The 120-second offline lifecycle soak completes 1,326 passes, 42,432 channel
visits, 25,459,200 live inserts and six logout cycles in 120.072 seconds. It does
not measure media throughput, process/GPU memory or live Discord compatibility.

Source dimensions retain the existing 33,177,600-pixel and raw-byte envelope in
either orientation. Encoder bitrates interpolate between the existing preset
pixel-count/rate points and retain the 50 Mbit/s cap. Preset tests cover common
landscape, ultrawide, square, portrait and custom ratios; pending selection and
resize tests exercise synthetic RTP/DAVE transport.

Current native UI/display sampling is unavailable in this cloud session: no
native display session or window-capture tool. macOS now captures physical native
pixels before shared scaling, so default-preset capture CPU and memory need native
comparison; bounded worst-case allocation is unchanged. Current application and
clean/cached package size checks belong to the CI run for the delivered head.
Earlier package measurements above retain their historical revision labels.

## CodeRabbit audio review follow-up — October 12, 2026

Baseline `a65238d` and the review fixes use separately compiled native helpers
with `cc -O2`, the same private FFmpeg prefix and Linux host. Each helper feeds
5,000 stereo planar-float frames, 1,024 samples at 48 kHz, through the actual
attachment resampling boundary. One warmup precedes five alternating pairs.
These wall-clock samples include process startup and concurrent Rust build load;
their overlapping ranges do not establish a throughput regression or improvement.

| Metric / method | Baseline | Review fixes | Interpretation |
| --- | ---: | ---: | --- |
| Native helper median, five pairs | 11.364 ms | 12.396 ms | +1.031 ms; overlapping ranges |
| Native helper range | 9.067–19.189 ms | 11.642–13.683 ms | Startup/scheduler noise included |
| Native helper executable | 34,432 B | 34,816 B | +384 B; not an application/package measurement |

The workload checks stable-format PCM timing. Separate native regressions cover
stereo-to-mono, planar-to-packed, same-count channel-layout changes, rate changes,
invalid metadata, output guards and reset cleanup. Resampler configuration is
recreated only when input layout, sample format or rate changes. These helpers exclude demuxing,
decoding, audio devices, GUI and live Discord. Current native display sampling
is unavailable here; application and clean/cached package checks run in CI for
the delivered revision. Previous package measurements retain their original heads.

## Upstream media integration checks — October 13, 2026

The merge candidate combines video head `aa8a09a` with parent `61337f3` using
Rust 1.98.1 and the committed lockfile. Workspace formatting, strict Clippy,
tests, the minimal application build and policy pass. Exact 6144×2560 and PQ/HLG
RTP/DAVE regressions, native/HDR resource switching, the offscreen production
media shader and workspace/fuzz license checks pass. Native GPU/display and
Discord interoperability checks remain separate owner-controlled work.

The preserved `5b8ae38` release replay has unchanged reducer source through
`aa8a09a`; its compiler, profile and rustflags fingerprints match the merge
build. One warmup precedes five alternating pairs on the same Linux cloud host.
Scheduling noise remains; this synthetic reducer excludes media, UI and RSS.
The host is Linux 6.18.44/x86_64, Xeon Platinum 8573C, with a 16 GiB memory
limit. Offline delivery uses a Debian trixie container limited to four CPUs and
14 GiB; release settings retain fat LTO, one codegen unit and line-table debug
information. No native display session was available.

| Metric / method | Pre-sync replay | Merge candidate | Delta |
| --- | ---: | ---: | ---: |
| 100,000-event median, five pairs | 104.499 ms | 119.017 ms | +14.517 ms / +13.9% |
| Run range | 99.541–109.792 ms | 111.264–131.862 ms | Slower in all five pairs |
| Retained estimate, 500 messages | 331,992–332,477 B | 393,492–393,977 B | +61,500 B; not RSS |

The cause is not isolated by these measurements. The 120-second lifecycle soak
passes 1,923 cycles, 61,536 channel visits, 36,921,600 inserts and eight logout
cycles in 120.029 seconds, retaining bounded active/dormant histories. Navigation
and the 20,001-channel workload also pass.

Normal `cargo xtask package --format dir` builds the standard release without
demo/developer features and stages into an empty destination. Verification found
an omitted notification-sound provenance file; the allowlist was corrected and
the complete command rerun from an empty destination with caches. No payload was
manually repaired. The executable remained byte-identical across these runs.

| Linux package metric | Verified candidate | Method |
| --- | ---: | --- |
| Release executable | 87,856,520 B | Compiled, staged and installed hashes match |
| Installed payload | 132,622,544 B / 266 files | Sum of installed regular-file lengths |
| Compressed directory | 86,126,968 B | Python tarfile, gzip level 9 |

Executable SHA-256 is `eb703b348cdc54a9db0d616e340aa88eabe7309a80d81520c266faa17b712fd3`.
All six private libraries, corresponding-source files and notices match their
staging inputs; ELF runtime paths are relative and exclude the development FFmpeg
prefix. There is no matching `aa8a09a` package baseline, so no package delta is
claimed. Native distro package/runtime closure, Windows/macOS packages, display
transitions, playback throughput and settled process/GPU memory require CI or
native hardware; directory staging alone does not establish them.

## HDR capture/SPS review fixes — October 13, 2026

The `c47036c` baseline package was verified by executable hash before source
edits and preserved separately. The changed package measures the reviewed source
committed with this entry; the PR records its exact revision. Both use Rust
1.98.1, the committed lockfile and the same Linux 6.18.44/x86_64 cloud host
(Xeon Platinum 8573C, 16 GiB host limit), with a Debian trixie build container
limited to four CPUs and 14 GiB. Standard release includes voice and excludes
demo/developer features: fat LTO, one codegen unit, line-table debug information
and symbol stripping.

`cargo xtask package --format dir` starts with an empty output directory after
rebuilding xtask. Compiled, staged and installed executable hashes match; all
six private libraries, corresponding-source files and notices match their
staging inputs. ELF runtime paths are relative and omit the development FFmpeg
prefix. There is no manual staging repair. One package per source revision is
measured by file lengths; warmup is not applicable. Both compressed directories
use Python tarfile with gzip level 9.

| Linux package metric / method | Baseline `c47036c` | Review fixes | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 87,856,520 B | 87,856,648 B | +128 B |
| Installed payload | 132,622,544 B | 132,622,672 B | +128 B |
| Compressed directory | 86,126,968 B | 86,126,955 B | -13 B |
| Installed regular files | 266 | 266 | +0 |

Changed executable SHA-256: `83e7f472230cab8fd369e83d42e9d69cc12fd6afaf453638428ecde381c9cac5`.

Workspace checking passes formatting, strict Clippy, 1,420 tests (31 ignored),
the minimal application build and policy. Login-handoff, cached-xtask workspace,
application/fuzz license checks and packaging fixtures pass. The actual Linux
capture pipeline retains P010/BT.2020/PQ through cropping and HDR→SDR→HDR
transitions. Its crop element requires planar 10-bit input, adding format
conversion work; no throughput or CPU improvement is claimed.

The fresh 120-second offline soak completes 1,488 passes, 47,616 channel visits,
28,569,600 inserts and six logout cycles in 120.043 seconds. This core workload
excludes capture, decoding, GUI and process/GPU memory. The historical +13.9%
reducer slowdown above retains its original revisions and unisolated cause;
it is not a fresh measurement of these media fixes.

Native display/window capture and suitable devices are unavailable here, so
media throughput, CPU and peak/settled process/GPU memory remain unmeasured.
Directory staging does not establish native application launch, full distro
runtime closure, physical HDR behavior or Discord interoperability. Current-head
Windows/macOS/Linux application/package CI and owner-controlled hardware checks
are reported separately in the PR. The retained settings screenshot pair is
historical evidence from `b1e3474`, predating the current control placement and
Source/8K additions.

## Current nightly comparison — October 13, 2026

This comparison supersedes the older baselines in the PR description. Its
baseline is the latest published upstream nightly,
[`v1.0.0-nightly.20261010.57`](https://github.com/ViceVerse-cz/Serein/releases/tag/v1.0.0-nightly.20261010.57),
source `cc0987e2d4505bdfb780045327bb14c3b0c0928e`; the changed source is
`4ad106c2348d5280a4cdf2ba840c2872aca14675`. These are matching local Linux
source rebuilds, using each revision's committed lockfile, Rust 1.98.1 and
standard voice-inclusive release without demo/developer features. Both profiles
use fat LTO, one codegen unit, line-table debug information and symbol stripping.
The Linux 6.18.44/x86_64 host is a Xeon Platinum 8573C with a 16 GiB limit;
the Debian trixie build/measurement container has four CPUs and a 14 GiB limit.
Separate worktrees and Cargo targets prevent reuse of workspace artifacts. The
existing PR package was hash-verified before reuse. These measurements compare
source rebuilds, not the published release's distribution archives.

| Metric / method | Nightly `cc0987e` | PR `4ad106c` | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 87,185,832 B | 87,856,648 B | +670,816 B / +0.8% |
| Installed payload | 91,717,776 B | 132,622,672 B | +40,904,896 B / +44.6% |
| Compressed directory, gzip 9 | 53,094,741 B | 86,126,955 B | +33,032,214 B / +62.2% |
| Installed regular files | 228 | 266 | +38 |
| 100,000-event replay median, five pairs | 111.294 ms | 117.525 ms | +6.231 ms / +5.6% |
| Retained estimate, 500 messages | 331,992–332,477 B | 393,492–393,977 B | +61,500 B; not RSS |

Both packages use normal `cargo xtask package --format dir` from an empty
output directory. Compiled, staged and installed executable hashes match. File
lengths measure executable and installed payload sizes; Python tarfile with gzip
level 9 and the same archive basename measures both compressed directories.
There is one package per source revision and no warmup for file-size counters.
Most of the payload increase comes from bundled media libraries and their
corresponding-source archives. Nightly executable SHA-256:
`888ce8ddb9180be739f307b09c059aabd21923b0f39a5476ffb520de3e63d2c2`;
PR executable SHA-256:
`83e7f472230cab8fd369e83d42e9d69cc12fd6afaf453638428ecde381c9cac5`.

The release replay program is identical at both revisions. One warmup per
binary precedes five alternating pairs on the same host after compilation has
finished. Each sample processes 100,000 synthetic events and retains 500
messages. The nightly runs span 107.809–196.299 ms; PR runs span
111.828–147.282 ms. The PR is slower in four of five pairs, but the ranges
overlap and include scheduling noise. The measured median slowdown is not
attributed to a particular feature.

The comparison includes upstream changes made after the nightly, as well as
the retained video work; it does not isolate individual features. Replay
measures the core reducer, not media throughput, UI latency or process RSS.
Native media CPU/throughput, peak/settled process/GPU memory and physical HDR/
Discord interoperability remain unmeasured here. Older comparison sections
retain their original revisions and are historical evidence.

## Native Windows/macOS nightly comparison — October 13, 2026

Native CI source rebuilds compare the same published nightly
[`v1.0.0-nightly.20261010.57`](https://github.com/ViceVerse-cz/Serein/releases/tag/v1.0.0-nightly.20261010.57),
`cc0987e2d4505bdfb780045327bb14c3b0c0928e`, against PR source
`8a1ab4a2014c6660c86f8009407f396c36571480`. The latter differs from the
Linux measurement's `4ad106c` only in this performance document. These are
matching source rebuilds, not byte-identical copies of the published signed
release archives. Each OS uses its own hosted runner for both sides, with
separate clean checkouts and fresh Cargo targets, Rust 1.98.1 and each source's
committed lockfile. Standard release includes voice and excludes demo/developer
features; both profiles use fat LTO, one codegen unit, line-table debug
information and symbol stripping. No desktop, owner account, microphone or
live capture session is launched.

### Windows X64

Environment: Microsoft Windows Server 2025 Datacenter build 26100; AMD EPYC 7763 64-Core Processor; 16.0 GiB RAM, 4 logical CPUs. Runner image `win25-vs2026` / `20260925.250.1`.

| Metric / method | Nightly `cc0987e` | PR `8a1ab4a` | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 86,203,392 B | 87,188,992 B | +985,600 B / +1.1% |
| Installed payload | 90,394,509 B | 137,153,849 B | +46,759,340 B / +51.7% |
| Compressed distribution ZIP | 50,912,221 B | 83,622,631 B | +32,710,410 B / +64.2% |
| Unsigned NSIS installer | 42,992,192 B | 73,600,334 B | +30,608,142 B / +71.2% |
| Installed regular files | 216 | 248 | +32 |
| 100,000-event replay median, five pairs | 105.887 ms | 117.777 ms | +11.890 ms / +11.2% |
| Retained estimate, 500 messages | 331,992–332,477 B | 393,492–393,977 B | +61,500 B; not RSS |

Replay ranges: nightly 104.890–107.109 ms; PR 116.095–119.153 ms. Ranges do not overlap in this sample; the cause of the measured difference is not isolated.

[Native comparison job](https://github.com/MCShotty/Serein/actions/runs/38408007242/job/115332603702) preserves the exact JSON results. Packaged executable SHA-256: nightly `feb9c6a7c4e2267d868dad376e30beb0f6628d4545775927c74d28cf1196f097`; PR `c99607af30ba96b2f2eb26fa28d66fe9818ff0d3bdca0a6aa47ba2eeefc7e3f8`.

Measured replay samples in pair order (ms): nightly `104.890, 105.887, 106.635, 105.681, 107.109`; PR `118.033, 117.411, 119.153, 116.095, 117.777`.

### macOS ARM64

Environment: macOS 26.6.2 build 25G83; Apple M1 (Virtual); 7.0 GiB RAM, 3 logical CPUs. Runner image `macos26` / `20260907.0351.1`.

| Metric / method | Nightly `cc0987e` | PR `8a1ab4a` | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 68,638,560 B | 69,091,776 B | +453,216 B / +0.7% |
| Installed payload | 74,603,532 B | 95,585,421 B | +20,981,889 B / +28.1% |
| Compressed distribution ZIP | 48,052,367 B | 64,506,402 B | +16,454,035 B / +34.2% |
| Installed regular files | 223 | 248 | +25 |
| 100,000-event replay median, five pairs | 63.177 ms | 70.793 ms | +7.616 ms / +12.1% |
| Retained estimate, 500 messages | 331,992–332,477 B | 393,492–393,977 B | +61,500 B; not RSS |

Replay ranges: nightly 62.051–66.883 ms; PR 66.162–83.932 ms. Ranges overlap; scheduling noise prevents attributing the median difference to a feature.

[Native comparison job](https://github.com/MCShotty/Serein/actions/runs/38406359420/job/115327118696) preserves the exact JSON results. Packaged executable SHA-256: nightly `667a0fb5493ebdd349aa62cc74675658fcb29828dda639ce2d4df84e55268fb5`; PR `715c06bb6aff0df6307caf97fd565809dd4a7a73563f484753c5c501b4e858bd`.

Measured replay samples in pair order (ms): nightly `63.067, 64.794, 62.051, 66.883, 63.177`; PR `66.162, 67.295, 83.932, 82.592, 70.793`.

The [measurement helper](https://github.com/MCShotty/Serein/blob/6bad96ebcf66faf8c010a25b15368d98c6d181a9/scripts/measure-native-nightly.py) records the source revisions, build profile, environment, hashes and every replay sample.

Both sides run normal `cargo xtask package` after rebuilding xtask and start
with empty package output. Windows compiled/staged executable hashes match;
normal macOS packaging ad-hoc signs the staged copy, so its hash is recorded
separately and the bundle passes `codesign --verify --strict`. Windows ZIPs
use PowerShell/.NET Optimal compression; macOS ZIPs use
`ditto -c -k --sequesterRsrc`, matching each platform's release archive method.
Every archive passes a CRC check. One package per source is measured by file
lengths; size counters have no warmup. Windows installers are unsigned, and
macOS bundles are not Developer ID signed or notarized.

The identical release replay program receives one warmup per binary and five
alternating pairs after all compilation and compression finish. It measures
100,000 synthetic reducer events, not media throughput, UI latency or process
RSS. These comparisons include post-nightly upstream changes alongside the
video work and do not isolate individual features. Native encoding/playback
FPS, media CPU use, peak/settled process/GPU memory and physical HDR/Discord
interoperability remain unmeasured. Earlier Windows measurements above retain
their historical revision labels and are not reused as current results.

## Video encoding evidence for #567

The [compact evidence record](pr-evidence/video-encoding/README.md) retains
matched synthetic native captures, narrow/light camera and screen layouts, and
[unchanged historical samples](https://github.com/MCShotty/Serein/blob/918a4abe391a9e1619ba678af2b3f204f594cf6e/docs/pr-evidence/video-encoding/measurements.json).
Older helpers, logs and detailed build records remain in the
[immutable archive](https://github.com/MCShotty/Serein/tree/faa9660761a2099cf2e6f5b748cf48bfa6871b63/docs/pr-evidence).
The archive commit identifies storage; each record retains its measured revision,
working-tree source hashes, environment, method and limitations. None of these
historical results validates the latest extracted PR head.

### Windows release package — October 8, 2026

Matched voice-inclusive releases compare `1b3e4a7b` with `eefa0fe5`, using Rust
1.98.1 on Windows 11 build 26200, Ryzen 7 7800X3D and 32 GiB RAM, without demo.
Installed size sums every file in `dist`; ZIPs use .NET's Optimal compression.

| Metric | Baseline | FFmpeg integration | Delta |
| --- | ---: | ---: | ---: |
| Release executable | 86,008,832 B | 86,554,112 B | +545,280 B |
| Installed package | 90,199,948 B | 124,070,028 B | +33,870,080 B |
| Complete ZIP | 50,001,798 B | 78,054,030 B | +28,052,232 B |

The historical package reused a stale xtask that omitted FFmpeg staging. Its
output was repaired by replaying `packaging/ffmpeg/bundle.py`; all 241
non-executable files then matched their expected staging sources, including
DLLs, corresponding source, recipes and notices. The complete archive passed
integrity checks. **No clean end-to-end package rerun is claimed.** These are
portable directory/ZIP sizes, not NSIS installer sizes. The
[original method](https://github.com/MCShotty/Serein/blob/faa9660761a2099cf2e6f5b748cf48bfa6871b63/docs/performance.md#windows-ffmpeg-integration-package---october-8-2026)
retains the exact executable identity and checks.

### Linux synthetic component and UI results — October 5–9, 2026

These are separate additions and workloads, not one whole-PR comparison.
Original record names in the consolidated JSON identify the source of each row.

| Historical workload | Baseline | After | Interpretation |
| --- | ---: | ---: | --- |
| Software FFmpeg camera, 300 frames | 993.070 ms | 1,024.365 ms | Five measured alternating pairs; overlapping ranges |
| Driver detection, fresh processes | 262.167 ms | 265.354 ms | Five measured pairs; no GPU drivers available |
| 8K controls, sampled peak/settled helper RSS | 162,140,160 B | 163,909,632 B | One helper pair, +1,769,472 B; no encoding/capture |
| Split-option setup, 60,000 contexts | 1,301.148 ms | 1,371.227 ms | Five measured pairs; codecs/drivers never opened |

The camera fixture warms 30 frames before timing 300 deterministic 640×480 RGB
frames; one process warmup precedes five measured pairs. Driver detection uses
one warmup and five alternating processes against real detector/native code;
all nine NVENC/AMF/QSV codec paths are unavailable on the recorded host. The 8K
helper warms six seconds, receives 100 wheel events and records fifteen
one-second CPU/RSS samples. Split setup uses matched optimized diagnostic
FFmpeg builds that differ from the standard distribution recipe. Full OS,
CPU/RAM, renderer, compiler, source identities, commands, samples and noise
limits are retained in the linked records and stage methods.

These workloads do not establish general speed/quality improvements. Helper
sizes are not package sizes; sampled RSS is not a memory ceiling. Physical
8K/60 fps, frame/startup p95, GPU memory, active hardware throughput and fresh
whole-app/package comparisons remain unmeasured in these historical records.
Current implementation checks are recorded below separately from these measurements.

### Shared ultrawide/HDR media — October 11, 2026

The baseline is `c4bc650`, with its unchanged normal FFmpeg recipe in a detached
worktree. The new shared-media recipe is built with NVENC, AMF and QSV enabled,
H.264/HEVC/AV1 decoding and dav1d 1.5.1. Each revision uses its own Cargo release
directory, pinned Rust 1.98.1 and the standard voice-inclusive, no-development-data
package command. No historical executable or manually repaired staging tree is
used for this comparison.

`cargo xtask check` passes in an offline Linux container using the build host's
native libraries, CA certificates and a private temporary directory. The container
avoids the managed sandbox's nonstandard root-directory ownership, which the
unchanged IPC permission test correctly rejects. Formatting, strict Clippy,
workspace tests and policy checks pass. Login-handoff and the six license-policy
fixtures pass. Locked application/fuzz license graphs, native query/preset/packet
fixtures, source-patch reproduction and packaging fixtures were checked separately.

The actual media shader passes offscreen software-Vulkan tests for SDR, PQ/HLG
highlights, rotation, odd-crop chroma, resource cleanup and device recreation.
The exact 6144×2560 picture is decoded through synthetic RTP/DAVE at 60 fps
timestamp cadence. Four HEVC/AV1 PQ/HLG transport fixtures and HDR attachment
audio/seek/cancellation fixtures pass. These establish picture/color/lifecycle
behavior, not sustained 60 fps throughput or live Discord compatibility.

Fresh measurements compare `c4bc650` with `53cd786` on Linux x86-64, Intel Xeon
Platinum 8573C, with a four-CPU quota and 16 GiB memory limit. Each uses its own
release target directory. The baseline normal `cargo xtask package --format dir`
completed from empty output without a staging repair: executable 87,744,200 bytes,
installed tree 125,169,707 bytes, and canonical sorted `gzip -n -6` distribution
82,836,259 bytes. Both include voice and corresponding-source/license material.

| Synthetic release replay | Baseline | Shared media | Method |
| --- | ---: | ---: | --- |
| 100,000-event median | 97.741 ms | 96.144 ms | One warmup, five alternating pairs after builds settled |
| Observed elapsed range | 80.842–113.114 ms | 88.715–106.501 ms | Overlapping samples; no improvement claimed |
| Retained timeline | 331,992–332,477 bytes / 500 records | Same | Reducer accounting, not RSS |

The replay executables have identical `.text` hashes; earlier samples taken
during compilation were noisy and are excluded from this comparison. The new
120-second lifecycle soak completes 2,145 passes / 68,640 channel visits and
41,184,000 inserts within the shipping row/byte limits, with nine logout cycles.
It uses no network, storage, renderer or audio devices and does not measure media
throughput or process memory.

Clean and cached current-package measurements and native CI results are reported
in [#567's Performance and Verification sections](https://github.com/ViceVerse-cz/Serein/pull/567).
CPU, frame-delivery rate and peak/settled memory during physical HDR playback remain
unmeasured here. Native window capture and a suitable HDR display are unavailable
in this environment. The owner has agreed to test the current CI Windows installer
with the 6144×2560 stream and HDR/SDR display transitions. Outgoing HDR captures
currently tone-map to SDR until receiver 10-bit compatibility is established.

## Attachment picker and draft removal — October 10, 2026

Starting from `b6d608bb`, with upstream `0d9c6772` merged, four offline regressions
fail before the attachment fixes and pass afterwards. The complete upload module's
15 tests and one headless tray/card interaction fixture pass. Current upload,
model/core/session/protocol/API and picker/HEIC source is rebuilt against cached
external dependencies; the tray fixture extracts current methods and uses cached
UI helpers. Preview cancellation returns its job/byte permits and preserves the
previous upload; no queue, resource budget or persistence is added. These are
scoped correctness/resource checks, not full application or performance evidence.
Current release package sizes, native screenshots, CPU/RSS and frame latency are
unmeasured: the shared Cargo build-cache lock is not writable, disk space prevents
a fresh full cache, and native window capture tools are unavailable.

## Runtime resource cleanup — October 8, 2026

Historical evidence from the original combined PR, separated by scope. The recorded revision labels, hashes and measurements are unchanged; these results do not validate the new branch heads.

Against `98158b4`, queued updater results retained staged directories or inert helpers after app closure. Results now retain cleanup ownership until accepted. The remaining staged-directory count changes from 1 to 0; the helper exits within the fixture’s three-second bound. Registered-game normalization also preserves its 256-byte ceiling. See [the resource evidence](pr-evidence/updater-resource-hunt/README.md). Full executable/package size, CPU/RSS and native integration remain unmeasured.
## Repository-wide lifecycle and state bug hunt — October 5, 2026

Compare the verified local starting commit `b6c32b6ddfa3bef63f01bc9b101de5aa20203017`
with the repository-wide bug-hunt fixes. The baseline and after binaries were built
from isolated worktrees with locked dependencies. Testing used synthetic offline data
and did not require an account, physical capture device or GPU. Raw samples, artifact
identities and environment details are in the
[bug-hunt measurements](pr-evidence/repository-bug-hunt/measurements.json).

| Metric / method | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Standard executable, bytes | 85,746,216 | 85,796,648 | 50,432 / +0.059% |
| Installed regular files, bytes | 120,976,132 | 121,026,564 | 50,432 / +0.042% |
| Compressed DEB, bytes | 70,795,724 | 70,809,936 | 14,212 / +0.020% |
| Reducer replay median, 100,000 events | 79.729 ms | 80.958 ms | 1.229 ms / +1.54% |
| Appearance idle mean CPU, one core | 360.955% | 347.380% | -13.575 percentage points / -3.76% |
| Appearance idle sampled peak RSS | 192.594 MiB | 194.070 MiB | 1.477 MiB / +0.77% |

The environment was Debian 13.7 on Linux 6.18.44 x86_64, Intel Xeon Platinum
8573C, five visible logical CPUs, 18,882,699,264 bytes RAM and Rust 1.98.1.
The standard `cargo xtask package` build used the normal release profile and
passed the 248-file DEB smoke covering its executable, desktop entry, metadata,
ownership, content and host shared-library closure. Installed size sums regular
file content and excludes filesystem allocation and symlink overhead.

The reducer benchmark replays 100,000 deterministic gateway events into a fresh
process. After one warmup per revision, five baseline/after pairs alternated order.
Baseline samples ranged from 73.512 to 111.762 ms and after samples from 75.801 to
94.434 ms. Both revisions retained 500 records with an estimated timeline size of
331,992 to 332,477 bytes. The overlapping ranges and 1.54% median difference are
consistent with run-to-run noise, so no reducer regression or improvement is
claimed.

The native idle check used the release `profile_preview` example with demo data on
the Appearance page at 1120×760 under Xvfb and Mesa lavapipe Vulkan. Each revision
warmed for eight seconds, settled for three seconds after pointer movement, then
recorded twenty one-second CPU and RSS samples. Neither process spawned children.
After mean CPU was 13.575 percentage points lower and sampled RSS was 1,548,288
bytes higher. This single software-renderer pair does not establish a performance
change; frame timing, GPU memory and active media workloads remain unmeasured.

`cargo xtask check` passed the full workspace tests, strict all-target Clippy,
documentation, no-default-feature build and policy checks. Focused GStreamer,
voice, packaging and repository-script tests also passed, followed by the standard
package smoke. Native Windows/macOS builds, physical GPU and capture devices, live
accounts, live Wayland compositor events and extended soak testing remain
unverified.

## Responsive native controls — October 5, 2026

Historical evidence from the original combined PR, separated by scope. The recorded revision labels, hashes and measurements are unchanged; these results do not validate the new branch heads.

[Raw samples and source hashes](pr-evidence/responsive-ui/measurements.json) and
[reproduction scripts](pr-evidence/responsive-ui/README.md) record a fresh native
before/after comparison. Environment: Debian 13 x86-64, Xeon Platinum 8573C,
five visible CPUs, 17 GiB RAM, Rust 1.98.1, eframe/wgpu under Xvfb with
`WGPU_BACKEND=gl` and Mesa software output. Both builds use the same isolated
native preview, `dev` profile with optimization level 1 and no debug info, actual
UI code, synthetic long-name reply fixture, dark appearance, 1120x760 pixels
and 100% zoom. Tray/startup availability is fixed to false; the helper contains
no service or capture adapters. These are development preview measurements,
not standard release/package measurements.

Compilation stopped before sampling. One run per revision warms up for three
seconds, then samples process RSS once per second for 15 seconds and computes
aggregate CPU time over that interval. Neither process had children.

| Metric / method | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Idle CPU, percentage of one core | 0.00% | 0.00% | Below process-timer resolution |
| Post-warmup sampled peak/settled RSS | 192,663,552 bytes | 191,758,336 bytes | -905,216 bytes / -0.47% |
| Isolated development preview executable | 83,176,272 bytes | 83,183,560 bytes | +7,288 bytes / +0.009% |

The small RSS difference is noise from one short paired sample; no memory or
speed improvement is claimed. Startup peak, frame/startup latency, sustained
load, leak soaks and GPU memory remain unmeasured. Native screenshots were
inspected at 1120x760/100% dark and 760x520/150% light/dark, including scrolling
to the encoding card and the last Add Friend action. The reply/edit regression
test also verifies that context controls leave the message input inside the
viewport. All 417 UI library tests pass (five benchmark tests ignored), and
strict UI Clippy across all targets passes.

The default workspace-check/package commands hit the inherited read-only debug
cache. Retrying the workspace check and standard release package in an owned
target directory reaches the native dependency build and fails because
`glib-sys` cannot find `glib-2.0.pc`.
The environment also lacks GTK4/WebKit6 development/runtime dependencies.
Standard release executable, installed-package and compressed-distribution
sizes remain unavailable; complete workspace/packaging validation is a CI
requirement. The development preview sizes above must not be treated as shipped
package sizes.

## Codebase bug hunt — October 5, 2026

Baseline `8a369cc3c71c18d97065dde8197c16d90970ed0a`; see
[reproduction and raw samples](pr-evidence/codebase-bug-hunt/README.md).
Debian 13 x86_64, kernel 6.18.44, five visible CPUs, 18,882,699,264 bytes RAM
and Rust 1.98.1. Standard release `replay-bench` binaries were rebuilt from each
revision. Each ran once for warmup and five measured times with alternating pair
order, without concurrent compilation or preview processes. Both replay 100,000
synthetic events and retain 331,992–332,477 estimated timeline bytes / 500 records.

The isolated native Appearance preview uses actual UI code with `ui/demo`,
opt-level 1/debug 0, Xvfb, eframe/wgpu and requested GL software rendering
(`WGPU_BACKEND=gl`, `LIBGL_ALWAYS_SOFTWARE=1`; exact adapter not instrumented).
Both use 1120 × 760 dark at 100% zoom, a six-second warmup including 100 scroll
events, then 15 one-second process RSS samples. Aggregate idle CPU is the
percentage of one core over that interval. Neither preview had children. The
new synthetic fixture requests Serif after session reset; baseline silently
retains Inter, while the fixed registry applies Serif.

| Metric / method | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Release reducer median, five runs | 83.116483 ms | 88.286294 ms | +5.169811 ms / +6.22% |
| Release replay executable | 1,510,296 bytes | 1,509,592 bytes | −704 bytes / −0.047% |
| Auxiliary UI sampled peak/settled RSS | 161,026,048 bytes | 161,624,064 bytes | +598,016 bytes / +0.37% |
| Auxiliary UI idle CPU, one core | 0.00% | 0.00% | Below process-timer resolution |
| Auxiliary UI executable | 89,526,776 bytes | 89,530,560 bytes | +3,784 bytes / +0.0042% |
| Full desktop release / installed / compressed package | Unavailable | Unavailable | Missing `glib-2.0.pc` |

Reducer ranges overlap widely: 80.75–123.18 ms before and 82.21–142.91 ms after.
The single short UI pair and five replay samples do not establish a performance
improvement or a statistically significant regression. Preview binary sizes
must not be treated as shipped desktop/package sizes. Frame/startup latency,
GPU memory, sustained-load/leak soaks and physical media-device cost remain
unmeasured. Actual voice tests pass with two Linux OpenH264 runtime versions;
those checks measure correctness, not encoding performance.

The thumbnail fix admits at most two jobs and 32 MiB of source bytes before
copying/spawning, retaining permits through actual decoder completion. Failed
placeholder attempts cap at 2,048; tracked credential-removal IDs cap at eight. These are resource ceilings, not
process RSS or proof that all application leaks are absent. Standard workspace
and package checks were attempted and remain blocked at missing GLib development
metadata; the license-policy command also lacks `cargo-deny`.

## Runtime bug hunt — October 7, 2026

Compared baseline `8003ae068679fecb67cd02c4bc5e6fbedf37ffee` with this task's
client-core search corrections. The measured after build uses the working tree
before adding this documentation; the source blob IDs and binary hashes are in
[the raw measurements](pr-evidence/runtime-bug-hunt/measurements.json).
Both actual `replay-bench` binaries use Rust 1.98.1, the pinned lockfile, normal
release fat LTO and no optional features. The baseline uses a detached worktree;
both builds share the same writable Cargo cache and target directory, with the
baseline binary copied aside before building the changed sources.

Host: Linux x86-64, Intel Xeon Platinum 8573C, five exposed logical CPUs and
17 GiB exposed RAM. After one warmup per binary, five measured runs per binary
use alternating pair order. No task compiler or native fixture process runs
during sampling; variation from the shared host remains. The existing workload
applies 100,000 synthetic message events, checks the 500-record / 4 MiB timeline
bounds and verifies logout releases retained timeline data.

| Metric / method | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Reducer elapsed median (five runs) | 99.675228 ms | 95.818134 ms | −3.857094 ms (−3.87%); noisy |
| Reducer elapsed sample range | 94.165187–106.522434 ms | 87.880852–107.966072 ms | Overlapping ranges |
| Retained timeline estimate | 331,992–332,477 B / 500 records | 331,992–332,477 B / 500 records | Unchanged |
| Release replay executable | 1,511,512 B | 1,516,760 B | +5,248 B (+0.3472%) |
| Standard voice-enabled app, installed and compressed package | Unmeasured | Unmeasured | Missing `glib-2.0.pc` prevents both builds |

This workload does not exercise permission changes with an open search or
desktop credential/cache failure handling. Its timing is a general reducer
regression check, with no performance improvement claim. The executable size
is for the reducer tool, not the installed application. Native UI CPU/RSS,
startup, search latency and application-wide leak detection are unmeasured.
The full workspace check and standard package attempt stop at missing GLib
development metadata. Native before/after screenshots for search retirement
and account cleanup status are consequently unavailable.

## PR #567 upstream conflict resolution — October 7, 2026

Compared the verified release reducer from feature head
`11abe5a3700cfe0ec734808931d3791922d98b12` with the merged working tree including
upstream `20230cb91b0117fe4c888d70867fa600704dcad4`. The subsequent upstream
documentation cleanup `a76e030d749ef558e687bbff6dc592a7dddec244` changes none of
the measured runtime sources. Both use Rust 1.98.1, normal
fat LTO and no optional features; the merged build uses the updated upstream
lockfile. Separate binary paths preserve the baseline. Complete reducer source
blob IDs, executable hashes, warmups and samples are in
`docs/pr-evidence/pr567-conflict-resolution/measurements.json`.

Linux / Intel Xeon Platinum 8573C, five exposed logical CPUs, 17 GiB exposed RAM;
one warmup and five measured runs per binary, with alternating pair order and no
active task compiler during sampling. The workload replays 100,000 synthetic
events and verifies the bounded 500-record timeline and logout release.

| Metric | Baseline | Merged | Delta / method |
| --- | ---: | ---: | --- |
| Reducer median | 125.050355 ms | 144.784515 ms | +19.734160 ms (+15.78%); wide, overlapping sample ranges |
| Reducer sample range | 83.038196–158.760550 ms | 122.432188–168.732948 ms | Five samples each; shared-host scheduling noise |
| Retained timeline estimate | 331,992–332,477 B / 500 records | 331,992–332,477 B / 500 records | Unchanged; not process RSS |
| Release reducer executable | 1,516,760 B | 1,527,472 B | +10,712 B (+0.71%); not the application package |
| Standard voice-enabled app / installed / compressed package | Unmeasured | Unmeasured | Both full check and package attempts stop at missing `glib-2.0.pc` |

The measured median increased; the large timing spread prevents attributing that
change to the merge. No performance improvement is claimed. This reducer does
not exercise native audio, upload previews, renderer GPU routing or UI latency.
Full native GUI screenshots, process CPU/RSS, physical hardware encoding and
live service compatibility remain unverified. Offline audio and upload regressions
exercise their actual bounded production helpers without opening devices.

## Theme command deadline regression

The local bug hunt based on `6451d97` fixes Linux theme detection retaining a
worker when a descendant keeps the helper's stdout open after its parent exits.
The existing incremental process reader now bounds stdout and helper completion
together. With the same inert one-second descendant and 50-ms command deadline,
the original regression fails and the fixed regression passes.

| Metric | Before | After | Method |
| --- | ---: | ---: | --- |
| Fixture process elapsed median | 1013.038 ms | 60.032 ms | One warmup and five samples per binary |
| Sample range | 1008.951–1021.818 ms | 57.558–63.705 ms | Counterbalanced order, shared Linux host |

[Evidence and raw samples](pr-evidence/theme-command-timeout/README.md) record
the pinned toolchain, current-source checks and limits. Elapsed time includes
test startup and demonstrates bounded helper completion; it does not measure
whole-app theme latency, CPU or RSS. Full executable and installed/compressed
package sizes remain unmeasured because workspace check/package attempts stop
at the existing build-cache permission error.

## Post rename review repair (2026-10-14, PR #655)

Fresh standard voice-inclusive packages compare pre-repair `74ff7038` with
runtime repair `460d7117`; both include the same main integration (`c84b5998`).
Environment: Windows 11 x64 build 26200, Ryzen 7 7800X3D, 32 GB RAM, pinned
Rust 1.98.1. `cargo xtask package` builds without default, demo or developer
features. NSIS is unavailable, so these are unsigned directory packages and
ZIP archives, not installer validation.

| Metric / method | Baseline | Repair | Delta |
| --- | ---: | ---: | ---: |
| serein.exe file length | 87,251,456 B | 87,251,456 B | 0 B |
| Installed payload, 216 files | 91,444,745 B | 91,444,745 B | 0 B |
| ZIP, sorted paths / DEFLATE level 9 | 50,287,594 B | 50,287,599 B | +5 B |
| 100,000-event reducer replay, median of five | 68.47 ms | 66.50 ms | -1.98 ms / -2.89% |

Each replay executable was built once and verified through `cargo replay`,
then run directly with one warmup and five measured samples. Both retain
500 timeline records / 393,492..393,977 estimated bytes. The timing ranges
overlap (67.82..73.78 ms before, 64.52..70.47 ms after); this is not evidence
of a speed improvement. The workload measures the general synthetic reducer,
not rename-specific latency, process RSS, UI frame timing or live compatibility.

The repair retains existing bounded state and adds no dependency or cache.
Regressions cover revoked access, rename echoes with invalidated mutable details,
a fresh post-details reload and a later conflicting name. Full local
`cargo xtask check` and separate fuzz formatting passed. Native GUI capture
and owner-controlled live validation remain unavailable/unverified.
Raw samples, source revisions and executable hashes are in
[measurements.json](pr-evidence/post-rename-review/measurements.json).

## URL markup parsing (2026-10-14, issue #656)

[Measurements and source hashes](pr-evidence/url-underscores/measurements.json)
compare `fbcbbad3` with the URL protection change on Windows 11 x64 build 26200,
Ryzen 7 7800X3D, 32 GB RAM and pinned Rust 1.98.1. Standard packages include voice
and use `cargo xtask package`, without demo or developer features. Performance
notes and development evidence are not included in the installed payload.

| Metric / method | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| Executable | 87,366,656 B | 87,381,504 B | +14,848 B / +0.0170% |
| Installed payload, 216 files | 91,559,945 B | 91,574,793 B | +14,848 B / +0.0162% |
| ZIP, Optimal compression | 50,470,238 B | 50,479,398 B | +9,160 B / +0.0181% |
| Plain message, median / 10,000 parses | 25.1799 ms | 26.1144 ms | +0.9345 ms / +3.71% |
| Underscore URLs, median / 10,000 parses | 28.7708 ms | 41.5100 ms | +12.7392 ms / +44.28% |

The plain-message sample ranges overlap; no speed improvement is claimed.
Protecting underscore URLs adds about 1.27 microseconds per parse in this workload.

The release component workload is `cargo test --release --locked -p ui --lib
url_underscore_parse_benchmark -- --ignored --nocapture`. Each built test binary
is then run directly, with one warmup and five samples of 10,000 parses for each
synthetic message. This measures parsing, not native frame latency, idle CPU,
RSS or live Discord behavior. Native desktop automation is unavailable here.

These numbers were measured on the underscore-only revision `e6f92b0f`. Protection
was later generalized to `*`, `~` and `&` entities, plus a trailing-opener check of
at most four passes. That revision was not re-measured.

Protection adds temporary passes only for candidate URLs containing `_`, `*`, `~`
or `&`; other messages use an early return. Source remains capped at 8 KiB, each
pass at 512 events (exhaustion falls back to bounded literal text), and stored
links at 16. No dependency or retained cache is added.

## FFmpeg media playback — October 17, 2026

The audio/preview change starts from fork main `f9e39f7`. A native component
comparison uses that revision's Cargo-built libopus static library versus the
new `serein_audio_open/decode/close` wrapper and the existing pinned FFmpeg build.
Both consume the same 151 packets from the original three-second mono Opus
fixture at 48 kHz, recreating the decoder 200 times per sample. The temporary C
harness uses GCC `-O3 -DNDEBUG`; the shared Linux worker has a Xeon Platinum
8573C, five available CPUs and a 16 GiB memory limit. One warmup precedes five
alternating measured runs per backend; elapsed time uses `CLOCK_MONOTONIC`.
Other workspace compilation was running, so these are indicative component
measurements, not isolated native application benchmarks.

| Metric / method | `f9e39f7` libopus path | FFmpeg packet path | Delta |
| --- | ---: | ---: | ---: |
| Median elapsed per 200 full decodes | 1.5084 s | 0.8771 s | −0.6313 s (−41.9%) |
| Decoded mono samples, before demuxer trim | 28,992,000 | 28,992,000 | 0 |
| Release executable / installed payload / compressed package | Unmeasured | Unmeasured | Unavailable |

The component comparison excludes demuxing, Rust allocation, HTTP, output devices,
GUI rendering and the playback queue. Separate production-path regressions compare
trimmed duration, channel count and RMS level for WAV, MP3, Opus and Vorbis, and
check cancellation, SDR/HDR posters, rotation and bounded embedded-clip playback.
A full-clip regression also covers FFmpeg growing its input buffer: larger native
requests receive bounded 64 KiB reads instead of failing partway through playback.
No application CPU/RSS, startup, video throughput or physical display performance
claim follows from these results.
The shared audio wrapper also completed 200 decoder lifecycles with AddressSanitizer
and UndefinedBehaviorSanitizer; linked codec libraries were not instrumented.

Full local release packaging was not repeated: earlier production fat-LTO links
exceeded this worker's memory limit, and remaining disk space cannot support a
fresh matching package pair. Current native packages remain CI checks. No new
runtime library, codec recipe or license dependency is introduced by this change.


## Soundboard and playback integration — October 17, 2026

This feature builds on fork media PR #7 (`f3abb03`), not a merged fork dependency.
Focused soundboard model/protocol, request/call correlation and API loopback tests pass,
including a dropped POST response that is not retried. Separate synthetic replay and
120-second soak runs passed (1,168 passes), before the final read-only spinner guard.
Login-handoff, cached-xtask and license-policy fixtures pass; the dependency graph and
FFmpeg recipe are unchanged. Full native/UI/decoder checks and ASan results are
reported separately in the PR; earlier upstream measurements are historical evidence.

The branch's release comparison jobs use the current published upstream nightly,
previous fork and reviewed commit on Windows/macOS/Linux. Matching Rust 1.98.1
release overrides, separate targets, one warmup and five rotated samples are recorded
in JSON with exact revisions, environment and sample ranges. Core reducer timing does
not measure audio stretching, media decode, native UI latency or CPU/RSS. Those media
measurements remain unavailable here. Current-head standard native packages provide
the executable, installed payload and compressed distribution comparisons.
