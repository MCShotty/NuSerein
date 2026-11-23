# Native DAVE provider compatibility

Based on crates.io `openmls_basic_credential 0.5.0`, OpenMLS revision `6b85f0edc560b4fe0f5b9266092947a774614f3f`, archive SHA-256 `983e8be1457dd6f316f409292cec334af3b57b49a19deadc925c83c3c35e15b6`. `Cargo.toml.orig` and the MIT license record upstream provenance.

Keep the OpenMLS 0.8 trait and wire types, but use the current RustCrypto P-256 public API (same raw private scalar, uncompressed SEC1 public key and DER signature formats). The RustCrypto provider also uses the updated `hpke-rs` stack; it does not implement its own HPKE or curve operations. No DAVE ciphersuite or application protocol logic changes.

Call each scheme's own `Signer` trait explicitly: current P-256 uses `signature` 3, while Ed25519 still uses `signature` 2. Neither path depends on development features to unify the traits.
