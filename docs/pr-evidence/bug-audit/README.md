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

The Gateway, scrolling, typing, image, reply, poll, status, placeholder, voice
and identity regressions failed before their respective fixes. The
member-list race and partial-profile fixture also failed before their fixes. The gallery fixture and the
skipped-midnight case were extended afterward. The
tests use synthetic data and do not contact Discord or capture devices.

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
and inspected interactions. A complete UI delivery gate remains open because
the full interface has not been visually inspected or exercised here.

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
FFmpeg recipe with those filters. Headless egui tests
exercise the affected interaction paths; this environment has no native
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
