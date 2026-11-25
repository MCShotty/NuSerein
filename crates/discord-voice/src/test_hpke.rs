//! RFC 9180 Appendix A.3.1, through the actual OpenMLS provider selected for DAVE.
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{OpenMlsProvider, crypto::OpenMlsCrypto, types::*};

fn bytes(s: &str) -> Vec<u8> {
	let (pairs, remainder) = s.as_bytes().as_chunks::<2>();
	assert!(
		remainder.is_empty(),
		"RFC vector must contain complete hex bytes"
	);
	pairs
		.iter()
		.map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
		.collect()
}

#[test]
fn p256_hpke_opens_rfc9180_base_vector() {
	let provider = OpenMlsRustCrypto::default();
	let suite = || {
		HpkeConfig(
			HpkeKemType::DhKemP256,
			HpkeKdfType::HkdfSha256,
			HpkeAeadType::AesGcm128,
		)
	};
	let key = bytes("f3ce7fdae57e1a310d87f1ebbde6f328be0a99cdbcadf4d6589cf29de4b8ffd2");
	let mut packet = HpkeCiphertext {
		kem_output: bytes(concat!(
			"04a92719c6195d5085104f469a8b9814d5838ff72b60501e2c4466e5e67b325",
			"ac98536d7b61a1af4b78e5b7f951c0900be863c403ce65c9bfcb9382657222d18c4",
		))
		.into(),
		ciphertext: bytes(concat!(
			"5ad590bb8baa577f8619db35a36311226a896e7342a6d836d8b7bcd2f20b6c7f",
			"9076ac232e3ab2523f39513434",
		))
		.into(),
	};
	let info = b"Ode on a Grecian Urn";
	let aad = b"Count-0";
	assert_eq!(
		provider
			.crypto()
			.hpke_open(suite(), &packet, &key, info, aad)
			.unwrap(),
		b"Beauty is truth, truth beauty"
	);
	packet.ciphertext = vec![0; packet.ciphertext.as_slice().len()].into();
	assert!(
		provider
			.crypto()
			.hpke_open(suite(), &packet, &key, info, aad)
			.is_err()
	);
}
