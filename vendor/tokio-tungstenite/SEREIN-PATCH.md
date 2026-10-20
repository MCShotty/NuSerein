# tokio-tungstenite 0.30.0: patched certificate roots

Source: the published crates.io 0.30.0 archive, upstream revision `4994a078037a18a9347b8cf49291a5031d63d9e0`.
Published archive SHA-256: `17a073bfed563fa236697a068031408a93cd9522e08abf9933ead3e73411bd71`.
License: MIT; the original LICENSE and upstream source/examples are retained.
The workspace Cargo.lock remains authoritative; the upstream package lockfile is omitted.

Only the optional webpki-roots dependency is changed, from 0.26 to >=1.0.10.
The released 0.30.0 still selects the retired 0.26 compatibility package,
which is flagged by RUSTSEC-2026-0381. The TLS code uses the same
TLS_SERVER_ROOTS API and the same Rustls verifier. The workspace also locks
rustls-webpki >=0.103.16 and webpki-root-certs >=1.0.10 to address
RUSTSEC-2026-0382 and RUSTSEC-2026-0383.

No certificate checks, audit findings, TLS features or platform trust policies
are disabled. Remove this patch when an upstream release supports patched roots.

Validation: cargo audit --deny warnings, the workspace check and synthetic
WebSocket/voice transport regressions. Live Discord connections remain owner-controlled.
