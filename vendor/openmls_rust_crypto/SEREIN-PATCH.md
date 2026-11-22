# Native DAVE provider compatibility

Based on crates.io `openmls_rust_crypto 0.5.1`, OpenMLS revision `47dbedecad0c1fd8eb5368d582250ebfcc1e1ce6`, archive SHA-256 `fafcc8a3552b10fbb3ab757cccaf1a34081e826ca819f49aa7e6645b1d95c00f`. `Cargo.toml.orig` and the MIT license record upstream provenance.

Keep the OpenMLS 0.8 trait and wire types, but use the current RustCrypto P-256 public API (same raw private scalar, uncompressed SEC1 public key and DER signature formats). The RustCrypto provider also uses the updated `hpke-rs` stack; it does not implement its own HPKE or curve operations. No DAVE ciphersuite or application protocol logic changes.
