# LavaSRv2 microphone enhancement

LavaSR and its model weights are Apache-2.0, from
[ysharma3501/LavaSR](https://github.com/ysharma3501/LavaSR/tree/33ac040892519c1bb4aed7eb32e79af51cc29e2a)
and the `YatharthS/LavaSR` model repository revision
`b98dc8be472da45ab7b6346ad7997e1dfeb5911d`. The selected model is **enhancer_v2**,
with the ULUNAS denoiser enabled. `LavaSR-LICENSE` preserves its license.

The Apache-2.0 embedded-weight ONNX exports are from
[LavaSR-ONNX-Embedded](https://github.com/idocinthebox/LavaSR-ONNX-Embedded/tree/b591321da28c070cdfb8af961df4cb5be662490d),
release `v1.0.0` (the export release number, not the model generation).
`LavaSR-ONNX-LICENSE` preserves the exporter's license. Native Rust processing
implements periodic-Hann STFT/ISTFT, Slaney mel features, sinc resampling and
low-frequency merging using the published v2 configuration. Streaming windows,
overlap, cancellation and the helper protocol are Serein additions; no quality
equivalence to non-streaming inference is claimed.

The CPU ONNX Runtime **1.23.2** is MIT-licensed. Each package includes its
`ONNXRuntime-LICENSE` and `ONNXRuntime-ThirdPartyNotices.txt` from the verified
official archive. Windows also includes the MSVC app-local runtime and its
redistribution license. No Python runtime, CUDA libraries or remote audio
service ships. Source and all artifact URLs, sizes and SHA-256 pins are in
`packaging/lavasr/bundle.py`; the package's `voice-enhancement/build.json`
records the staged model and runtime provenance.

Windows app-local CRT files come from the installed MSVC toolchain's
`Redist/MSVC/*/{x64,arm64}/Microsoft.VC*.CRT` redistributable list, under that
toolchain's Visual Studio redistribution terms. `MSVC-LICENSE.txt` is the
runtime's end-user terms, transcribed by joining the paragraphs of Microsoft's
[original DOCX](https://visualstudio.microsoft.com/wp-content/uploads/2021/09/Visual-C-Runtime-2015-2022-License-1.docx)
(SHA-256 `f1e3d56ceb2ad68aae0711b910375009e651ac5530fa0760f0dea6e81e54fae1`;
paragraph-ending whitespace is normalized),
linked from its official Visual C++ 2015–2022 Runtime license page. These
end-user terms are distinct from the build toolchain's redistribution grant.

Model verification happens before inference; inference runs only in a bounded,
disposable local microphone helper. Installation supplies the models. Selecting
this option never downloads models or sends microphone audio to a model server.

## Rust support notices

`RUST-NOTICES.txt` concatenates each chosen MIT/ISC grant from the locked
registry archive, preserving license wording and normalizing the final blank line. Archives were matched to Cargo.lock before extracting these texts.
Apache-2.0 remains an alternative where declared by the crate.

| Crate | Registry archive SHA-256 | License file | Text SHA-256 |
| --- | --- | --- | --- |
| `ort 2.0.0-rc.14` | `08ca9287c05aace7e95250104a00d54b7da0fc9ac5f25efa424b80827d2e25eb` | `LICENSE-MIT` | `6bba08de46289c40986a2e6b310e2da61fcea33b3c112c8320f6093f8f9cb71b` |
| `ort-sys 2.0.0-rc.14` | `4af4763e553f916650b0bfe4b8d217d61c28607f6117e48b8ab5584d1138f22f` | `LICENSE-MIT` | `6bba08de46289c40986a2e6b310e2da61fcea33b3c112c8320f6093f8f9cb71b` |
| `libloading 0.9.0` | `754ca22de805bb5744484a5b151a9e1a8e837d5dc232c2d7d8c2e3492edc8b60` | `LICENSE` | `b29f8b01452350c20dd1af16ef83b598fea3053578ccc1c7a0ef40e57be2620f` |
| `ndarray 0.17.2` | `520080814a7a6b4a6e9070823bb24b4531daac8c4627e08ba5de8c5ef2f2752d` | `LICENSE-MIT` | `1fd6747d2c8e80f9fa766f57c5888864774621deb85cc2838ccaed727db32d45` |
| `matrixmultiply 0.3.11` | `3f607c237553f086e7043417a51df26b2eb899d3caff94e6a67592ff992fedc7` | `LICENSE-MIT` | `792d075c7bad6dac258a44e799eb64cbf465e24d9932d27669be08c5ec957e27` |
| `rawpointer 0.2.1` | `60a357793950651c4ed0f3f52338f53b2f809f32d83a07f72909fa13e4c6c1e3` | `LICENSE-MIT` | `7576269ea71f767b99297934c0b2367532690f8c4badc695edf8e04ab6a1e545` |

LavaSR credits Vocos and UL-UNAS as its architecture/denoiser sources. Their
MIT notices are additionally retained as `Vocos-LICENSE` and `UL-UNAS-LICENSE`:

- Xiaobin-Rong/ul-unas: [`00f7c700da43d38347f30a6ccebd86fcbc798e07`](https://raw.githubusercontent.com/Xiaobin-Rong/ul-unas/00f7c700da43d38347f30a6ccebd86fcbc798e07/LICENSE); MIT; text SHA-256 `94daad6ad643a2f79d4a111015c3415fe20c6f630b908e884edf466923724184`.
- gemelo-ai/vocos: [`eb39abfc42c1dee4854d9b10d44dd7d4fd3b0e53`](https://raw.githubusercontent.com/gemelo-ai/vocos/eb39abfc42c1dee4854d9b10d44dd7d4fd3b0e53/LICENSE); MIT; text SHA-256 `8f36ba0c214d4f487ffd7ee0b8528da9b67b459bef2d115de9cd001316589f46`.

## Eigen corresponding source

The CPU runtime's third-party notices identify Eigen (MPL-2.0). Packages ship
the unmodified Eigen source archive pinned in ONNX Runtime v1.23.2's
`cmake/deps.txt`, commit `1d8b82b0740839c0de7f1242a3585e3390ff5f33`.
Its SHA-1 matches the upstream build pin `05b19b49e6fbb91246be711d801160528c135e34`;
Serein additionally verifies SHA-256
`6a60d76351f97132669daeeb721d6bf14b008101883ad2d687a3201c5c461eb0`.
The release's `cmake/external/eigen.cmake` consumes this archive without patches.
Its complete license texts remain in the source archive; the MPL text is also
retained in `ONNXRuntime-ThirdPartyNotices.txt`. The artifact's `build.json`
includes the source URL and checksum. This source is distributed alongside
all supported runtime builds, including Flatpak and Nix.
