# Windows capture-overhead comparison

Baseline: installer revision `30d3437ffe6a3b5d06ecd2ae67c7d2f1c76cd28b`.
The reported case is a 1440p60 SDR share with FFmpeg selected and game FPS falling from
approximately 250 to 115. That observation is owner supplied; this document does not
claim a measured cause or an improvement on physical gaming hardware.

Keep codec, bitrate, vendor preset, lookahead, pre-analysis, source, game scene, display
refresh rate, driver, viewport and HDR state fixed. Record **actual output dimensions**:
1440p preserves the source aspect ratio, so different monitors can encode different widths.

## Isolate the stages

Use the separate `windows-capture-diagnostics-X64`/`ARM64` CI artifact. Extract the whole
archive; its adjacent private FFmpeg libraries, licenses and corresponding source belong
with `windows_capture.exe`. It is an optional diagnostic tool and is not run by the app.
For a source build, use pinned Rust and the normal FFmpeg development environment:

```powershell
cargo build --locked --release -p discord-voice --example windows_capture
.\target\release\examples\windows_capture.exe --help
```

Run `--list` to enumerate sources without capturing. Source handles are transient; use
an exact returned handle immediately. Only start a stage for a source you own and elect
to capture. No probe contacts Discord, captures audio, writes pictures, or sends media.

```powershell
.\windows_capture.exe --list
.\windows_capture.exe --owner-controlled --source display:0xHANDLE --stage capture-only --resolution 1440 --fps 60 --warmup 10 --seconds 30 --revision COMMIT
.\windows_capture.exe --owner-controlled --source display:0xHANDLE --stage readback --resolution 1440 --fps 60 --warmup 10 --seconds 30 --revision COMMIT
.\windows_capture.exe --owner-controlled --source display:0xHANDLE --stage encode --resolution 1440 --fps 60 --codec h264 --backend ffmpeg --adapter VENDOR:DEVICE:luid:WINDOWS_LUID --warmup 10 --seconds 30 --revision COMMIT
```

Replace placeholders; keep the actual selected codec instead of blindly copying `h264`.
The encode stage requires the exact renderer adapter helper key from Serein diagnostics,
including its Windows LUID. It cannot infer the intended adapter from its vendor alone.
`capture-only` admits and releases native frames without readback or encoding. `readback`
uses the production native processing/readback and conversion into a reusable local sink.
`encode` uses the production capture/encoder worker and discards output locally; **it does
not measure Discord transport, receiver delivery or viewer quality**. Preview is disabled.
Use the normal app with the same private session for the full-sharing stage.

Output contains bounded JSON start/complete records. Warmup totals are subtracted; atomic
updates crossing the boundary can contribute one in-progress operation. Timing maxima are
explicitly labeled as including warmup. Missing stage counters are `null`, not measured zero.
Readback timings are CPU wait/copy wall time, not GPU timestamp-query durations. Capture-only
on a static desktop can have no new frames during the sample; it must still observe an
initial frame. Closing the selected source or an early worker exit invalidates the sample.

The original installer does not contain this new harness. Measure its **full sharing**
using the external sampler below. Stage isolation is evidence from the current source and
must not be presented as a capture-only measurement of the unmodified original installer.

## Measure the game's impact

Use a preinstalled **PresentMon 2.x console** executable by exact path. The sampler observes only
explicit game/capture PIDs and never starts, stops or changes either application. It starts
and retires its own PresentMon helper. Check PresentMon's requirements for the selected game
and Windows permissions; an unavailable/failed capture invalidates the run.

```powershell
powershell -NoProfile -File .\stream-capture-sample.ps1 -OwnerControlled -PresentMon C:\Tools\PresentMon.exe -GameProcessId 1234 -CaptureProcessId 5678 -Scenario full-sharing -Revision COMMIT -Label 'Same scene; actual dimensions; codec; bitrate; preset; HDR off' > sample.json
```

For `no-share`, stop sharing yourself and omit `-CaptureProcessId`. For `capture-only`,
`readback` or `encode`, use the matching probe's PID and keep that explicitly started probe
running longer than the sampler's warmup/sample window (up to `--seconds 600`). For the full
sharing comparison, use the app's exact PID. Starting, ending or resizing a share during a
sample invalidates it. The two tools' timing windows are independent; retain their durations
and do not equate their counter windows.

The script rejects an early process exit, missing frame data and multiple swap chains.
If the game has several swap chains, repeat with `-SwapChain` selecting the intended chain.
The revision argument is a caller-supplied label; verify the captured executable SHA-256
against its CI `build.json` or the baseline package. It reports game present FPS, 1% lows,
frame-interval p95/p99, observed-process CPU and sampled
peak/settled memory. GPU engine contexts remain separate; their Windows counters are sampled
at 1 Hz and are not physical GPU timestamps. Failed/unavailable GPU counters are reported
explicitly. The observer adds work: use identical tooling for every run; a second comparison
with `-SkipGpuCounters` can check measurement interference.

Run one warmup, then five alternating baseline/current repetitions for each comparable
scenario. Report medians and frame-time tails, plus the no-sharing penalty. Inspect the
receiver's actual frame cadence and visual quality separately. The acceptance target is no
more than 10% average game-FPS loss while retaining requested cadence and visual quality;
synthetic tests and native CI alone do not establish that target.

Keep raw logs and any private source/window metadata local. Commit only a reviewed aggregate
comparison with revisions, hardware, settings, method and remaining limitations. No physical
Windows gaming result has been collected in this cloud environment.

The [local performance record](../../performance.md#windows-capture-overhead--october-23-2026)
contains the matched SDR conversion method and results, baseline package
provenance and verification limits. This component result is not a substitute
for the owner-controlled gaming comparison above. Current installer provenance,
package deltas and native CI status are recorded in
[PR #22](https://github.com/MCShotty/Serein/pull/22).
