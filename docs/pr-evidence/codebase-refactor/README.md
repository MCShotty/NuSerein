# Codebase simplification review

## Baseline and scope

Fork PRs #20, #21 and #22 were merged in dependency order with ordinary merge
commits. The frozen merged baseline is `6d71c738d6e4d48bb91599658e0f1048b74656be`;
its tree is identical to the previous complete implementation, `be2f6eca`.
Backup references preserve fork main and all three starting PR heads. The parent
repository was not changed. Refactor implementation revision: `fd181b4e`.

The review inventory covers 781 first-party source/configuration paths, including
Rust, native adapters, extensions, fixtures, tools, packaging and workflows.
Inventory/reference analysis covered every path; detailed implementation review
concentrated on ownership boundaries, shared helpers and lifecycle/order-sensitive
code. This is a structural review with regression testing, not a claim of a
line-by-line correctness or security audit. Every file need not change.

## Subsystem findings and disposition

| Subsystem | Finding and resulting disposition |
| --- | --- |
| Desktop (93 original paths) | One entry point mixed construction, preferences, account events, commands and frames. Private implementation modules now separate those responsibilities while retaining one `Desktop` owner. Offline command dispatch preserves its original two phases, feature gates and order. Teardown and persistence policy remain explicit. |
| Core and session cache (66) | Contracts and lifecycle domains were interleaved with reduction. Command/event exports remain at the crate root; startup, navigation, history, outgoing and member-list implementations have domain modules. Incoming messages, send results and access cleanup use small helpers at the same reducer positions. Session-cache eviction/mutation bookkeeping already has a coherent owner and remains unchanged. |
| UI (171) | Composer, panel coordination and frame orchestration were mixed. Their methods move unchanged. Timeline row painting borrows the existing frame context inside the same UI scope, preserving widget allocation order, focus and scroll reconciliation. Twenty-one matching test helpers are replaced by logical/visual bounds variants; their different geometry policies remain separate. Existing navigation/media/menu helpers stay with their owners. |
| REST, Gateway and protocol (120) | REST admission/error policy is separated from endpoint families; root public paths remain. The desktop connection separates reliable-event admission, bounded read/mutation lanes, presence and voice lifecycles from session startup. Gateway's heartbeat/read/recovery state machine remains intact: introducing another dispatcher would obscure its ordering guarantees. User/channel DTOs move with their exact conversion behavior. |
| Models (60) | Identity, navigation, messages, presence and member records move into cohesive modules with their existing root exports. Public fields, serde attributes, validation and retained-byte calculations are preserved. Other domain DTOs already have owners and remain unchanged. |
| Storage (4) | Preferences, schema/migrations, history and account operations move into private modules. SQL, transaction boundaries, migration order and schema version 29 remain unchanged. |
| Voice and platform (129) | Live-video, packet-audio and attachment FFmpeg contexts now have separate worker-owned implementations. FFI declarations, shared leases and callbacks remain together. Capture/conversion/encoding/presentation already use distinct interfaces; consolidating them further would mix GPU/resource lifetimes. Native sources, ABI, security-transition handling, capture backpressure and encoder/HDR policy remain unchanged. |
| Extensions (host 24; SDK 14; plugins 8 and tools) | Host manifest/invocation/output validation moves out of the wire-record module without changing validation order. Keep deliberate host/SDK compatibility mirrors, capability checks and sandbox limits separate. SDK API/wire revisions and plugin binaries are unchanged; no author-visible SDK or wiki change is needed. |
| Tests, examples, fuzz and developer tools | Existing assertions/fixtures/platform variants are retained. Add a stale-startup-after-logout regression before extraction. Preserve replay/fuzz workloads and policy checks; actual native-persistence methods/options remain where the policy inspects them. |
| Scripts, packaging (26), workflows and configuration | Keep platform-specific preparation, signing, library-closure and source/license assembly distinct. Reuse the identical replay baseline environment expression and freeze this branch's comparison revisions. No package allowlist, dependency version, feature default, license text or package source is changed. |
| Serein vendor patches | Reviewed all six patch records and their scoped manifest/native/HDR/mipmap/SHAKE adaptations. Retain these upstream compatibility boundaries and their removal conditions; no vendor source or notice changes. |

## Equivalence checks

- Public model/protocol/core/store/extension record comparison retains 745 records
  and their attributes. The only declaration difference is `PreparedStartup`'s
  internal fields becoming `pub(super)` to retain their former ancestor access.
- Token-level function-body comparison matches 11,053 bodies. Reviewed exceptions
  are the core helper calls, desktop delegates/CLI dispatch, timeline row extraction
  and shared test-helper callers. The parser predates some native Rust syntax, so
  these counts are supporting evidence, not a universal equivalence proof.
- Parsed assertion sites remain present: 18,382 before, 18,388 after. The six added
  assertions cover a delayed prepared startup and warnings after logout.
- Main/fuzz lockfiles, workspace dependency manifests, native capture/codec sources,
  assets, vendor sources and the extension SDK are unchanged. Native decoder FFI
  records and C/header definitions remain at the original interface.
- Startup generation admission still precedes applying startup. Access reconciliation
  still precedes the resident-budget pass. Incoming/outgoing reconciliation, event
  FIFO permits, mutation serialization, cancellation and uncertain-write handling
  preserve their original statement order.

## Appearance

Not applicable — no visible UI change. Representative native synthetic views are
inspected separately for the conversation, Language & Time and server Members views;
historical screenshots retain their original revision labels.
No redundant screenshot pair or raw logs are added to the repository.

## Validation and performance

The pinned workspace gate passed at `fd181b4e`: 2,218 tests, 36 default-ignored
cases and one additional locale child-process check. Formatting, strict Clippy,
the minimal application build and policy checks passed. Strict demo/debug Clippy
also passed. Follow-up `7f9e7f4` aligns offline-check imports and a private check
with their existing debug-only caller; strict release/demo Clippy passed afterward.
Those compile guards are absent from the production binary and UI preview inputs.

Login-handoff, provider-player, cached-workspace/packager, dependency-license,
extension catalog/SDK compatibility, native capability/encoding and applicable
FFmpeg/LavaSR/Flatpak/Linux/Windows-update/macOS-signing fixtures passed. The
Windows behavioral installer fixture requires Windows and was not run locally.
Optional post-GTK Flatpak locale and software HDR/SDR/mipmap rendering checks passed.
The two ASAN fuzz targets completed 121,630 and 7,779 runs without an oracle failure.

Release replay and the 120-second lifecycle soak passed. The soak performed 2,757
passes, 88,224 channel visits and 52,934,400 live inserts, with 11 logout cycles;
retained-history estimates remained within the existing budgets. These are state
estimates, not process RSS or media throughput.

Normal Linux directory packaging started from absent output directories for both
clean and cached invocations. Their contents and modes match exactly. Both passed
codec and LavaSR self-tests without development FFmpeg/LavaSR environment paths.
The executable's only runpaths are `$ORIGIN/lib` and `$ORIGIN/../lib/serein`.
The installed payload differs only in the executable; libraries, models, source
bundles, notices and file membership match the baseline. See the release measurements
in [Performance](../../performance.md#codebase-simplification).

No synthetic test establishes live Discord acceptance, physical display/capture
performance, microphone behavior or native HDR output. Full-desktop asynchronous
task occupancy, media throughput and physical Windows/macOS UI/input performance
remain unmeasured; regression tests retain queue/cancellation/resource bounds.

Commands used include `cargo +1.98.1 xtask check`, strict debug/demo and release/demo
Clippy, `node tests/login-handoff.cjs`, `cargo replay`, `cargo replay --soak 120`,
`cargo xtask licenses`, the affected fixture scripts and normal `cargo xtask package`
directory packaging. The performance document identifies the distinct build profiles,
revisions and measurement limits; CI artifacts contain the native replay samples.

## Existing review candidates kept separate

| Finding | Disposition |
| --- | --- |
| [Voice-lifecycle Inbox cancellation](https://github.com/MCShotty/Serein/pull/23#discussion_r4365511106) | The watch and command branches already have different cancellation sets in frozen `6d71c738`. Preserve them here; whether Inbox reads should follow voice interruption needs a separate behavior decision. |
| [Autocomplete rejection](https://github.com/MCShotty/Serein/pull/23#discussion_r4365511130) | The rejected autocomplete path already falls through to generic stale handling in the frozen baseline; cancellation also lacks its own branch. A behavior fix belongs in a separate change. |

The import-guard finding was addressed. The suggested non-demo unused-argument
case does not apply: the entire offline-check module is gated by the demo feature.

## Native CI and inherited failures

The [runtime implementation workflow](https://github.com/MCShotty/Serein/actions/runs/40049052574)
passed Windows x64/ARM64, macOS and Linux application/package jobs, extension compatibility,
fuzz, license and all three replay-comparison jobs. The security audit flags the unchanged
ctutils/tokio advisories and yanked clipboard-win package. No dependency was upgraded or
advisory suppressed for this refactor.

Linux Nix passed. macOS Nix still fails on `___isPlatformVersionAtLeast` from
`_serein_capture_luminance`, matching the same native object in the
[baseline run](https://github.com/MCShotty/Serein/actions/runs/40017479085) and
[implementation run](https://github.com/MCShotty/Serein/actions/runs/40049052617).
The ordinary macOS build/package job passed. Final documentation/profile-guard-only CI
reruns are reported on the PR rather than being confused with this runtime revision.

## Benchmark clock

The offline preview's original metrics clock begins in its constructor. Slow software
renderer startup can consume its sampling window before useful painting. Both active
comparison binaries therefore use the same optional timing-only recorder gate, after
visible paint and three seconds of warmup. The collector waits for the recorder's file
acknowledgment before interpreting its five-second interval. Input remains active through
warmup and collection. Crashes or missing callbacks during timed collection are failures;
performance values are never a reason to reject a trial. The patch is not shipped in the PR.

<details>
<summary>Diagnostic recorder patch applied to both frozen revisions (tabs shown as spaces)</summary>

```diff
--- a/apps/desktop/examples/fixtures/native_metrics.rs
+++ b/apps/desktop/examples/fixtures/native_metrics.rs
@@ -1,15 +1,18 @@
-// Optional, bounded instrumentation for the offline native preview, never the application.
+// Identical task-local timing instrumentation for both frozen application revisions.
 struct NativeMetrics {
    output: PathBuf,
-   started: Instant,
+   started: Option<Instant>,
+   gate: Option<PathBuf>,
    samples: Vec<(f64, u64, usize)>,
    written: bool,
 }
 impl NativeMetrics {
    fn new(output: &str) -> Self {
+       let gate = std::env::var_os("SEREIN_PREVIEW_METRICS_GATE").map(PathBuf::from);
        Self {
            output: output.into(),
-           started: Instant::now(),
+           started: gate.is_none().then(Instant::now),
+           gate,
            samples: Vec::new(),
            written: false,
        }
@@ -18,18 +21,25 @@
        if self.written {
            return;
        }
-       let elapsed = self.started.elapsed().as_secs_f64();
-       if (3.0..8.0).contains(&elapsed) && self.samples.len() < 8192 {
-           self.samples.push((
-               elapsed,
-               duration.as_nanos().min(u128::from(u64::MAX)) as u64,
-               wheel_events,
-           ));
+       if self.started.is_none() {
+           if !self.gate.as_ref().is_some_and(|path| path.is_file()) {
+               ctx.request_repaint_after(Duration::from_millis(50));
+               return;
+           }
+           self.started = Some(Instant::now());
+           std::fs::write(self.output.with_extension("ready"), b"ready")
+               .expect("write offline preview readiness");
        }
-       if elapsed >= 8.0 {
+       let elapsed = self.started.unwrap().elapsed().as_secs_f64();
+       let warmup = if self.gate.is_some() { 0.0 } else { 3.0 };
+       if (warmup..warmup + 5.0).contains(&elapsed) && self.samples.len() < 8192 {
+           self.samples.push((elapsed, duration.as_nanos().min(u128::from(u64::MAX)) as u64, wheel_events));
+       }
+       if elapsed >= warmup + 5.0 {
            let report = serde_json::json!({
                "method":"MessagingUi::show callback only; not presented-frame or input latency",
-               "warmup_seconds":3, "sample_seconds":5,
+               "warmup_seconds":warmup, "sample_seconds":5,
+               "external_gate":self.gate.is_some(),
                "columns":["elapsed_seconds","callback_nanoseconds","wheel_event_count"],
                "samples":self.samples
            });
@@ -37,12 +47,7 @@
                .expect("write offline preview measurements");
            self.written = true;
        } else {
-           // Opt-in active sampling only; process-idle measurements omit --metrics.
-           let delay = if elapsed < 3.0 {
-               3.0 - elapsed
-           } else {
-               0.1_f64.min(8.0 - elapsed)
-           };
+           let delay = if elapsed < warmup { warmup - elapsed } else { 0.1_f64.min(warmup + 5.0 - elapsed) };
            ctx.request_repaint_after(Duration::from_secs_f64(delay));
        }
    }
```

</details>
