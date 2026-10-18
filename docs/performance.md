# Performance findings

Recent synthetic/offline measurements are workload-specific. They do not establish live Discord
performance, universal device results or application-wide memory bounds. The older upstream PR screenshot,
log and per-run evidence archive has been removed; the summaries below retain the useful results.

## Server folder mark-as-read — October 15, 2026 (PR #672)

Compared base `104b1aa5` with folder mark-as-read `2b02b49e`. Standard unsigned Windows x64
portable packages use Rust 1.98.1 and `cargo xtask package` (voice included, demo disabled).
Each installed tree contains 216 files; ZIP output uses .NET `Compress-Archive`. NSIS was
unavailable, so no installer was measured.

| Package metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| `serein.exe` | 87,399,936 B | 87,405,568 B | +5,632 B (+0.0064%) |
| Installed tree | 91,593,225 B | 91,598,857 B | +5,632 B (+0.00615%) |
| ZIP archive | 51,350,178 B | 51,354,785 B | +4,607 B (+0.00897%) |

Release demo builds at both revisions ran the same offline `--demo` workspace with the
synthetic folder context menu open in a 1120×760 window. The after menu shows the new row disabled
because this fixture has no unread guild in that folder. On Windows 11 Pro 10.0.26300, Ryzen 7
7800X3D (8 cores / 16 logical), 31.4 GiB visible RAM and AMD Radeon RX 7900 GRE (DX12),
process counters were sampled once per revision after a five-second warmup, 21 samples about one
second apart. CPU is the `Get-Process` CPU-time delta divided by actual elapsed wall time, where
100% is one logical core. Memory reports peak and mean of the final five samples.

| Native demo metric | Baseline | After | Delta |
| --- | ---: | ---: | ---: |
| CPU, one-core percentage | 1.4795% | 0.3893% | -1.0902 percentage points |
| Working set, peak / final-five mean | 185,057,280 B | 167,518,208 B | -17,539,072 B |
| Private bytes, peak / final-five mean | 204,038,144 B | 185,970,688 B | -18,067,456 B |

These single samples are descriptive only; no runtime improvement is claimed. Startup and frame
latency were not measured. The synthetic screenshots, raw samples and reproduction details are in
[`docs/pr-evidence/folder-mark-as-read/`](pr-evidence/folder-mark-as-read/). Live Discord behavior
and clicking the action against a folder containing unread guilds remain outside this offline run.
A later revision of the PR replaced the per-guild queue with the home tile's bulk acknowledgement
scoped to the folder's servers; it was not re-measured.

## Personal notification and navigation controls — October 17, 2026

Compared current upstream nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`),
fork baseline `f9e39f7`, and implementation `8205c36d931b`. Each release replay
was built with Rust 1.98.1, the committed lockfile, default replay features, fat
LTO and one codegen unit, using a separate source/target directory. Linux 6.18.44
x86-64/glibc 2.41 container, Xeon Platinum 8573C, five visible CPUs and
18,440,136 KiB visible RAM. Builds and tests were idle during sampling. One warmup
and five measured direct-binary runs per revision/workload, rotating revision order,
with the measurement process and its children pinned to CPU 0; elapsed times are
reported by the workload's `Instant` timer. No account, network, storage or audio device.

| Release workload / median | Current nightly | Fork baseline | After | After vs nightly | After vs fork |
| --- | ---: | ---: | ---: | ---: | ---: |
| Reducer, 100,000 events | 108.718 ms | 124.482 ms | 122.868 ms | +13.0% | −1.3% |
| Wide-channel reducer, 10,000 messages / 20,001 channels | 166.945 ms | 196.989 ms | 174.953 ms | +4.8% | −11.2% |
| Steady retained timeline estimates, 500 records | 393,492–393,977 B | 401,492–401,977 B | 401,492–401,977 B | +8,000 B | unchanged |

Scheduling variation is substantial: reducer ranges were 104.234–218.455 ms
(nightly), 110.811–216.428 ms (fork), and 112.729–185.926 ms (after); wide-channel
ranges were 146.976–175.158, 169.186–227.320, and 162.341–221.322 ms respectively.
An initial unpinned trial gave +5.2% / +33.2% against nightly, with a noisy
106.586–406.827 ms nightly wide-channel range, prompting the affinity-controlled
comparison above. No speedup is claimed. The nightly comparison includes earlier
video, reliability and upstream changes already in the fork; it does not isolate
this milestone. The feature's retained timeline estimates match its fork baseline.

The 120-second release lifecycle soak completed 1,444 passes / 46,208 channel
visits / 27,724,800 live inserts and six logout cycles in 120.896 seconds, with
its shipping row/byte assertions passing. This measures synthetic core pressure;
native UI CPU/RSS, frame latency and live synchronization remain unmeasured.

Current-nightly package baselines were downloaded from the immutable release tag
and checked against its published SHA256SUMS. Payload bytes sum regular archive
entries; Linux uses the Debian data archive, Windows its portable ZIP, and macOS
the complete ZIP including the app bundle and notices.

| Nightly package | Executable bytes | Full payload bytes | Published archive bytes |
| --- | ---: | ---: | ---: |
| Ubuntu 26.04 x64 DEB | 87,746,736 | 92,280,770 | 44,864,244 |
| Windows x64 ZIP | 86,937,600 | 91,130,889 | 51,171,600 |
| macOS ARM64 ZIP | 69,149,632 | 75,120,529 | 48,274,698 |

Current-head native application/package checks and matching after-package
measurements are tracked in the PR. No local production package, native Windows/macOS
UI performance comparison or live OS-notification check is claimed here.

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

## PR #620 upstream conflict resolution — October 18, 2026

Runtime merge `3d1acc7968e228c64fbe683b0e13aa9cd0804502` combines the previous
PR head `4806c9bb4a0f2646bf3e43e048e08796843705b8` with parent main
`d9d87471f0ea2190da3f90db815e115045461703`. The evidence follow-up changes only
this document. Account cleanup retains upstream's opt-in saved-login storage
and pending-save protection. Upload-preview ownership, GIF hover fixtures and
cached-package font-license fixtures are reconciled with upstream.

Pinned Rust 1.98.1 `cargo xtask check` passes: **1,537 passed, 29 ignored**, with
formatting, strict workspace/all-target Clippy, minimal production configuration
and policy checks. Final strict Clippy, login-handoff, cached-xtask/workspace/package,
Linux/Flatpak packaging, repository-setup and mocked macOS signing fixtures pass.
Both locked license graphs and six license-policy fixtures pass. Bounded ASan
fuzzing completes 98,662 decode and 7,086 state runs (31 seconds per target,
512 MiB ceiling), with no finding. Release replay and the 120-second lifecycle
soak pass; the soak completes 1,866 passes and eight logout cycles. Replay ran
alongside compilation, so its timing is not a comparable performance sample.
These checks use synthetic/offline input and do not establish live compatibility.

Standard voice-enabled Linux directory packaging uses Debian 13 x86-64, the
pinned toolchain/lockfile, normal release fat LTO, two Cargo build jobs and a
task-owned native SDK. The complete `cargo xtask package --format dir` command
passes with an empty output directory, then passes again after preserving that
output and starting with another empty directory. Dependency caches are reused;
this is not a cold-cache measurement. No staging repair is performed.

| Footprint / repeatability | First output | Cached repeat | Delta |
| --- | ---: | ---: | ---: |
| Executable | 92,193,224 B | 92,193,224 B | 0 B |
| Installed payload (242 files) | 96,791,057 B | 96,791,057 B | 0 B |
| Normalized ZIP | 55,763,863 B | 55,763,863 B | 0 B |

The payload matches the current Cargo executable and allowlisted installation
manifest, modes and notices, including all 19 font licenses. ELF dependency
closure resolves with the task SDK; no development RPATH is embedded. The
application is not launched. The comparison ZIP is generated from each verified
directory payload using sorted paths, fixed timestamps, preserved 0755/0644
modes and DEFLATE level 9. This checks repeatability, not a PR performance delta.
Executable SHA-256: `2559f36f1c19738521712aa48ff2dbd123e3f8f0abce1bb6a8323e6249b88f6e`.
Normalized ZIP SHA-256: `21067951174a549de564eb5a5965a5ccc8da4c6fd67963b00492e7f3a54c56be`.
Both payload manifests and both ZIP hashes are identical.

Historical component evidence keeps its original revisions: the
[theme-helper fixture](https://github.com/MCShotty/Serein/blob/4e01c7c9526d17da292485e108abf18621dd5ae2/docs/pr-evidence/theme-command-timeout/README.md)
records a 1,013.038 → 60.032 ms median with one warmup and five samples.
The avatar reservation fixture compares exact `reserve_attempt` implementations
at `422e7ad` and `463abd7`: Rust 1.98.1 `-O`, offline Linux x86-64 (4 vCPUs,
12 GiB), one warmup and five samples of 100,000 calls with 2,047 fresh failed
entries; median 1,312.063 → 0.149 ms. Neither fixture measures whole-app speed.
The October 15 package SIGKILL remains a historical failed attempt; this pass
does not relabel that result.

No fresh matched runtime baseline, UI latency, CPU/RSS, physical-device, Windows
or macOS package measurement is claimed. Native display and live Discord checks
remain unverified. Parent native/security/license/fuzz CI jobs exclude draft PRs;
#620 retains its draft status at the owner's request. Mocked signing and local
Linux SDK closure do not establish native macOS or global Linux installation.


## Application-command follow-up — October 17, 2026

Based on fork `f9e39f7`. Focused command schema, discovery, API loopback, autocomplete
correlation and composer regressions pass. Separate replay and 120-second soak runs
passed (1,024 passes). The bounded ASan decoder/state smoke passed; dependencies and
lockfiles are unchanged. The final full-form stale-suggestion guard has focused Core
and UI coverage. Full workspace/native demo checks are recorded in the PR.

`scripts/compare-replay.py` compares the reviewed commit, the current published upstream
nightly and the previous fork on independent Windows/macOS/Linux CI workers. It uses
matching Rust 1.98.1 release overrides, separate worktrees/targets, one warmup and five
rotated samples per workload. Artifacts retain revisions, environment and sample ranges.
These synthetic reducer results are not UI latency or media throughput measurements.
Current-head executable, installed-payload and compressed-package comparisons await
native CI; UI CPU/RSS and live command performance remain unmeasured.


### Command review and native evidence at `564d8dd1` plus the field-style correction

The complete local `cargo xtask check` passed 1,676 tests (32 ignored), policy
checks and the minimal application build at `564d8dd1`. The later autocomplete
field correction only matches the existing proportional 14-point scalar-field
font, alignment and text color. Its existing composer regression, strict native
UI all-target/demo Clippy, formatting and native `profile_preview` rebuild pass.

The native before/after pair in `pr-evidence/application-commands` uses the same
1120 × 760, scale-1 dark X11 window and synthetic `/weather` form with `Pra`.
Before is fork `f9e39f7`; after is `564d8dd1` plus that three-line style fix.
The before build has two demo-only compilation repairs: remove duplicated avatar
helpers and supply the missing optional error argument to `pending::show`.
Neither changes the displayed feature. After opens the new synthetic suggestions
popup; this is feature-absence evidence, not live service data. Native light-mode
layout at the supported minimum 760 × 520 was also inspected; those extra captures
are not bundled. Capture uses explicit `--demo --interactive`, Xvfb, software
rendering and ImageMagick's application-window capture, with process-liveness checks.
These debug preview pictures do not establish release CPU/RSS or frame latency.

A submitted command's later HTTP failure still requires form re-entry. Only local
validation failures preserve that form; this limitation remains in the inventory.


### Cross-platform command replay at `564d8dd1`

[Run 39056323878](https://github.com/MCShotty/Serein/actions/runs/39056323878)
compares nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`), fork `f9e39f7`
and command code `564d8dd1`. The later field-style correction does not change
this core workload. Five rotated measured runs follow one warmup per build;
Rust 1.98.1, opt-level 3, thin LTO, one codegen unit and panic=abort match.
Linux/Windows pin one allowed logical CPU; macOS uses its platform scheduler.
Raw CI JSON records each worker's OS/CPU, full revisions, settings and ranges.
Values are milliseconds; each entry is median (minimum–maximum).

| Platform / workload | Nightly | Previous fork | Commands | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / reducer | 80.517 (79.993–80.916) | 79.284 (78.504–80.101) | 79.093 (79.018–82.000) | -1.8% / -0.2% |
| Linux-X64 / wide channels | 226.205 (221.134–229.776) | 230.142 (224.395–234.305) | 226.427 (221.166–231.768) | +0.1% / -1.6% |
| Windows-X64 / reducer | 78.035 (77.959–101.676) | 79.240 (79.080–106.378) | 78.814 (78.539–79.393) | +1.0% / -0.5% |
| Windows-X64 / wide channels | 48.810 (48.105–49.212) | 69.543 (68.001–74.627) | 68.152 (67.905–68.479) | +39.6% / -2.0% |
| macOS-ARM64 / reducer | 55.292 (55.175–55.410) | 55.624 (55.409–56.302) | 55.943 (55.716–56.636) | +1.2% / +0.6% |
| macOS-ARM64 / wide channels | 83.693 (83.187–84.001) | 83.590 (83.116–84.013) | 82.957 (82.794–83.901) | -0.9% / -0.8% |

The Windows wide-channel result is 39.6% slower than nightly; the previous fork
is already slower. Its cause is unisolated. These are core reducer measurements,
not autocomplete, UI latency, media throughput or a causal improvement claim.


### Native package snapshot at `564d8dd1`

## Moderation and AutoMod — October 17, 2026

Based on fork `f9e39f7`. Focused permission/hierarchy, sparse rule preservation, bounded
parsing, loopback HTTP and native UI-state regressions pass, including whitespace
preservation on unrelated rule edits. Replay and the 120-second soak passed (1,531
passes). Login-handoff, cached-xtask workspace and license-policy fixtures pass; the
locked graph is unchanged. After sequencing builds, bounded ASan decoder/state smoke
passed at `91c95fdd` (112,105 and 3,638 runs, 31 seconds per target). The earlier compiler
OOM did not produce a fuzz result. Native application checks are recorded separately in the PR.

`scripts/compare-replay.py` compares the reviewed commit, the current published upstream
nightly and the previous fork on independent Windows/macOS/Linux CI workers. Matching
Rust 1.98.1 release overrides, separate worktrees/targets, one warmup and five rotated
samples produce revision-labelled JSON artifacts including sample ranges. Native
packages must separately establish executable, installed-payload and distribution
sizes. Native UI CPU/RSS, moderation latency and live service behavior remain unmeasured.


### Moderation release comparison at `91c95fdd`

[Native CI run 39056325676](https://github.com/MCShotty/Serein/actions/runs/39056325676)
compares published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`), previous fork
`f9e39f7` and moderation code `91c95fdd`. Rust 1.98.1, opt-level 3, thin LTO, one
codegen unit, stripped symbols and panic=abort match across each worker's builds.
One warmup precedes five rotated samples of each bounded workload. Linux/Windows
pin one allowed logical CPU; macOS uses its platform scheduler. The run's per-OS JSON
artifacts include the exact OS/CPU, full revisions, settings and raw samples. These
are separate GitHub-hosted x64 Linux/Windows and ARM64 macOS workers; elapsed values
are milliseconds, with minimum–maximum ranges below.

| Platform / workload | Nightly median (range) | Previous fork | Moderation | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / reducer | 48.866 (47.986–49.167) | 47.722 (47.084–48.275) | 47.769 (47.428–48.763) | -2.2% / +0.1% |
| Linux-X64 / wide channels | 61.633 (60.952–68.746) | 61.370 (60.250–66.805) | 62.140 (60.508–63.802) | +0.8% / +1.3% |
| Windows-X64 / reducer | 97.703 (95.286–130.104) | 97.112 (93.646–113.227) | 96.023 (94.101–144.464) | -1.7% / -1.1% |
| Windows-X64 / wide channels | 94.748 (81.093–118.035) | 116.428 (107.706–146.935) | 112.896 (109.315–161.461) | +19.2% / -3.0% |
| macOS-ARM64 / reducer | 79.794 (73.478–90.337) | 80.099 (61.130–91.886) | 80.526 (70.608–92.206) | +0.9% / +0.5% |
| macOS-ARM64 / wide channels | 137.806 (112.357–141.365) | 125.894 (113.885–155.284) | 130.810 (107.787–215.842) | -5.1% / +3.9% |

The Windows wide-channel result is slower than nightly; its cause is unisolated.
The workloads exercise core state, not moderation latency, native UI or media throughput.
No causal performance improvement or official-client equivalence is claimed.


### Native package snapshot at `91c95fdd`

Standard voice-inclusive native CI packages, compared with published nightly
`3a4bf8d` and previous fork `f9e39f7`. Installed size sums regular-file bytes;
distribution normalizes those files with Python 3.12 ZIP DEFLATE level 6 and
fixed metadata, excluding outer CI archives and installers. One artifact per
revision/platform; MiB = 1,048,576 bytes. Other native platforms remain pending.

| Platform / metric | Nightly | Previous fork | This code | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / executable | 83.682 | 87.900 | 88.037 | +0.137 (+0.16%) |
| Linux-X64 / installed payload | 88.006 | 130.657 | 130.795 | +0.137 (+0.11%) |
| Linux-X64 / normalized distribution | 51.211 | 84.553 | 84.614 | +0.061 (+0.07%) |

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
reported separately in the PR; earlier upstream measurements are historical evidence. Bounded ASan
decoder/state smoke subsequently passed (93,615 and 2,469 runs, 31 seconds each) on the
reviewed media implementation. The subsequent Windows-only test-argument correction
changes no fuzzed parser/reducer code.

The branch's release comparison jobs use the current published upstream nightly,
previous fork and reviewed commit on Windows/macOS/Linux. Matching Rust 1.98.1
release overrides, separate targets, one warmup and five rotated samples are recorded
in JSON with exact revisions, environment and sample ranges. Core reducer timing does
not measure audio stretching, media decode, native UI latency or CPU/RSS. Those media
measurements remain unavailable here. Current-head standard native packages provide
the executable, installed payload and compressed distribution comparisons.


### Current media release comparison at `a94983ab`

[Run 39058677973](https://github.com/MCShotty/Serein/actions/runs/39058677973)
compares published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`), previous
fork `f9e39f7` and media code `a94983ab` on separate Windows x64, Linux x64 and
macOS ARM64 workers. Rust 1.98.1, opt-level 3, thin LTO, one codegen unit and
panic=abort match; one warmup precedes five rotated measured runs. Linux/Windows
pin one allowed CPU, while macOS uses the platform scheduler. Raw CI JSON records
OS/CPU, exact revisions, settings and ranges. Values are milliseconds, median
(minimum–maximum). This core-only workload does not exercise audio decoding,
stretching, soundboard mixing or GUI rendering.

| Platform / workload | Nightly | Previous fork | Media | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / reducer | 63.295 (61.608–64.248) | 63.491 (61.648–64.701) | 64.237 (60.519–65.560) | +1.5% / +1.2% |
| Linux-X64 / wide channels | 112.988 (94.424–117.141) | 119.903 (116.312–121.558) | 121.310 (116.398–122.577) | +7.4% / +1.2% |
| Windows-X64 / reducer | 58.066 (57.603–60.780) | 58.144 (57.711–60.494) | 56.240 (55.600–57.943) | -3.1% / -3.3% |
| Windows-X64 / wide channels | 69.075 (64.426–84.789) | 64.319 (62.407–66.892) | 64.566 (63.838–68.376) | -6.5% / +0.4% |
| macOS-ARM64 / reducer | 67.099 (66.357–70.469) | 68.648 (66.939–70.439) | 68.725 (67.667–70.565) | +2.4% / +0.1% |
| macOS-ARM64 / wide channels | 100.793 (97.166–103.192) | 105.346 (103.715–108.621) | 100.973 (99.229–106.693) | +0.2% / -4.2% |

Linux wide-channel replay is 7.4% slower than nightly and 1.2% slower than the
previous fork; the cause is unisolated. No causal speed improvement is claimed.
The inherited FFmpeg-first decoder changes in prerequisite #7 are included here;
these comparisons do not isolate soundboard cost. Standard native package sizes
are recorded below; release media throughput, idle CPU, RSS and frame latency remain
unmeasured. This is not proof of official-client interoperability.


### Current media focused native checks

At `a94983ab`, strict native `ui`/`serein` all-target/demo Clippy and
formatting pass. Focused native checks pass four sound-editor UI cases, eight
audio/UI cases, twelve sound/worker/upload cases, one Ogg Opus round-trip and
two pitch-preserving speed cases. They cover bounded clips, stale-call worker
cancellation, deafen, released cached PCM, trim encoding, damaged audio, keyboard
controls, narrow cards and preserved unknown/custom emoji on unrelated edits.
These are offline fixtures, not a microphone or live-call test.

Native CI run 39058677973 passes all four application/package gates (Linux x64,
Windows x64/ARM64 and macOS ARM64), all three release replay comparisons,
x64/ARM64 installer fixtures, SDK, licenses and ASan fuzz.
Inherited security and separate Nix failures remain visible in the PR. The
Windows-only missing optional-effects test argument was corrected at `a94983ab`.

### Linux media package snapshot at `a94983ab`

Standard release including voice, compared with nightly `3a4bf8d` and previous
fork `f9e39f7`. Installed values sum regular-file bytes; distribution recompresses
the payload using Python 3.12 ZIP DEFLATE level 6 and fixed metadata, excluding
the outer CI archive/installers. One artifact per revision; MiB = 1,048,576 bytes.

| Metric | Nightly | Previous fork | Media | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| executable | 83.682 | 87.900 | 88.282 | +0.382 (+0.43%) |
| installed payload | 88.006 | 130.657 | 131.039 | +0.382 (+0.29%) |
| normalized distribution | 51.211 | 84.553 | 84.700 | +0.147 (+0.17%) |

This includes prerequisite #7 and does not isolate the soundboard contribution. Windows/macOS package snapshots follow below.


### Current media native screenshots

The matched native pair in `pr-evidence/desktop-media` uses the synthetic server
settings fixture at 1120 × 760, scale 1, dark mode. Before is fork `f9e39f7`
with only the two demo-build repairs (duplicate avatar helpers and the missing
`pending::show` optional-error argument). Prerequisite #7 has no changes to these
UI/settings/preview paths, verified against `f3abb039`; the baseline represents
feature absence. After is media code `a94983ab`, not an upstream screenshot.
The supported minimum 760 × 520 light-mode soundboard layout was inspected too.

The native preview build passes. Captures launch explicitly with
`--demo --interactive` on Xvfb/software rendering, check process liveness and
capture only the app window with ImageMagick. No account, microphone, speaker
playback or service was used. Only one small current pair is committed. Debug
preview captures are not release CPU/RSS, frame latency or live acceptance evidence.


### Windows and macOS media package snapshots at `a94983ab`

Same standard voice-inclusive package and normalization method as Linux above.
One artifact per revision/platform; MiB = 1,048,576 bytes. Baselines are nightly
`3a4bf8d` and previous fork `f9e39f7`. CI checkout `d03ae140` has the exact Git
tree of source `a94983ab`, verified before these measurements were recorded.

| Platform / metric | Nightly | Previous fork | Media | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| Windows-X64 / executable | 82.910 | 87.505 | 87.982 | +0.477 (+0.54%) |
| Windows-X64 / installed payload | 86.909 | 135.216 | 135.693 | +0.477 (+0.35%) |
| Windows-X64 / normalized distribution | 47.986 | 80.844 | 81.015 | +0.170 (+0.21%) |
| macOS-ARM64 / executable | 65.946 | 69.884 | 70.198 | +0.314 (+0.45%) |
| macOS-ARM64 / installed payload | 71.641 | 95.216 | 95.530 | +0.314 (+0.33%) |
| macOS-ARM64 / normalized distribution | 46.027 | 63.548 | 63.677 | +0.129 (+0.20%) |

Windows x64/ARM64 both pass clean and cached normal packaging from empty job-owned
output directories, archive validation and a bounded packaged driver-query launch
without development FFmpeg paths. No manual staging repair was used. The x64 ZIPs
are both 86,052,573 bytes with metadata-dependent hashes; bit-for-bit reproducibility
is not claimed. The query helper exits before account/UI/audio initialization,
so these checks do not establish an interactive launch or hardware encoding.
macOS nightly is signed and the CI bundle is unsigned. These are observed package
deltas, not isolated soundboard cost or release media/UI performance claims.

revision/platform; MiB = 1,048,576 bytes. The Windows x64 snapshot below completes this three-platform comparison.

| Platform / metric | Nightly | Previous fork | This code | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / executable | 83.682 | 87.900 | 88.186 | +0.286 (+0.33%) |
| Linux-X64 / installed payload | 88.006 | 130.657 | 130.944 | +0.287 (+0.22%) |
| Linux-X64 / normalized distribution | 51.211 | 84.553 | 84.678 | +0.124 (+0.15%) |
| macOS-ARM64 / executable | 65.946 | 69.884 | 70.135 | +0.251 (+0.36%) |
| macOS-ARM64 / installed payload | 71.641 | 95.216 | 95.467 | +0.251 (+0.26%) |
| macOS-ARM64 / normalized distribution | 46.027 | 63.548 | 63.659 | +0.111 (+0.17%) |

macOS nightly is signed while CI is unsigned; these are observed package deltas, not an isolated feature cost.


### Current moderation native evidence

Code `91c95fdd` passes the local strict native `ui`/`serein` all-target/demo
Clippy run, three AutoMod UI regressions and the moderation cancellation UI
regression. The native preview rebuild also passes. These checks follow the
read-only ban-view permission correction and whitespace-preservation fix.
Native CI run 39056325676 passes the full application/package gate on Linux x64,
Windows x64/ARM64 and macOS ARM64, plus SDK, licenses, ASan fuzz, installer
fixtures and all three release replay comparisons. Inherited security/Nix
failures are reported separately; an earlier local resource-limited full-gate
attempt is not claimed as passed.

`pr-evidence/server-moderation` contains an actual native X11 window pair at
1120 × 760, scale 1, dark mode. The same synthetic server settings fixture shows
feature absence at fork `f9e39f7` and the AutoMod page at `91c95fdd`. The baseline
has only the two documented demo-build repairs (duplicate avatar helpers and
missing `pending::show` optional-error argument); its feature UI is unchanged.
The supported minimum 760 × 520 light-mode moderation layout was also inspected.
Captures use explicit `--demo --interactive`, Xvfb/software rendering, bounded
process-liveness checks and ImageMagick application-window capture. They do not
establish live moderation, release UI performance or a side-by-side official
client comparison. Only one small current pair is committed.


### Windows moderation package snapshot at `91c95fdd`

Same standard package and normalization method as the Linux/macOS snapshot above.
One artifact per revision; MiB = 1,048,576 bytes. Baseline nightly `3a4bf8d`,
previous fork `f9e39f7`, after `91c95fdd`.

| Metric | Nightly | Previous fork | Moderation | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| executable | 82.910 | 87.505 | 87.870 | +0.365 (+0.42%) |
| installed payload | 86.909 | 135.216 | 135.581 | +0.365 (+0.27%) |
| normalized distribution | 47.986 | 80.844 | 80.979 | +0.135 (+0.17%) |

Windows x64/ARM64 CI validates both clean and cached normal packages from empty
output directories. The bounded packaged driver-query helper launches with the
development FFmpeg prefix removed from PATH; no manual staging repair was used.
This check stops before UI/account/audio initialization and is not a full
interactive launch or hardware-encoding result. Archive metadata may change hashes.

## Desktop Inbox — October 17, 2026

Code revision `2139384fcb259e504535bd9688c9de6cb2e4a727`, based on fork `f9e39f7`.
The current published upstream nightly was rechecked on October 17:
`v1.0.0-nightly.20261014.58` (`3a4bf8d`). The owner-supplied Discord Stable build is
recorded in the compatibility inventory; it is not the performance baseline.

Release reducer comparison: Rust 1.98.1, committed lockfiles and release profiles,
Linux 6.18.44 x86_64/glibc 2.41, Intel Xeon Platinum 8573C, 16 GiB container memory,
four-CPU quota. Each binary/workload received one warmup and five rotating measured
runs, pinned to CPU 0. Task compiler processes were paused during sampling and no
native preview was running. Values below are medians; ranges expose host variation.

| Workload / elapsed milliseconds | Current nightly | Previous fork | Inbox | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| 100,000-event reducer | 152.105 (146.564–254.134) | 151.199 (139.729–213.125) | 149.063 (139.452–223.483) | −2.0% / −1.4% |
| Wide channel navigation | 185.019 (156.702–248.175) | 211.192 (195.419–287.684) | 219.871 (206.293–231.027) | +18.8% / +4.1% |

The wide-channel result is slower; its cause has not been isolated. The overlapping,
wide ranges do not establish a causal improvement or regression from the Inbox alone.
These are synthetic core workloads, not GUI latency, media throughput or process RSS.
The final 120-second soak passed 1,457 passes, 46,624 visits and six logout cycles
in 120.041 seconds, with the existing retained-history budgets unchanged.

`cargo xtask check` passed on the implementation before the final navigation/content
retirement corrections. Focused navigation and Inbox tests, strict Clippy and native
preview builds were rerun after those corrections; the initial follow-up Inbox suite has nine
passing cases, including dark/light narrow layouts and read-marker isolation.
The bounded ASan fuzz smoke passed 80,860 decoder inputs and 3,079 state transitions.
Login-handoff, cached-xtask workspace, license-policy and packaging fixtures passed.

The matched native screenshot pair in `pr-evidence/desktop-inbox` uses the same
1120 × 760 channel-links fixture, font setup and dark appearance. Before is fork
`f9e39f7` (with its required demo-only pending-upload tuple repair); after adds the
new synthetic Inbox state, not live account data. Light/narrow rendering and bookmark
navigation were inspected separately. The feature branches also remove byte-identical
duplicate demo helpers so their previews compile; production behavior is unaffected.

The initial native/package results are recorded below; subsequent review fixes require
their own current-head CI. Baseline fork artifacts are pinned to run `39021847864` at `f9e39f7`;
its native, installer, license and fuzz jobs passed, while security and separate Nix
checks failed. Native release UI CPU/RSS, frame latency, media throughput and physical
Windows/macOS/Discord interoperability remain unmeasured here. No debug-preview result
is represented as a release or official-client performance measurement.

### Initial Inbox package comparison

These are the initial feature packages at PR head `f19c412`, built by native CI
run [39048221415](https://github.com/MCShotty/Serein/actions/runs/39048221415).
GitHub checked out merge `37cd8be`; both revisions have the same Git tree
`f291a249939b99466b9ff42c68ee0620a3a243a2`. Later review corrections are not included
in these measurements; their current-head checks/results are reported in the PR.

Baselines are published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`) and previous
fork `f9e39f7`. All are standard release packages including voice. Installed values
sum regular-file payload bytes; distribution values recompress that payload with
Python 3.12 ZIP DEFLATE level 6 and fixed metadata, excluding outer CI archives and
installers. Debian payloads are read without installation. Values below are MiB
(1,048,576 bytes); deltas are against the previous fork. One artifact per build/platform.

| Platform | Metric | Nightly | Previous fork | Initial Inbox | Delta vs fork |
| --- | --- | ---: | ---: | ---: | ---: |
| Linux-X64 | Executable | 83.682 | 87.900 | 88.274 | +0.374 (+0.43%) |
| Linux-X64 | Installed payload | 88.006 | 130.657 | 131.031 | +0.374 (+0.29%) |
| Linux-X64 | Normalized distribution | 51.211 | 84.553 | 84.710 | +0.157 (+0.19%) |
| Windows-X64 | Executable | 82.910 | 87.505 | 87.968 | +0.462 (+0.53%) |
| Windows-X64 | Installed payload | 86.909 | 135.216 | 135.678 | +0.462 (+0.34%) |
| Windows-X64 | Normalized distribution | 47.986 | 80.844 | 81.007 | +0.162 (+0.20%) |
| macOS-ARM64 | Executable | 65.946 | 69.884 | 70.198 | +0.314 (+0.45%) |
| macOS-ARM64 | Installed payload | 71.641 | 95.216 | 95.529 | +0.314 (+0.33%) |
| macOS-ARM64 | Normalized distribution | 46.027 | 63.548 | 63.683 | +0.135 (+0.21%) |

Windows x64 and ARM64 CI completed clean and cached packages from empty output
directories and validated their archives. The packaged driver-query helper exited
successfully with the development FFmpeg prefix removed from PATH. This is a bounded
load/query check before UI, account, capture or audio initialization; it is not a full
interactive application launch or a hardware encoding result. No staging repair was used.
A separate local release build was interrupted before staging for review corrections;
no completed local clean/cached package is claimed. Native playback throughput, UI
CPU/RSS and physical display/Discord checks remain unmeasured.

The sibling feature branches add `scripts/compare-replay.py` and three isolated CI jobs.
Each records the current published nightly, previous fork and reviewed head, Rust 1.98.1,
matching release overrides, OS/CPU, one warmup and five rotated samples per workload.
Linux/Windows sampling uses one allowed logical CPU when available; macOS reports the
platform scheduler. Raw results include ranges and are uploaded per OS/architecture.
These are synthetic core measurements, not UI/media performance or a parity claim.

### Inbox review corrections

The subsequent lifecycle review adds ten passing Inbox regressions, including service
echoes before/after REST confirmation, closing/reopening during accepted writes,
transient Gateway outages and a stopped connection retiring an unknown outcome.
Rust 1.98.1 formatting and native/demo all-target strict Clippy passed after these fixes.
The earlier full workspace, fuzz, replay and package results above retain their original
revision scope; they are not measurements of these corrections. The native connection
cancellation test awaits CI because its local code-generation run was deferred for the
workspace memory budget. Current revision results are linked in the PR.


### Current Inbox application and package verification at `3d20f9bb`

[Run 39054844003](https://github.com/MCShotty/Serein/actions/runs/39054844003)
passed all four native application/package jobs (Linux x64, Windows x64/ARM64,
macOS ARM64), installer fixtures, SDK, licenses, ASan fuzz and all three release
replay comparisons. Its merge checkout `071e456b` has the exact Git tree of
`3d20f9bb`. The connection-cancellation test is included in the current native
workspace gate; its formerly deferred local run is not claimed separately.
Security and separate Nix failures remain inherited.

The initial package measurements above remain historical. Current standard
voice-inclusive packages use the same regular-file installed-payload and fixed
ZIP DEFLATE-6 normalization method. Baselines remain published nightly
`3a4bf8d` and previous fork `f9e39f7`; one artifact per revision/platform.
No installer or outer CI archive is included in the normalized distribution.
Values are MiB (1,048,576 bytes), with absolute/percentage deltas versus the fork.

| Platform / metric | Nightly | Previous fork | Current Inbox | Delta vs fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / executable | 83.682 | 87.900 | 88.273 | +0.373 (+0.42%) |
| Linux-X64 / installed payload | 88.006 | 130.657 | 131.031 | +0.373 (+0.29%) |
| Linux-X64 / normalized distribution | 51.211 | 84.553 | 84.712 | +0.158 (+0.19%) |
| Windows-X64 / executable | 82.910 | 87.505 | 87.970 | +0.465 (+0.53%) |
| Windows-X64 / installed payload | 86.909 | 135.216 | 135.681 | +0.465 (+0.34%) |
| Windows-X64 / normalized distribution | 47.986 | 80.844 | 81.007 | +0.163 (+0.20%) |
| macOS-ARM64 / executable | 65.946 | 69.884 | 70.198 | +0.314 (+0.45%) |
| macOS-ARM64 / installed payload | 71.641 | 95.216 | 95.530 | +0.314 (+0.33%) |
| macOS-ARM64 / normalized distribution | 46.027 | 63.548 | 63.686 | +0.138 (+0.22%) |

Windows x64 and ARM64 clean/cached packages both pass archive validation and the
bounded driver-query launch without the development FFmpeg prefix in PATH.
The x64 clean/cached ZIPs are both 86,047,535 bytes; ARM64 is 69,104,411 bytes.
Their metadata-dependent hashes differ, so bit-for-bit reproducibility is not
claimed. The helper exits before UI/account/audio initialization: these are not
interactive application launches or hardware-encoding acceptance tests. No manual
staging repair was used. macOS nightly is signed while the CI bundle is unsigned;
all package figures are observed build deltas, not an isolated feature cost.
Current artifacts and clean/cached JSON evidence are retained by that run.
Release UI CPU/RSS, media throughput and physical display checks remain unmeasured.


### Current Inbox cross-platform release replay at `3d20f9bb`

[Run 39054844003](https://github.com/MCShotty/Serein/actions/runs/39054844003)
compares published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`), previous
fork `f9e39f7` and Inbox code `3d20f9bb`. The same release settings and sampling
method above apply: one warmup, five rotated samples, one allowed logical CPU
on Linux/Windows and platform scheduling on macOS. Per-OS artifacts record exact
revisions, OS/CPU, settings and raw samples. Values are milliseconds, median
(minimum–maximum). They measure core state, not native UI/media performance.

| Platform / workload | Nightly | Previous fork | Inbox | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| Linux-X64 / reducer | 56.668 (55.112–57.319) | 57.320 (55.984–60.732) | 56.592 (55.781–56.929) | -0.1% / -1.3% |
| Linux-X64 / wide channels | 100.255 (89.555–102.417) | 109.163 (107.814–110.290) | 95.278 (93.712–96.481) | -5.0% / -12.7% |
| Windows-X64 / reducer | 110.119 (103.805–124.310) | 107.383 (106.629–108.035) | 106.993 (106.750–109.522) | -2.8% / -0.4% |
| Windows-X64 / wide channels | 228.188 (222.732–231.541) | 227.009 (224.482–233.393) | 224.888 (222.317–228.622) | -1.4% / -0.9% |
| macOS-ARM64 / reducer | 65.531 (64.239–70.252) | 70.053 (60.927–79.586) | 66.613 (62.559–79.989) | +1.7% / -4.9% |
| macOS-ARM64 / wide channels | 100.838 (94.759–111.089) | 94.479 (91.853–99.470) | 98.590 (90.037–116.093) | -2.2% / +4.4% |

Ranges overlap and no causal improvement is claimed. Earlier local slower measurements above remain historical and unisolated.


### Final Inbox native evidence and footer correction

The final pair replaces the initial feature screenshots with fresh native captures
of fork `f9e39f7` and Inbox code `3d20f9bb` plus the footer-only correction.
Both use the same synthetic channel-links scenario, 1120 × 760 dark window and
scale 1. The baseline has the two documented demo-only build repairs (duplicate
avatar helpers and the missing optional error argument to `pending::show`),
with its displayed feature UI unchanged. The new frame opens the Unread tab;
no live account or service is used. Light mode at the supported minimum 760 × 520
was also inspected. Only one small current pair is committed.

The final native capture exposed an empty action strip on a fresh Unread page.
The footer now draws when another tab has controls or an existing stale-status
message needs it. The ten Inbox regressions, pinned formatting and strict native
UI all-target/demo Clippy pass after this conditional-rendering correction.
The native preview is rebuilt for the final captures. Capture uses explicit
`--demo --interactive`, Xvfb/software rendering, process-liveness checks and
ImageMagick's application-window capture; no image editing is used.

Full native application/package CI above is for `3d20f9bb`, before the footer
correction. Core release replay and package snapshots retain that revision label;
the footer does not change the measured core workload. The final UI/code-and-evidence
head receives its own CI. Release UI CPU/RSS, media throughput, physical displays
and owner-controlled Discord acceptance remain unmeasured or unverified here.

## Combined desktop parity — October 17, 2026

The combined validation branch is `integration/desktop-parity-20261017`. Its feature work remains in fork PRs [#7](https://github.com/MCShotty/Serein/pull/7)–[#15](https://github.com/MCShotty/Serein/pull/15); neither main nor parent #567/#620 is modified by this work. The final feature merges preserve the verified runtime source and only add current native evidence.

### Release reducer comparison

Compared combined code `eee7fd755bd6ed3d94a7d983040bb1564843a9a8` with current published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8df2136945d51bd16a1acd9c4033d88feb5`) and fork `f9e39f792b2fe0af02f2c1e791e1516147084824`. Rust 1.98.1, locked graph, Linux 6.18.55/glibc 2.41, five logical CPUs and about 17 GiB RAM. Each revision has a separate build directory: opt-level 3, thin LTO, one codegen unit, line tables, stripped symbols and panic abort. One warmup and five rotated measured samples per workload. The sampler uses CPU 0; other native build threads were restricted to CPUs 1–4. Those builds still share the memory system.

| Workload / elapsed median (range), ms | Nightly | Previous fork | Combined | Delta vs nightly / fork |
| --- | ---: | ---: | ---: | ---: |
| 100,000-event reducer | 71.035 (70.452–75.244) | 74.009 (72.001–77.601) | 78.581 (77.600–79.799) | +10.6% / +6.2% |
| 10,000-event wide-channel reducer | 117.102 (100.668–127.305) | 124.630 (119.000–157.747) | 128.122 (107.561–155.779) | +9.4% / +2.8% |

The reducer is slower than both baselines; the cause is unisolated. Wide-channel ranges overlap. These are combined feature measurements, not attribution to an individual PR, process RSS, UI latency, frame delivery or media throughput. The earlier run under unconstrained concurrent builds had substantially wider ranges and is excluded from the headline comparison. No speed improvement is claimed.

Raw elapsed samples in milliseconds:

```text
reducer/nightly: 71.035405, 75.243870, 70.567324, 70.452204, 73.810681
reducer/previous_fork: 75.435199, 74.008517, 77.601141, 72.086003, 72.000906
reducer/after: 77.937841, 77.600340, 78.580850, 79.180244, 79.798562
wide_channels/nightly: 117.102036, 111.678245, 100.667822, 127.304744, 126.957419
wide_channels/previous_fork: 125.465557, 119.000406, 124.630067, 120.726529, 157.747206
wide_channels/after: 128.121680, 107.561215, 119.048428, 129.088519, 155.779327
```

Reproduce with `python3 scripts/compare-replay.py --fork-base f9e39f792b2fe0af02f2c1e791e1516147084824 --output target/parity-replay.json`. Windows/macOS/Linux scoped CI runs the same comparison against the nightly selected at each run. Historical per-OS results above retain their original source revisions and regressions.

### Verification and native UI

`cargo xtask check` at `0e7c26e3` passes 1,919 tests with zero failures and 32 tests/examples ignored, formatting, strict workspace Clippy, the minimal application check and policy checks. Later changes to the verified runtime are demo-only. Demo all-target Clippy and the native preview build pass. Login-handoff, cached-xtask workspace, both license graphs, six license-policy fixtures, packaging fixtures, release replay and the 120-second soak pass. The soak completed 61,568 visits, 36,940,800 live inserts and eight logout cycles in 120.015 seconds within the core budgets. ASan fuzz smoke completed 128,905 decoder and 4,340 state-transition runs without a finding. None of these runs used Discord credentials, a live call, microphone, speaker or capture device.

Current unedited native screenshot pairs are in `docs/pr-evidence/`: application commands, desktop Inbox/messaging, desktop media, profile settings, community onboarding and community events. The baseline is the verified commands revision `1d47c3d`, not the nightly; after captures use combined code `80350d0f`, with the Stage/messaging follow-up at `92d5c311`. The new community fixtures and the pending synthetic block proposal are described beside their images. Native light/compact layouts, scrolling, explicit disclosure and keyboard settings interaction were inspected. Screenshots do not establish media throughput, frame latency, physical accessibility or official-client interoperability.

### Current scoped CI comparisons

The following completed jobs compare the exact pushed feature heads with the same nightly `3a4bf8d` and previous fork `f9e39f7`. They use the committed comparison script and matching release settings, one warmup and five rotated samples per workload. These are separate workers and scopes; do not compare absolute times between rows. Remaining current-head jobs are pending. Ranges overlap in most rows; no causal performance improvement is claimed.

| PR / revision / worker | Workload | Nightly / fork median, ms | Feature median (range), ms | Delta vs nightly / fork |
| --- | --- | ---: | ---: | ---: |
| [#10](https://github.com/MCShotty/Serein/actions/runs/39078123597/job/117529404730) / `1f87c10c` / macOS arm64 | reducer | 82.270 / 93.790 | 83.025 (68.906–102.941) | +0.9% / -11.5% |
| [#10](https://github.com/MCShotty/Serein/actions/runs/39078123597/job/117529404730) / `1f87c10c` / macOS arm64 | wide_channels | 114.380 / 108.980 | 116.055 (108.913–161.802) | +1.5% / +6.5% |
| [#14](https://github.com/MCShotty/Serein/actions/runs/39077843579/job/117528589940) / `17dd6ea6` / Linux x64 | reducer | 66.267 / 66.012 | 67.190 (66.914–68.239) | +1.4% / +1.8% |
| [#14](https://github.com/MCShotty/Serein/actions/runs/39077843579/job/117528589940) / `17dd6ea6` / Linux x64 | wide_channels | 53.462 / 75.568 | 53.429 (52.148–55.652) | -0.1% / -29.3% |
| [#9](https://github.com/MCShotty/Serein/actions/runs/39077840306/job/117528650041) / `2d7ad3ff` / Windows x64 | reducer | 57.631 / 58.102 | 57.794 (57.649–59.047) | +0.3% / -0.5% |
| [#9](https://github.com/MCShotty/Serein/actions/runs/39077840306/job/117528650041) / `2d7ad3ff` / Windows x64 | wide_channels | 61.728 / 61.415 | 65.329 (60.783–70.728) | +5.8% / +6.4% |
| [#9](https://github.com/MCShotty/Serein/actions/runs/39077840306/job/117528650153) / `2d7ad3ff` / macOS arm64 | reducer | 62.666 / 65.128 | 67.349 (63.345–76.996) | +7.5% / +3.4% |
| [#9](https://github.com/MCShotty/Serein/actions/runs/39077840306/job/117528650153) / `2d7ad3ff` / macOS arm64 | wide_channels | 96.200 / 101.236 | 98.480 (96.132–102.579) | +2.4% / -2.7% |
| [#12](https://github.com/MCShotty/Serein/actions/runs/39077838059/job/117528631664) / `df0bc231` / Linux x64 | reducer | 80.851 / 78.743 | 80.172 (79.051–81.817) | -0.8% / +1.8% |
| [#12](https://github.com/MCShotty/Serein/actions/runs/39077838059/job/117528631664) / `df0bc231` / Linux x64 | wide_channels | 163.075 / 163.410 | 163.266 (146.565–167.507) | +0.1% / -0.1% |

Raw samples, compiler/settings, OS and affinity are retained in each linked job’s `parity-replay` artifact. This is core reducer evidence; package size, UI/media performance and interoperability are separate measurements.

### Standard package comparison

The normal `cargo xtask package --format dir` completed from an empty output directory with the standard fat-LTO release profile, committed lockfile, voice enabled and one build job. Runtime code matches the verified combined tree; subsequent merges add evidence only. The package was not repaired or manually staged. All six private runtime libraries, corresponding-source archives and licenses passed validation. Packaged source archives match the recipe hashes. Dynamic RPATHs contain no development prefix, and the installed binary's bounded driver-query launch succeeds with both FFmpeg development directories removed from the runtime search path. This launch exits before credentials, UI, clients or capture initialize.

The comparison uses the official published nightly Ubuntu 26.04 x86_64 Debian payload (SHA-256 `ac36677a300133379f4fc4060a724bde4f4afc0261d757e1957b7f511f118d04`) versus the local Debian 13 x86_64 installation tree. Both are standard packages with voice. Normalized ZIPs use sorted regular files, fixed timestamps/modes and Python DEFLATE level 9. These whole-fork inventories include the existing FFmpeg/video work and corresponding source; they do not isolate parity changes or establish an application runtime comparison across the two build environments. The previous fork package was not rebuilt for this comparison.

| Metric / bytes | Published nightly | Combined clean package | Delta |
| --- | ---: | ---: | ---: |
| Executable | 87,746,736 | 95,345,800 | +7,599,064 / +8.7% |
| Installed payload | 92,280,770 | 140,176,288 | +47,895,518 / +51.9% |
| Normalized distribution ZIP | 53,552,488 | 89,776,426 | +36,223,938 / +67.6% |

Clean executable SHA-256: `eabce23a31d8ae290b95c9fea4c5fc60b9843bc36fd286d35b7380ddbc92cecf`. Clean normalized ZIP SHA-256: `db1693d48960579e45565fa04d802a95b5368aefc988fb23a89ef44d0c87c74f`. Native UI CPU/RSS, interaction/frame latency and media throughput are unmeasured. Windows/macOS package-size deltas remain unmeasured; their native checks and release reducer comparisons run in CI.

Two further normal package invocations used the same dependency cache and an empty output directory each time. The first cached run rebuilt the desktop crate after the final evidence merges touched its source timestamps; Cargo reported 15m 38s for that release build. The unchanged warm repeat reused the executable and reported 1.27s for the release build. These are Cargo build durations, not complete packaging or launch timings.

Both repeats passed the same library, RPATH, source-hash, license and development-path-free launch checks (exit 0). Clean, cached and warm outputs have identical executable and normalized ZIP hashes, installed byte counts and 280-file inventories. All runs used the ordinary packaging command, without a staging repair or modified cache fingerprints.

At final local verification, scoped current-head native checks remain incomplete. Commands PR #10's Windows x64 job timed out in the unchanged local-Gateway voice join/leave fixture, outside its added autocomplete dispatch path; the earlier native checks passed. GitHub rejected a rerun while the workflow was still running. Inherited `zip` advisories, Nix builds and openSUSE dependency setup failures are separate from the passing local gate. The combined branch receives fresh Windows x64/ARM64, macOS and Linux application/package checks on push; pending jobs are not reported as passes.

### Remaining gates

Parent main advanced independently to `611ff7f` during verification (dedicated Windows installer CI jobs removed). The fork baseline remains `f9e39f7`; no upstream synchronization is implied. Parent #567 remains at `1473d408` with 149 paths, and #620 remains draft at `4806c9bb`.

Current-head native/package CI on Windows x64/ARM64, macOS and Linux remains distinct from local Linux evidence. Physical devices, HDR/monitor transitions, audio synchronization, native assistive technology, RTL behavior and normal-account/official-client workflows remain owner-controlled. The supplied reference is Discord Stable 641004 (`c9ac31f`), Host 1.0.9262 x64 (`94186`); that client was not run here. SDK wiki preview pages were generated and checked from pushed media source `631699ad`, but parent wiki write access is unavailable and existing unrelated preview content must be preserved; no publication is claimed. The current parity inventory records supported, partial and handoff-only workflows; games and Activities remain excluded.
