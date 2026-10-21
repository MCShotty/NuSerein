# Display timezone dependency provenance

Serein uses the MIT option for Jiff, jiff-core and jiff-tzdb (Unlicense OR MIT),
and iana-time-zone and its Haiku adapter (MIT OR Apache-2.0). The license texts
listed below are copied unmodified from the exact crates.io registry releases.
`Cargo.lock` retains each source identity and registry archive checksum.

Jiff is built without default features, enabling `std` and `tzdb-bundle-always`.
Named-zone conversion and search use its explicit bundled database. The pinned
jiff-tzdb 0.1.9 contains IANA release **2026e**, as recorded in its `tzname.rs`.
Its upstream `COPYING` statement is also retained unmodified as
`jiff-tzdb-0.1.9-COPYING`. See <https://www.iana.org/time-zones> and the pinned
registry source at <https://crates.io/crates/jiff-tzdb/0.1.9> for data provenance.

iana-time-zone only discovers the OS timezone identifier in a blocking worker;
Jiff performs the conversion. OS timezone settings are not changed. The Haiku
adapter is a target-specific locked dependency, not part of Windows/macOS/Linux
application binaries. Optional Jiff macro/defmt features are not enabled.

| Registry crate | Version | Archive SHA-256 | MIT text | Text SHA-256 |
| --- | --- | --- | --- | --- |
| jiff | 0.2.38 | `b2b005715dcbeb0089a3c0dab99f2ff1cc3b2525323552703d648585d342a383` | `jiff-0.2.38-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| jiff-core | 0.1.1 | `5e52fe76043ccecc9005d2305ebaadf7d7fc0cc89ca6baa10a94d6bc68c7128c` | `jiff-core-0.1.1-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| jiff-tzdb | 0.1.9 | `fa8377070c6bae868759445e5a77f66d84f0b72f3a054bfb00e6d038b8282da7` | `jiff-tzdb-0.1.9-LICENSE-MIT` | `0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| iana-time-zone | 0.1.65 | `e31bc9ad994ba00e440a8aa5c9ef0ec67d5cb5e5cb0cc7f8b744a35b389cc470` | `iana-time-zone-0.1.65-LICENSE-MIT` | `da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9` |
| iana-time-zone-haiku | 0.1.2 | `f31827a206f56af32e590ba56d5d2d085f558508192593743f16b2306495269f` | `iana-time-zone-haiku-0.1.2-LICENSE-MIT` | `da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9` |

Existing packaging copies `assets/licenses/files` unchanged into
`licenses/files`; no additional runtime libraries or source bundle are required
by these MIT license options. Each component retains its upstream terms.
