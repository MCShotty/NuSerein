# Chat and member loading evidence

The baseline is PR #31 `af865308` (application source `1607575e`). This follow-up is
stacked on #31 as PR #32 and leaves both main branches unchanged. Source `9b1e29c5`
adds bounded member display persistence, session identity/range reuse, safe stale
conversation previews, worker-admission retry and scoped storage cleanup.

## Behavioral boundaries

- Cached names never authorize actions or establish presence/current membership.
  A new independent, bounded Gateway watch keeps observed names current even when
  no additional member query is required. Discord's query cooldown is preserved.
- A dormant chat changed by background traffic is immediately available as a stale
  preview and still needs authoritative history. Read acknowledgement remains separate.
- Storage epoch fences and FIFO cleanup prevent earlier reads/saves from restoring
  deleted or inaccessible data. Parent-channel loss includes child thread windows;
  unrelated channels/accounts are retained. Unknown access or failed cleanup retains
  conservative account-wide clearing and the existing failure state.
- Schema 30 stores names/usernames/nicknames, bounded globally to 4,096 / 2 MiB.
  Clear cache and logout delete these hints. Unchanged persisted message projections
  and identical member snapshots produce no row writes in SQLite trigger regressions.
- Disk hints predate in-flight live requests. A later role-only update cannot promote
  an old cached nickname into a newer observation and overwrite the live response.
- Session recovery resets the member-load/save admission markers with the RAM cache,
  so an accepted earlier read cannot permanently prevent reloading names. A completed
  read rejected after newer live observations may retry on worker progress, without
  a repaint timer or weakening account, epoch and live-revision guards.

## Verification status

Pinned Rust 1.98.1 and committed locks. `cargo xtask check` passed on `d854f5fe`:
formatting, strict workspace Clippy, 2,455 passing tests, minimal application build
and policy checks. This includes core (386), Gateway (95), local-store (48) and
UI (826) regressions. Login-handoff, cached-xtask workspace/packager, FFmpeg, Flatpak
and Linux packaging fixtures pass. The FFmpeg fixture reports four capability skips.

Release replay, the new loading workload and the 120-second soak pass: 120.021 seconds,
2,403 passes, 76,896 channel visits, 46,137,600 inserts and ten logout cycles. Retained
core-history estimates peak at 14,364,789 bytes under the existing 16-MiB budget.
Bounded fuzz runs pass: 173,644 decoder cases and 3,798 state cases, 31 seconds each.
License-policy fixtures and native license CI pass; locks/dependency versions are unchanged.
The desktop recovery follow-up adds two SQLite/state/admission regressions and leaves
the measured reducer/model source unchanged. Its final local/native check outcomes,
normal packages and installer checksum/provenance are recorded in PR #32.

Production-path regressions cover repeated switching, cached sidebar chunks, pane
reopening, explicit nickname/role removal, delayed identity results, account/access
fences, cached-member WebSocket subscriptions, member-query cooldown, stale target
navigation, queue admission, scoped cleanup races, migration, bounds and write counts.

## Measurements and unavailable evidence

The [performance record](../../performance.md#conversation-and-member-loading) includes
both matched loading batches, availability counters and their limitations. The common
fixture uses the real reducer and public selection/lookup paths. Synthetic held responses
establish preview availability while waiting; they do not measure real HTTP latency.

Existing layouts are retained except the mention picker's cooldown explanation. This
workspace has no graphical display, Xvfb or native Windows/macOS input session,
so no native screenshots, presented-frame latency, idle CPU/RSS or live Discord
acceptance are claimed. Owner acceptance should repeat busy channel navigation, scrolling
and reopening the member pane, restart with a warm cache, and cross-client rename/removal
and access changes, including reconnects. No owner account or private content was used.

Current source CI's dependency audit fails on the unchanged locked dependency graph:
crypto-bigint/ctutils/pulldown-cmark advisories, a Tokio warning and a yanked clipboard-win
warning. The pulldown-cmark advisory was newly reported during this task; the dependency
was already present. Linux Nix fails downloading the unchanged FFmpeg source archive.
These are separate from the passing license checks. Other native outcomes and any installer
limitations are recorded in PR #32 rather than described as passing in advance.
