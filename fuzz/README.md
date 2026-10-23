# Offline coverage-guided fuzzing

This separate development workspace exercises the real protocol conversions and core reducer.
It has no account, HTTP, Gateway socket, filesystem cache, GUI or audio adapter dependencies.
All committed seeds are handcrafted synthetic inputs. Never import captured service traffic,
credentials, signed URLs or private conversations into the corpus.

Install the pinned tools and fetch the committed dependency graph:

```sh
rustup toolchain install nightly-2026-09-09 --profile minimal
cargo install cargo-fuzz --version 0.13.2 --locked
cargo fetch --locked --manifest-path fuzz/Cargo.toml
cargo fmt --manifest-path fuzz/Cargo.toml --all -- --check
cargo xtask fuzz
```

Windows additionally needs the MSVC x64 tools and C++ AddressSanitizer component, with the
MSVC tools/ASAN DLL directory on PATH (Developer PowerShell for Visual Studio supplies it).
See the [official Windows setup](https://rust-fuzz.github.io/book/cargo-fuzz/windows/setup.html).
The project CI smoke job runs on Linux; local Windows results do not validate Linux or macOS.

`cargo xtask fuzz` checks cargo-fuzz 0.13.2, validates the lockfile offline, then runs each target
with AddressSanitizer, debug assertions and overflow checks. Each target stops after 30 seconds
or one million executions, whichever comes first. The time budget is checked between executions
and a run can finish slightly later; individual cases have a five-second timeout
and the fuzzer has a 512 MiB RSS ceiling. This is a developer-process limit, not client RSS.
Tool installation/compilation is outside the fuzz execution time; CI has a 15-minute job limit.

Each run copies committed seeds into a fresh generated corpus below `target/fuzz-smoke`, and
removes only that invocation's corpus on success or ordinary failure. Committed seeds are never
mutated. A killed process may leave a corpus directory for manual cleanup. The latest failing
input for each target is saved at `target/fuzz-smoke/<target>.crash` (at most about 4 MiB for
decode or 16 KiB for state). A later failure can replace it. CI uploads only these synthetic
failure files, with seven-day retention. Reproduce before adding a minimized regression seed:

```sh
cargo +nightly-2026-09-09 fuzz run decode target/fuzz-smoke/decode.crash
```

Cargo-fuzz has no `--locked` switch. The runner fetches with `--locked --offline`, runs Cargo
offline and checks that the fuzz lockfile is unchanged afterward. Updating it is deliberate;
keep shared dependency versions aligned with the main lockfile when upgrading. Run
`cargo xtask licenses` after fetching both graphs; the normal and fuzz graphs are both checked.
Neither libFuzzer nor its instrumentation is linked into the application.

## Targets and limits

`decode` consumes a selector byte and up to `MAX_WIRE + 1` payload bytes. Selector 8 covers bounded saved-message parsing, selector 9 covers AutoMod rules and selector 10 covers ban pages. The original eight branches cover
message/patch model conversion, READY navigation/users, permission snapshots, thread sync,
presence normalization and channel/full-patch conversion. Selector 255 derives a valid JSON
message padded to the 4 MiB boundary, then verifies rejection one byte above it, so large seeds
are unnecessary. Parse success does not imply admission into the separate timeline policy.

`state-transitions` consumes at most 16 KiB and 256 eight-byte operations. It starts with three
small synthetic guild channels. Operations select/history/load/patch/delete, replay stale
requests or generations, revoke/restore permissions, reconnect, logout/re-READY and clear caches.
Flags generate null/absent patches, reply-deletion markers and page/bulk-delete pressure. Payload
expansion is limited to 24 MiB per iteration and reaches real message/window eviction boundaries.
Reply operations also retain one-level summaries, schedule visible lookups, apply successful,
failed or stale outcomes, jump to originals and return to the preceding window. A synthetic
decoder seed includes an embedded original with a nested reply that must be ignored.
Oracles check scoped ordered unique rows, deleted-body safety, 500 rows/4 MiB per timeline,
two dormant windows, 1,475 aggregate rows/16 MiB, permission visibility and stale-state isolation.
See the operation encoding in the target when creating a seed; no real clock is manipulated.

The seeds are replayed during corpus initialization before coverage-guided mutation. A passing
smoke run establishes only that these seeds and generated inputs found no failing oracle during
that run. Coverage counters are instrumentation feedback, not a whole-repository coverage
percentage, proof of absent bugs, native UI evidence or live Discord compatibility.

Command parity also exercises the bounded autocomplete-choice and private-context account-index parsers (selectors 11 and 12). Seeds are synthetic and contain no account or service traffic.

Soundboard catalogs, administration rows and Gateway effects also use synthetic seeds
(selectors 13 and 14); successful parsing does not establish live soundboard support.

Personal channel preferences use selector 16, including unknown raw flags and partial
settings snapshots. Selector 17 exercises bounded access settings, member applications and
service-provided incident updates. State operations 25 and 26 reconcile channel selections
and accepted changes, including stale-generation responses; the fingerprint includes retained
preference flags. These paths use synthetic identifiers and never contact Discord or mutate
an account.
