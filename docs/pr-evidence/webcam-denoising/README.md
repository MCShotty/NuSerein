# Webcam denoising evidence

The current synthetic Voice & Video pair shows the removal of Auto and the manual
**Off / ATADenoise** choices. Both captures use debug profile-preview builds at
1120 × 850, scale 1, dark theme, the same scroll input and software rendering.
The before binary is from `4204c3cb`, whose application source matches `5220039d`;
the after binary is the manual-selection update at `f47ac7dc`. These are UI fixtures,
not live camera captures or release performance evidence.
The updated controls were also inspected in a 720 × 900 light-theme fixture;
the two choices and their descriptions remain readable at that width.

The [original feature screenshots](https://github.com/MCShotty/Serein/tree/5220039d9035e6965356abbb74675c3b2abef8cb/docs/pr-evidence/webcam-denoising)
remain pinned to their original revision: before used the `c991521b` release fixture
(the same tree as fork main `d1fb20e3`), and after used the `4204c3cb` debug fixture.

An isolated CPU filter probe used the pinned private FFmpeg build and the C bridge
compiled with `-O2` on Linux x86-64, Intel Xeon Platinum 8573C, with five available
CPUs. For each resolution it processed a static synthetic I420 frame 60 times per
sample, after one warmup, for five measured samples. Median wall time was 0.728 ms
per input at 640 × 480 and 1.764 ms at 1280 × 720. This measures only the filter
and its copies; it is not end-to-end camera latency, an FPS guarantee, a comparison
against a disabled pipeline, or evidence for other hardware or moving scenes.

Real webcam quality, synchronization, CPU/GPU use and package-size changes have not
been measured. The current scope is manual FFmpeg filtering only, with Off as the
default; BSVD and hardware-denoising detection are deferred. See the pull request
for current CI and installer status.

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

The manual-selection update `f47ac7dc` was also compared with fork main `d1fb20e3`
and nightly `2d0165bf` (`v1.0.0-nightly.20261027.67`) in
[CI](https://github.com/MCShotty/Serein/actions/runs/40535040574). Each revision used
Rust 1.98.1, optimization level 3, thin LTO, one codegen unit, stripped symbols,
line-table debug information and panic abort, with one warmup and five alternating
measured samples per workload. Hosted environments were Windows Server 2025 x64
(four logical CPUs), Linux 6.17 x64/glibc 2.39 (four), and macOS 26.6.2 ARM64 (three).

| Runner / workload | Nightly (ms) | Fork main (ms) | `f47ac7dc` (ms) | Fork delta |
| --- | ---: | ---: | ---: | ---: |
| Windows x64 / reducer | 108.027 | 111.535 | 111.124 | −0.4% |
| Windows x64 / wide channels | 228.962 | 11.665 | 11.810 | +1.2% |
| Linux x64 / reducer | 79.392 | 87.338 | 86.751 | −0.7% |
| Linux x64 / wide channels | 194.747 | 9.369 | 9.293 | −0.8% |
| macOS ARM64 / reducer | 92.289 | 93.041 | 89.256 | −4.1% |
| macOS ARM64 / wide channels | 145.782 | 6.827 | 7.575 | +11.0% |

Windows and Linux used single-CPU affinity; macOS affinity was unavailable. The
Linux fork/main and updated reducer executables were byte-identical, so their small
timing differences do not establish an optimization. The earlier macOS reducer
slowdown was not reproduced by this sample; its cause remains unisolated. The current
macOS wide-channel result is another unresolved timing concern: updated samples
ranged from 6.519–11.260 ms, compared with 6.652–7.600 ms for fork main. These core
workloads do not run camera filtering, a UI, helper processes or native capture.
