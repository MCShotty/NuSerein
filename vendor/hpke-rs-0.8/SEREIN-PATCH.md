# Native HPKE 0.8 package

Registry hpke-rs 0.8.1, archive SHA-256 `cb3d62c3ce304a6ac89d584cdc8c619416bced26230a294df2ae7de1dc163a48`, upstream commit `24330863b3d1d4179e4bcea8cef6bf6fa313057e` in celabshq/libcrux. For the native OpenMLS provider only: retains the previous standard SHAKE256 substitution, removes the unneeded libcrux backend from the graph, and keeps the upstream 0.8 RustCrypto backend. No key formats or HPKE ciphersuites are changed. Corresponding MPL-2.0 source must ship with binaries.
