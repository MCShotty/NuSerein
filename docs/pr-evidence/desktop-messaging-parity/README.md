# Desktop messaging follow-up

This scoped follow-up starts from Inbox commit
`67a1647cf001f2b18ac9c82d677b8e86323d580b` on
`feat/desktop-parity-messaging`, with an initially clean worktree. The remote default
is `origin/main`; this branch deliberately keeps the Inbox dependency. Rust 1.98.1
and the committed lockfile are used. The common test-only Gateway race fix is
cherry-picked as `2ca4e6fb`; it changes no production Gateway behavior.

Implemented workflows:

- Blocked/ignored timeline bodies, thread starters, loaded replies, search/pins,
  Inbox previews and forwarding require explicit disclosure. Hidden bodies do not
  enter media/artwork renderers; source/account/restriction changes revoke disclosure.
  Queued alerts recheck the author, while another group member can still notify.
- Group creation selects 2–9 friends and admits the exact returned group. Members
  load on demand, invitations check capacity, and removals/owner transfers require
  confirmation. A failed or uncertain membership write requires an explicit refresh.
- Public/private thread creation correlates the returned ID, private invitations and
  creator/moderator removals use current permissions, and the native member list
  refreshes after confirmed changes. Private/public/joined-private archives retain
  their existing separate read permissions and bounded paging.
- Forum tag editing enforces five tags, required selections and moderated additions
  and removals. Concurrent changes require explicit reload; failures preserve drafts.
- Announcement publication has a confirmation; following chooses one known channel
  with Manage Webhooks. Both require exact response identities, with no mutation retry.

The broader reply/edit/upload/reaction/poll/pin/request/search/read/navigation review
remains partial. Existing paths and their regressions are retained; this change does
not claim full desktop messaging parity.

All checks use synthetic data or loopback HTTP, with no Discord traffic, real account
credentials, microphone or media device. Completed checks:

| Check | Result |
| --- | --- |
| Core/API/UI library check, locked | Passed |
| Full core suite, one test thread | 214 passed, 2 manual workloads ignored |
| Full headless UI library suite, one test thread | 588 passed, 9 manual workloads ignored |
| Channel action HTTP tests | 7 passed |
| Group action HTTP tests | 3 passed |
| Core/API/protocol/UI/test-support Clippy, all targets, warnings denied | Passed |
| Format and diff whitespace checks | Passed |

The final fingerprint-only follow-up replaces egui ID hashing with a standard hasher
to avoid its debug source registry. Scoped Clippy covers that final source; the full
headless UI run preceded this follow-up. Full workspace
`cargo xtask check`, the standard package and release replay/native measurements
remain in the coordinated integration pass. Native screenshots, the standard package,
and release before/after measurements are pending the parent task's coordinated
native build/evidence pass. No old executable or headless shape output is presented
as native screenshot or performance evidence.

The owner's reference is Stable 641004 (`c9ac31f`), host 1.0.9262 x64 (`94186`).
Official desktop comparison, normal-account acceptance, cross-account private-thread
visibility, group ownership transfer, entitlements and Windows/macOS interaction
remain owner/live gates. The group recipient mutation route is unofficial for a
normal session; Discord's public route describes an OAuth flow. Pinned protocol
sources and request/recovery bounds are recorded in
[compatibility](../../discord-compatibility.md) and
[storage policy](../../storage-policy.md).

## Combined native verification

The combined feature tree passed `cargo xtask check` at `0e7c26e3`: **1,919 tests passed**, 32 tests/examples ignored, strict workspace Clippy, formatting, the no-development-data application check and policy checks passed. Demo all-target Clippy and the native preview build also pass after demo-only follow-ups. Login-handoff, cached-xtask workspace, license checks for both lockfiles, six license-policy fixtures, the release replay and the 120-second soak pass. Bounded ASan fuzz smoke completed 128,905 decode and 4,340 state-transition runs without a finding. These checks use synthetic data and local peers, with no Discord session or physical audio/capture.

The combined release reducer comparison at `eee7fd75` uses published nightly `v1.0.0-nightly.20261014.58` (`3a4bf8d`) and unchanged fork main `f9e39f7`: one warmup and five rotated samples, Rust 1.98.1, opt-level 3, thin LTO, one codegen unit, panic abort and stripped symbols. Native build threads were kept off sampling CPU 0. Reducer medians were 71.035 / 74.009 / 78.581 ms (nightly / fork / combined): **+10.6% / +6.2%**. Wide-channel medians were 117.102 / 124.630 / 128.122 ms: **+9.4% / +2.8%**, with overlapping ranges. Cause is unisolated; this measures all sibling features together, not this PR alone, UI/media throughput or process RSS. Current-head per-OS CI remains separate.
