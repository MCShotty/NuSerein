# Bug audit

The audit starts at merged fork main `1e8123c0`. The repository contains over
800 Rust files as well as native code, scripts and packaging. This record
does not claim the whole repository has been reviewed.

## Confirmed fixes

- Gateway queue admission removed pending updates before checking whether a
  replacement fit. A rejected presence update or subscription cancellation
  could therefore discard earlier work. The regression checks both byte-limit
  failures and confirms that the original packets remain queued.
- Wheel events preceding a click in the same input frame could restart motion
  after the click. The regression covers wheel and touchpad input with smooth
  scrolling enabled and disabled, in both event orders.
- The typing indicator animated even when the egui style disabled animation.
  The regression checks static dot appearance and repaint deadlines.
- When startup was unavailable, the nested Start minimized switch was dimmed
  twice. Both switches now use the same disabled text treatment without
  changing when either can be used.
- Forum cards centered short titles inside their reserved width. List and
  gallery titles now align with the unread marker while preserving badge space.
- Image MIME types with parameters were not classified as images. The model
  regression covers parameters, whitespace and non-image types; the gallery
  fixture verifies that such an image is displayed and can be opened.
- Reply previews lost the image classification for HEIC files whose MIME type
  was `application/octet-stream`. The regression also checks that the preview
  does not retain the original media URL.
- Extreme poll counts overflowed while rendering a closed-poll card. The
  rendering regression covers zero votes, normal ratios, clamping and the
  largest input values.
- Custom-status expiry used the current offset for future deadlines. The
  regressions cover spring and fall transitions and a skipped midnight.
- Image placeholders with padded or unpadded Base64 could be rejected. The
  decoder regression covers attachment and embed thumbnails, malformed
  padding and the 64-byte limit.
- The incoming voice jitter queue discarded valid multi-frame Opus packets
  larger than 1,275 bytes. A 120 ms packet now passes through the actual mixer
  while the aggregate queued-payload budget remains unchanged.
- Member-identity updates accumulated obsolete message IDs even when no
  history page was loading. The regression rotates three timeline windows and
  checks that in-flight pages still preserve newer nicknames.
- A delayed history page could overwrite newer member-list nicknames and role
  assignments. Reconciliation now retains only the changed member fields,
  allowing newer message content to load. Tests cover cached pages, range
  navigation and bounded tracking as well as the reducer's member-list path.
- A partial member profile replaced the whole message-author record and could
  erase existing avatars or cosmetics. Member-list and timeline updates now
  share the same partial profile merge. Explicit identity clears also survive
  delayed history without suppressing newer message bodies.
- Omitted `global_name` values were converted to usernames and overwrote known
  display names. Partial identity updates now distinguish omission, null and a
  supplied value. Active Gateway member lists retain bounded name provenance
  across row updates and reordering, including the case where a global name
  matches the old username. Explicit clears still follow later username changes.
- Initial member lists, thread lists and member-search results now retain that
  distinction. Reconciliation keeps newer names through delayed searches,
  updates already-loaded messages, and keeps fallback names working across
  successive 200-member snapshots and overlay eviction. Disk hints do not
  establish permission or authoritative name provenance.
- Force Sync now carries omitted and explicitly cleared global names through
  REST and Gateway roster reads. Incomplete records remain visible without
  replacing a confirmed SQLite display name, and rejected member results are
  not written to disk. Member-list snapshots also avoid caching an omitted
  name as though Discord supplied the username as a display name.
- Member-search results now keep an omitted name out of the confirmed display
  cache while retaining the nickname; an explicit name removal is still saved.
- Large history-range cleanup used to delete a batch before checking whether
  the incoming page could be serialized. A rejected page could remove 128
  cached messages. Large cleanups now validate the page before their first
  committed deletion; a valid retry still completes the cleanup.

The Gateway, scrolling, typing, image, reply, poll, status, placeholder, voice
and identity regressions failed before their respective fixes. The
member-list races, partial-profile, omitted-name and Force Sync probes also
failed before their fixes. The gallery fixture and the skipped-midnight case
were extended afterward. The tests use synthetic data and do not contact
Discord or capture devices.

## Review coverage

The inspection so far has focused on the Gateway writer and its queue tests,
member identity reconciliation, selected REST rate-limit and sync paths,
SQLite history and cache-budget paths, selected extension validation and
sandbox paths, and the UI's scrolling, typing, search-pager, status time and
attachment-gallery paths. Native file saving, Windows capture startup, voice
jitter handling and receive format parsing were sampled by source inspection
only. These are targeted inspections, not complete reviews of those files or
subsystems. The remaining application, voice, platform, extension, tooling,
packaging, workflow and vendor code still needs review.

A repository-wide search of first-party Rust sources found no `TODO:`, `FIXME:`,
`todo!` or `unimplemented!` markers. That mechanical search is not a correctness
or security audit. A search for decorative comment banners, numbered steps and
generic labels found one false positive: the `WSAEMSGSIZE` comment records a
Windows-specific behavior and was retained. This is not a full comment review.

The Anti-Slop core, UI and code guidance was applied to the changed comments
and inspected interactions. An offline virtual-display preview at 1120 × 760
shows the unavailable startup switches [before](startup-before.png) at
`cb506cb2` and [after](startup-after.png) at `6951883b`. In the same synthetic
preview, clicking the tray switch changed its state and Escape closed settings.
A compact light-theme conversation and mention picker were also inspected.
These checks used software rendering, not Discord or native device controls. A
complete UI delivery gate remains open because the full interface has not been
visually inspected or exercised here.

An offline demo with software rendering showed the forum list at 1120 × 760
[before](forum-before.png) at `fff246c8` and [after](forum-after.png) at
`67743f64`. The [dark, narrow view](forum-dark-narrow.png) was inspected at
700 × 700. In that demo, the following interactions were exercised:

- Browse Channels opened; the Muted filter showed the muted channel.
- Searching for a forum while Muted was selected showed no match; clearing the
  filter showed the forum, and its row opened it.
- Sort & View opened the gallery; Escape closed its menu. New Post opened an
  empty composer with Post disabled, and its close control dismissed it.
- Appearance opened from the settings control; Dark changed the theme, and
  Escape closed settings.
- Removing a channel unchecked Show All and made its category mixed; selecting
  it again restored the channel. Tab and Enter selected a channel-list filter.

Seven additional programmatic demo checks covered member lookup, suggestion
clicks, settings sliders, forum navigation and menus, the conversation switcher
and forwarding. These are synthetic checks, not live Discord acceptance. The
first preview logged an X11 error after its test window was forcibly destroyed;
the later preview logged no errors during interaction and was ended by signal.

Force Sync and member-list name handling are covered by synthetic fixtures;
live Discord synchronization and the performance of large snapshots have not
been measured here. Large history cleanups still use multiple transactions; a
later storage failure can leave a partially cleaned, verified range.

## Validation limits

At source revision `80f07a92`, the protocol suite passed 148 tests and the UI
suite passed 849 tests with 10 ignored. The local-store and model library
suites passed at `9c5ab81a`; 108 Gateway tests passed at
`52569046`, and 41 reply tests passed at `8bd54424`. The voice suite passed
238 tests with four ignored at `9b4ec0ca`, including synthetic stream fixtures.
At `9f43824d`, the core suite passed 449 tests with two ignored, and the
session-cache suite passed 13 tests with two ignored. Synthetic login-handoff,
cached-xtask workspace, and two Linux packaging fixture tests, formatting and
strict workspace Clippy also passed at that revision. The full workspace check
and native CI are tracked in the PR. At `fd8541bf`, 451 core tests, 74 model
tests and 17 session-cache tests passed, with two ignored in each of the core
and session-cache suites; strict Clippy passed for those crates. Repository
policy and license checks passed at `03f1179d`. An older local
FFmpeg build initially failed the webcam reference fixture because it lacked
`hqdn3d` and `unsharp`; the fixture passed after rebuilding the project's pinned
FFmpeg recipe with those filters. At `fe93c25c`, 452 core, 109 Gateway, 149
protocol, 74 model and 17 session-cache tests passed, with two ignored in each
of the core and session-cache suites. Eight focused member-list tests, strict
workspace Clippy, the fuzz-target compilation and 849 UI tests also passed;
10 UI tests were ignored. At `cb506cb2`, the split workspace run passed 2,689
tests across 30 targets, with 35 ignored. Formatting, synthetic login handoff,
cached-xtask workspace/package fixtures, policy, license checks and strict
workspace Clippy passed. FFmpeg bundle fixtures passed 20 tests with five
skipped; Flatpak preparation and Linux package fixtures passed four and two
tests respectively. The exact `cargo xtask check` stopped during
`cargo test --workspace`: the container killed the linker for the UI
`autoscroll` example after Clippy passed. The split workspace test run above
does not include examples or doctests. At `6951883b`, the three startup UI tests,
focused strict Clippy and state-transition fuzz-target compilation passed after
the visual fix. At `4c1c26fd`, 459 core tests passed with two ignored, 151 API
tests passed with one ignored, 110 Gateway tests passed, and 19 desktop Force
Sync tests passed. Formatting and strict Clippy passed for the changed crates;
the state-transition fuzz target compiled. At `86e55c5f`, 462 core tests passed
with two ignored, and focused strict Clippy passed. At `f608babc`, 97 local-store
tests passed with two ignored, strict Clippy passed for that crate, and the
HTTP-to-SQLite Force Sync fixture passed. At `67743f64`, 849 UI tests passed
with 10 ignored, strict Clippy passed for UI, Gateway and the demo application,
and the seven programmatic demo checks passed. No fuzz campaign was run.
Headless egui tests
exercise the affected interaction paths; this environment has no physical
display for a visual or assistive-technology review. Live Discord, Windows
capture, microphone and real-device behavior remain owner-controlled and
unverified. Release replay measurements are in
[`docs/performance.md`](../../performance.md#bug-audit-replay-comparison).

The release replay binary built at `8bd54424` completed a 120-second synthetic
lifecycle soak: 3,087 passes, 98,784 channel visits and 13 logout cycles. It
predates the identity-cache fix. The binary built at `9f43824d` completed the
same 120-second soak with 2,921 passes, 93,472 channel visits and 12 logout
cycles. Other build work ran concurrently with both, so the pass counts are not
a performance comparison. The soak does not exercise network, storage,
rendering or devices.

At `cb506cb2`, the 120-second synthetic lifecycle soak completed 3,787 passes,
121,184 channel visits and 15 logout cycles. The preview build ran concurrently;
these counts are not a performance comparison.

At `cb506cb2`, the Linux Nix CI job failed while downloading the pinned FFmpeg
tarball: the connection to ffmpeg.org was reset. The same download failure
occurred at `66f78e54`. That earlier revision's macOS Nix job also failed to
link `___isPlatformVersionAtLeast`; the current macOS Nix result is pending.
The current macOS native job passed. Windows ARM64 failed the GPU webcam
denoising fixture with `DenoiseError`, as it did at `66f78e54`; the cause has
not been established here.
