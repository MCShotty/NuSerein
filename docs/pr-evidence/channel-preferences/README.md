# Channel preferences and Browse Channels

The starting installer is `d11d2aee92225474731809320e958218fac28df2`. Runtime changes
are in `52836c95fd091dc008ea7765163e9c5945314548`; the later evidence commit changes
no application code. PR #27 is stacked on #26 and neither is merged.

## Behavior and coverage

Gateway settings and REST confirmations now enter the same reconciliation path.
It distinguishes complete records, sparse fields and explicit nulls, retains unknown
flag bits, and fences responses by account generation, request and observed/server
ordering. Category membership reaches children and threads; removing an inherited
child preserves siblings. Saving overlays confirmed values immediately, coalesces
for 250 ms and uses one mutation lane. Pending state is limited to 1,024 entries /
256 KiB within the existing notification budget. Rejected saves roll back; uncertain
writes remain unconfirmed and are not automatically replayed. Refresh uses an empty
PATCH to the unofficial normal-account guild-settings endpoint.

The browser occupies the main content pane. Selection and navigation have separate
hit targets, categories have a tri-state checkbox, and saved membership is separate
from Show All, muting and temporary visibility. It shares Back/Forward history and
preserves the conversation. Saving and recovery feedback stay beside their targets.
Stable row identities preserve keyboard focus across saving feedback and reflow.

Production-path regressions cover decoding/reconciliation, stale REST/Gateway
orders, sparse/full/null records, unknown bits, inherited categories, conflicts,
uncertainty, cancellation, account changes and transport admission. UI tests cover
membership versus navigation, keyboard activation and reflow, main-pane routing,
no browsing acknowledgements, filters and narrow/scaled layouts.

## Native visual evidence

`before.png` and `after.png` are actual synthetic native application captures on
Linux x86_64 under isolated Xvfb, Mesa llvmpipe, 1280 × 900, scale 1, dark theme,
with fresh temporary demo data and no Discord adapter. The matching release builds
use Rust 1.98.1, opt-level 3, thin LTO, one codegen unit and stripped symbols.

| Capture | Source | Preview SHA-256 |
| --- | --- | --- |
| Before | `d11d2aee` | `aa68c975b0de168aa8b1e1c8ac5b824bfbd2e3e5092b73628d4c44414d301958` |
| After | `52836c95` | `080554a7d95dd94a61d32b649f26f37bb0c475880f2a160592791797f4d93306` |

Both use the existing channels fixture. The new fixture supplies the omitted
Hide Muted value as false to demonstrate loaded controls; unknown preferences have
separate regressions. The old modal obscures the navigation; the new page keeps
navigation and account controls visible. Native narrow dark and light 200% layouts,
checkbox actions, search and overflow were inspected. Keyboard focus/reflow and
Back/Forward semantics are also covered by deterministic application/core tests.
Native assistive-technology checks and physical Windows/macOS input remain unverified.

## Verification and performance

The complete local `cargo xtask check` and strict demo Clippy passed on `f8d99c95`.
The subsequent notification-path optimization passed all 369 release core tests,
strict core Clippy and formatting. Login-handoff, cached-xtask workspace, license
and packaging fixtures passed. Both bounded fuzz targets passed on `f8d99c95`;
current-head native CI repeats the full gates and fuzz checks.

Release `cargo replay` and `cargo replay --soak 120` passed on `52836c95`: 2,684 passes, 85,888
visits, 51,532,800 inserts and 11 logout cycles in 120.031 seconds. Retained-history bounds held.
Repeated native Tab/Space activation, Back, search and overflow passed in the release fixture;
narrow dark and light 200% captures were inspected. Packaging checks and current-head CI outcomes
are recorded in the PR.

See [performance measurements](../../performance.md#channel-preferences-and-browser--october-26-2026)
for release methods and results. Final native CI, package comparisons and installer
provenance are linked in [PR #27](https://github.com/MCShotty/Serein/pull/27).
Historical measurements elsewhere retain their original source labels.

## Owner-controlled acceptance

Synthetic tests establish neither live Discord synchronization nor hardware/input
acceptance. In the owner's session, check changes in both directions between
Discord and Serein: hide/show a channel and category, remove an inherited child,
restore siblings, toggle Show All/Hide Muted, and change timed mute/notification
settings. Verify a hidden active/pinged channel remains discoverable without
changing the saved selection. Repeat after reconnect and account switching.

No live Discord preference writes, owner accounts, calls or microphone capture
were used during automated verification. The inherited dependency-audit findings
remain outside this unchanged dependency graph. Native/display limitations and
current CI outcomes are stated in the PR.
