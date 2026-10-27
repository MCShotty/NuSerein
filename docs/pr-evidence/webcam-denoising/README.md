# Webcam denoising evidence

The synthetic Voice & Video view was captured at 1120 × 850, scale 1, dark theme,
with the same scroll input and software rendering. The baseline uses the `c991521b`
profile-preview release binary, whose tree matches fork main `d1fb20e3`. The after
view uses the `4204c3cb` profile-preview debug build. These are UI fixtures, not live
camera captures, and their different build profiles are not a performance comparison.

An isolated CPU filter probe used the pinned private FFmpeg build and the C bridge
compiled with `-O2` on Linux x86-64, Intel Xeon Platinum 8573C, with five available
CPUs. For each resolution it processed a static synthetic I420 frame 60 times per
sample, after one warmup, for five measured samples. Median wall time was 0.728 ms
per input at 640 × 480 and 1.764 ms at 1280 × 720. This measures only the filter
and its copies; it is not end-to-end camera latency, an FPS guarantee, a comparison
against a disabled pipeline, or evidence for other hardware or moving scenes.

Real webcam quality, synchronization, CPU/GPU use and package-size changes have not
been measured. BSVD and automatic detection of existing camera processing are not
implemented. See the pull request for current CI and installer status.

CI replay compared fork main `d1fb20e3`, implementation `4204c3cb`, and upstream
nightly `38e46b8f` (`v1.0.0-nightly.20261026.65`) in release mode with one warmup
and five samples per revision on each hosted runner. Times below are median
milliseconds for the synthetic reducer; percentages compare the implementation
with fork main. These do not measure UI, camera processing, process memory or live
behavior.

| Runner | Nightly | Fork main | Implementation | Fork delta |
| --- | ---: | ---: | ---: | ---: |
| Windows x64 | 77.170 | 83.074 | 82.568 | −0.6% |
| macOS ARM64 | 69.977 | 71.545 | 81.698 | +14.2% |
| Linux x64 | 70.832 | 75.867 | 75.669 | −0.3% |

The macOS samples varied substantially and affinity was unavailable. The result is
an unresolved timing concern, not proof of its cause or of equal performance.
The Windows and Linux samples used CPU affinity. Source measurements are in the
[CI run](https://github.com/MCShotty/Serein/actions/runs/40492101392).
