# Display timezone dependency provenance

Serein uses the MIT option for Jiff, jiff-core and jiff-tzdb (Unlicense OR MIT),
and iana-time-zone and its Haiku adapter (MIT OR Apache-2.0). The license texts
listed below were verified byte-for-byte against the current releases; filenames retain their original versions.
`Cargo.lock` retains each source identity and registry archive checksum.

Jiff is built without default features, enabling `std` and `tzdb-bundle-always`.
Named-zone conversion and search use its explicit bundled database. The pinned
jiff-tzdb 1.0.0 contains IANA release **2026f**, as recorded in its `tzname.rs`.
Its upstream `COPYING` statement is also retained unmodified as
`jiff-tzdb-0.1.9-COPYING`. See <https://www.iana.org/time-zones> and the pinned
registry source at <https://crates.io/crates/jiff-tzdb/1.0.0> for data provenance.

iana-time-zone only discovers the OS timezone identifier in a blocking worker;
Jiff performs the conversion. OS timezone settings are not changed. The Haiku
adapter is a target-specific locked dependency, not part of Windows/macOS/Linux
application binaries. Optional Jiff macro/defmt features are not enabled.

| Registry crate | Version | Archive SHA-256 | MIT text | Text SHA-256 |
| --- | --- | --- | --- | --- |
| jiff | 1.0.1 | `ed5d126d72eb017f10b302e73bdeca446bea405ee449af4d9e7866d6b082d83e` | `jiff-0.2.38-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| jiff-core | 1.0.1 | `b6b46a105a9874634dd96231021526bf24ca04955a5998c59a9c7666e634b7b8` | `jiff-core-0.1.1-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| jiff-tzdb | 1.0.0 | `12e71a1b68a875a44a33bb7a1dd84392cb77145a3367a552a3cfe6a4f5cf3afc` | `jiff-tzdb-0.1.9-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| iana-time-zone | 0.1.65 | `e31bc9ad994ba00e440a8aa5c9ef0ec67d5cb5e5cb0cc7f8b744a35b389cc470` | `iana-time-zone-0.1.65-LICENSE-MIT` | `da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9` |
| iana-time-zone-haiku | 0.1.2 | `f31827a206f56af32e590ba56d5d2d085f558508192593743f16b2306495269f` | `iana-time-zone-haiku-0.1.2-LICENSE-MIT` | `da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9` |

Existing packaging copies `assets/licenses/files` unchanged into
`licenses/files`; no additional runtime libraries or source bundle are required
by these MIT license options. Each component retains its upstream terms.
