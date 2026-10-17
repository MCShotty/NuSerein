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
