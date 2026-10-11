// Several tests are adapted from pdfcraft (MIT OR Apache-2.0), crates/sign/tests/{crypto,store_keys}.rs @ b8ddf71;
// the .p12 / .pem files in testdata/ are its OpenSSL-made test fixtures (password "test", they protect nothing).
//
// Safety rule for these tests: only self-signed test identities made here, in a temp dir. Nothing reads, lists or
// writes the user's certificate stores or keys.

use super::der::{self, Time, Tlv};
use super::keys::{DigestAlg, PssParams, Scheme, StoreKey};
use super::{Certificate, DigitalId, Name, PrivateKey, PublicKey, SignError, Trust, cms, pkcs12, trust, x509};

const RSA_AES: &[u8] = include_bytes!("testdata/rsa-aes.p12");
const RSA_LEGACY: &[u8] = include_bytes!("testdata/rsa-legacy.p12");
const EC_P256: &[u8] = include_bytes!("testdata/ec-p256.p12");
const EC_P384: &[u8] = include_bytes!("testdata/ec-p384.p12");
const EC_LEGACY: &[u8] = include_bytes!("testdata/ec-legacy.p12");
const CHAIN: &[u8] = include_bytes!("testdata/chain.p12");
const RSA_CRT_PEM: &[u8] = include_bytes!("testdata/rsa.crt.pem");

/// A per-test directory under the system temp dir, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(test: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("inkwell-sign-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_id(name: &str) -> DigitalId {
    DigitalId::create_self_signed(name, "test@example.com", "Inkwell Tests", 2).unwrap()
}

// ── digital IDs: create, PKCS #12, CMS ─────────────────────────────────────────────────────────

#[test]
fn self_signed_id_survives_a_pkcs12_file_and_signs_a_digest() {
    let dir = TempDir::new("roundtrip");
    let id = new_id("Grace Hopper");
    assert!(id.cert.is_self_signed());
    assert_eq!(id.cert.subject.common_name(), Some("Grace Hopper"));
    assert_eq!(id.cert.subject.email(), Some("test@example.com"));
    assert_eq!(id.key.public_key().describe(), "RSA 2048-bit");
    assert_eq!(id.cert.key_usage.map(|u| u & 0b11), Some(0b11), "digitalSignature + nonRepudiation");
    assert_eq!(id.cert.signing_problem(), None);

    let file = dir.path("id.p12");
    std::fs::write(&file, pkcs12::write(&id, "s3cret").unwrap()).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let back = pkcs12::open(&bytes, "s3cret").unwrap();
    assert_eq!(back.cert, id.cert);
    assert_eq!(back.friendly_name.as_deref(), Some("Grace Hopper"));
    assert!(matches!(pkcs12::open(&bytes, "wrong"), Err(SignError::WrongPassword)));
    assert!(matches!(pkcs12::open(&bytes, ""), Err(SignError::WrongPassword)));

    // Two byte ranges hashed without joining them, as a PDF's /ByteRange is.
    let digest = DigestAlg::Sha256.digest(&[b"first range", b"second range"]);
    assert_eq!(digest, DigestAlg::Sha256.digest(&[b"first rangesecond range"]));
    let cms_der = cms::sign_detached(&back.key, &back.cert, &back.chain, DigestAlg::Sha256, &digest).unwrap();
    // A PDF /Contents placeholder is zero padded.
    let mut padded = cms_der.clone();
    padded.extend([0u8; 64]);
    let sd = cms::SignedData::parse(&padded).unwrap();
    let signer = sd.signer_certificate().expect("the signer's certificate is embedded");
    assert_eq!(signer, &id.cert);
    assert!(sd.verify_signature(signer, &digest).unwrap());

    // PAdES B-B attributes: content-type, message-digest, signing-certificate-v2, and no signing-time.
    assert_eq!(sd.signer.content_type.as_deref(), Some(cms::DATA));
    assert_eq!(sd.signer.message_digest.as_deref(), Some(&digest[..]));
    assert!(sd.signer.signing_certificate);
    assert_eq!(sd.signer.signing_time, None);
    assert!(sd.content.is_none(), "detached");

    // Another content digest, or a flipped signature bit, is not the signature.
    // The message digest is inside the signed attributes: changing the signed bytes breaks the signature.
    let mut bad_attrs = sd.clone();
    if let Some(a) = bad_attrs.signer.signed_attrs.as_mut() {
        let at = a.windows(digest.len()).position(|w| w == digest.as_slice()).unwrap();
        a[at] ^= 1;
    }
    assert!(!bad_attrs.verify_signature(signer, &digest).unwrap());
    let mut bad_sig = sd.clone();
    bad_sig.signer.signature[5] ^= 1;
    assert!(!bad_sig.verify_signature(signer, &digest).unwrap());
    // And a certificate that did not sign it does not verify it.
    let stranger = new_id("Someone Else");
    assert!(!sd.verify_signature(&stranger.cert, &digest).unwrap());
}

#[test]
fn signatures_verify_for_every_key_type_and_digest() {
    for (file, bytes) in [("rsa-aes", RSA_AES), ("rsa-legacy", RSA_LEGACY), ("ec-p256", EC_P256), ("ec-p384", EC_P384), ("ec-legacy", EC_LEGACY), ("chain", CHAIN)] {
        let id = pkcs12::open(bytes, "test").unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(&id.cert.public_key, id.key.public_key(), "{file}");
        assert!(matches!(pkcs12::open(bytes, "nope"), Err(SignError::WrongPassword)), "{file}");
        for alg in [id.key.preferred_digest(), DigestAlg::Sha512] {
            let digest = alg.digest(&[b"the document bytes"]);
            let sig = cms::sign_detached(&id.key, &id.cert, &id.chain, alg, &digest).unwrap();
            let sd = cms::SignedData::parse(&sig).unwrap();
            let signer = sd.signer_certificate().unwrap();
            assert_eq!(signer, &id.cert, "{file}");
            assert!(sd.verify_signature(signer, &digest).unwrap(), "{file} {}", alg.name());
            assert_eq!(sd.certificates.len(), 1 + id.chain.len(), "{file}");
        }
    }
}

#[test]
fn p256_keys_generate_sign_and_verify() {
    let key = PrivateKey::generate_p256().unwrap();
    let name = Name::build("Ada Lovelace", "", "Analytical", "ada@example.com", "gb");
    let now = Time { year: 2026, month: 10, day: 2, hour: 12, minute: 0, second: 0 };
    let cert = Certificate::self_signed(&name, &key, now, 5, &[0x42, 0x01]).unwrap();
    assert!(cert.is_self_signed());
    assert_eq!(cert.subject.display(), "C=GB, O=Analytical, CN=Ada Lovelace, E=ada@example.com");
    assert_eq!(cert.not_after.year, 2031);
    assert_eq!(cert.serial_hex(), "4201");
    assert!(matches!(cert.public_key, PublicKey::P256(_)));
    let sig = key.sign(DigestAlg::Sha256, b"message").unwrap();
    let digest = DigestAlg::Sha256.digest(&[b"message"]);
    assert!(key.public_key().verify(Scheme::Ecdsa, DigestAlg::Sha256, &digest, &sig).unwrap());
    assert!(!key.public_key().verify(Scheme::Ecdsa, DigestAlg::Sha256, &DigestAlg::Sha256.digest(&[b"other"]), &sig).unwrap());
    // Signatures are for SHA-2 only.
    assert!(matches!(key.sign(DigestAlg::Sha1, b"x"), Err(SignError::Unsupported(_))));

    let id = DigitalId { key, cert, chain: Vec::new(), friendly_name: None };
    let back = pkcs12::open(&pkcs12::write(&id, "pw").unwrap(), "pw").unwrap();
    assert_eq!(back.cert, id.cert);
    assert_eq!(back.friendly_name, None);
}

#[test]
fn chain_fixture_has_its_issuer() {
    let id = pkcs12::open(CHAIN, "test").unwrap();
    assert_eq!(id.cert.subject.email(), Some("ada@example.com"));
    assert_eq!(id.chain.len(), 1);
    assert!(id.chain[0].is_ca && id.chain[0].is_self_signed());
    assert!(id.cert.signed_by(&id.chain[0].public_key), "the leaf is issued by the root");
    assert!(!id.cert.is_self_signed());
    let pool = [id.chain[0].clone()];
    assert_eq!(x509::build_chain(&id.cert, &pool, None).len(), 2);
    assert_eq!(pkcs12::open(RSA_AES, "test").unwrap().friendly_name.as_deref(), Some("Test Signer RSA"));
}

#[test]
fn certificates_parse_load_and_export_as_pem() {
    let certs = x509::load_certificates(RSA_CRT_PEM).unwrap();
    assert_eq!(certs.len(), 1);
    let c = &certs[0];
    assert_eq!(c.subject.display(), "CN=Test Signer RSA, O=PrintCraft Tests, C=US");
    assert_eq!(c.serial_hex(), "03E9");
    assert!(c.is_self_signed());
    assert_eq!(x509::load_certificates(&c.raw).unwrap()[0], *c);
    let pem = x509::to_pem(c);
    assert_eq!(x509::load_certificates(pem.as_bytes()).unwrap()[0], *c);
    assert!(x509::load_certificates(b"hello").is_err());
}

#[test]
fn external_keys_cannot_be_exported_and_unsupported_things_say_so() {
    // A PKCS #12 write needs the key's PKCS #8 bytes, which a key-store key doesn't have.
    struct Never;
    impl super::keys::ExternalKey for Never {
        fn sign(&self, _: DigestAlg, _: &[u8]) -> Result<Vec<u8>, SignError> {
            Err(SignError::Crypto("never".into()))
        }
    }
    let id = new_id("External");
    let ext = DigitalId {
        key: PrivateKey::external(id.key.public_key().clone(), std::sync::Arc::new(Never)),
        cert: id.cert.clone(),
        chain: Vec::new(),
        friendly_name: None,
    };
    assert!(ext.key.is_external());
    assert!(matches!(pkcs12::write(&ext, "pw"), Err(SignError::Unsupported(_))));
    assert!(matches!(PrivateKey::generate_rsa(1024), Err(SignError::Unsupported(_))));
    assert!(DigitalId::create_self_signed("  ", "", "", 1).is_err());

    // Ed448 and other unknown key types parse (for their name) and refuse to verify instead of saying "wrong".
    let spki = der::seq(&[&der::seq(&[&der::oid("1.3.101.113")]), &der::bit_string(&[7u8; 57])]);
    let key = PublicKey::from_spki(&Tlv::parse_all(&spki).unwrap()).unwrap();
    assert_eq!(key.spki(), spki);
    assert!(key.describe().contains("unsupported"));
    assert!(matches!(key.verify(Scheme::Ecdsa, DigestAlg::Sha256, &[0; 32], &[0; 64]), Err(SignError::Unsupported(_))));
    // P-521 and the Brainpool curves likewise: Unsupported, not a wrong signature.
    for curve in ["1.3.132.0.35", "1.3.36.3.3.2.8.1.1.7"] {
        let spki = der::seq(&[&der::seq(&[&der::oid("1.2.840.10045.2.1"), &der::oid(curve)]), &der::bit_string(&[4u8; 65])]);
        let key = PublicKey::from_spki(&Tlv::parse_all(&spki).unwrap()).unwrap();
        assert!(matches!(key.verify(Scheme::Ecdsa, DigestAlg::Sha256, &[0; 32], &[0; 64]), Err(SignError::Unsupported(_))));
        assert_eq!(key.store_signing_key(), None);
    }
    // SHA-3 and RIPEMD-160 are not digests here.
    assert_eq!(DigestAlg::from_oid("2.16.840.1.101.3.4.2.8"), None);
    assert_eq!(DigestAlg::from_oid("1.3.36.3.2.1"), None);
}

// ── malformed input never panics ───────────────────────────────────────────────────────────────

#[test]
fn malformed_inputs_are_errors_not_panics() {
    let junk: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![0x30],
        vec![0x30, 0x80],
        vec![0x30, 0x84, 0xFF, 0xFF, 0xFF, 0xFF],
        vec![0x30, 0x03, 0x02, 0x01],
        vec![0x1F, 0x81, 0x00, 0x00],
        b"-----BEGIN PKCS12-----".to_vec(),
        b"-----BEGIN PKCS12-----\n-----END PKCS12-----\n".to_vec(),
        b"not der at all".to_vec(),
        vec![0xFF; 300],
        [vec![0x30, 0x80], [0x30u8, 0x80].repeat(500), [0u8, 0].repeat(500)].concat(),
    ];
    for j in &junk {
        assert!(Tlv::parse_all(j).is_err() || j.len() > 10, "{j:?}");
        let _ = Tlv::parse_all_ber(j).map(|t| (t.children(), t.oid(), t.time(), t.bits(), t.u64()));
        assert!(Certificate::parse(j).is_err());
        assert!(cms::SignedData::parse(j).is_err());
        assert!(pkcs12::open(j, "x").is_err());
        assert!(PrivateKey::from_pkcs8(j).is_err());
        assert!(x509::load_certificates(j).is_err());
        let _ = der::try_oid("1..2");
    }
    assert!(der::try_oid("1.2.x").is_err() && der::try_oid("3.1").is_err() && der::try_oid("1").is_err());

    // Every single-byte corruption and every truncation of real structures still ends in Ok or Err.
    let id = pkcs12::open(CHAIN, "test").unwrap();
    let cert = id.cert.raw.clone();
    let digest = DigestAlg::Sha256.digest(&[b"x"]);
    let sig = cms::sign_detached(&id.key, &id.cert, &id.chain, DigestAlg::Sha256, &digest).unwrap();
    for original in [&cert, &sig] {
        for i in 0..original.len() {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut m = original.to_vec();
                m[i] ^= flip;
                if let Ok(sd) = cms::SignedData::parse(&m) {
                    let _ = sd.signer_certificate().map(|c| sd.verify_signature(c, &digest));
                }
                let _ = Certificate::parse(&m);
            }
            let _ = cms::SignedData::parse(&original[..i]);
            let _ = Certificate::parse(&original[..i]);
        }
    }
    for (bytes, step) in [(RSA_AES, 5), (RSA_LEGACY, 5), (EC_P384, 5)] {
        for i in (0..bytes.len()).step_by(step) {
            let mut m = bytes.to_vec();
            m[i] ^= 0x55;
            let _ = pkcs12::open(&m, "test");
        }
        for len in (0..bytes.len()).step_by(37) {
            assert!(pkcs12::open(&bytes[..len], "test").is_err());
        }
    }
    let err = pkcs12::open(&RSA_AES[..64], "test").expect_err("truncated file");
    assert!(err.to_string().contains("runs past the end"), "{err}");
}

// ── PKCS #12 forms ─────────────────────────────────────────────────────────────────────────────

/// One element with BER's indefinite length: contents, then the end-of-contents octets.
fn indefinite(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag, 0x80];
    v.extend_from_slice(body);
    v.extend([0, 0]);
    v
}

/// An OCTET STRING split into segments, BER's constructed form.
fn segmented(bytes: &[u8]) -> Vec<u8> {
    let parts: Vec<u8> = bytes.chunks(64).flat_map(der::octets).collect();
    indefinite(0x24, &parts)
}

/// A `.p12` in the shape Windows writes one: indefinite lengths, the AuthenticatedSafe in segments.
fn as_windows_writes_it(file: &[u8]) -> Vec<u8> {
    let pfx = Tlv::parse_all(file).unwrap().children().unwrap();
    let auth = pfx[1].children().unwrap();
    let content = auth[1].inner().unwrap().value.to_vec();
    let zero = indefinite(0xA0, &segmented(&content));
    let mut body = pfx[0].raw.to_vec();
    body.extend(indefinite(0x30, &[auth[0].raw, zero.as_slice()].concat()));
    for extra in &pfx[2..] {
        body.extend_from_slice(extra.raw);
    }
    indefinite(0x30, &body)
}

#[test]
fn pkcs12_opens_windows_ber_and_wrapped_files() {
    use base64::Engine as _;
    for file in [RSA_AES, RSA_LEGACY, CHAIN] {
        let want = pkcs12::open(file, "test").unwrap();
        let ber = as_windows_writes_it(file);
        assert_ne!(ber, file);
        let got = pkcs12::open(&ber, "test").unwrap();
        assert_eq!(got.cert.raw, want.cert.raw);
        assert_eq!(got.chain.len(), want.chain.len());
        assert!(matches!(pkcs12::open(&ber, "nope"), Err(SignError::WrongPassword)));
    }
    let signer = pkcs12::open(RSA_AES, "test").unwrap();
    let mut trailing = RSA_AES.to_vec();
    trailing.extend_from_slice(b"\r\n \n");
    assert_eq!(pkcs12::open(&trailing, "test").unwrap().cert, signer.cert);
    let b64 = base64::engine::general_purpose::STANDARD.encode(RSA_AES);
    let mut pem = String::from("-----BEGIN PKCS12-----\n");
    for line in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END PKCS12-----\n");
    assert_eq!(pkcs12::open(pem.as_bytes(), "test").unwrap().cert, signer.cert);
    assert_eq!(pkcs12::open(b64.as_bytes(), "test").unwrap().cert, signer.cert);
}

// ── RSA and ECDSA signature encodings ──────────────────────────────────────────────────────────

// RSA signatures over SHA-256("hello") by the key in rsa-aes.p12, made with OpenSSL 3 (pdfcraft tests/data/README.md):
// the standard DigestInfo, DigestInfo without the NULL parameter, the bare digest (refused), and RSASSA-PSS with a
// zero-length salt.
const RSA_STD: &str = "40896f76bc460e4ae9dc523dfe136159c7ccc20eff09bfeeafd570fd41a6c016d6b2a1d771ba81642dd5bf94d2bb9c4aeead237ce0e1662678efd5ff7b8590bf7d5042f6541b001a9f7d688e60a9fde5c9b3f295bd5373aea1059b53f1341f30a4db68302161e95c6a62847205782edcd1fa99b6720eaa60b59d3f54d4ec7be396c8f1146fe35f198bd12dca608b8080a446f89ba300fd74f8df4e7f177c59dc48cf878451423dd9674df768a12f85a1b5e81a5b8df43ac0fb03c4393cfed7dceeeddf1c8aef9a37e738ff4169a6881e783c2f841fb974eb0e6ecedf649bf64fbc51920b7824d86b26527acaec5d0ebb44454606441fea2b7d7b7a6014ab9d64";
const RSA_NO_NULL: &str = "a6589a056b04903cd77d5dae3b52f8c9905403ee2cef2e7343cfdd07746cbf1034c8c510a6f7258face9cfbcacb990e30f185dd091310d88efc3457e0fd34245d525e2cb5ceaab029e7b7fd25a581379c92e6abf90a36388cfc6afba0be47c99abc8b32e82a2c750ffe7c6d04de886cbfe63685cfc0f0204f641103e3c752496a0ae341fb503c4c2389824f7f20d07a03f0e3a95d5084fc1fb1b94c6bc607a14623b1bf7180b3d7d4592e217ba08698becb1994a6ef72ccab6e239418f11c57f184f59b0d1c7407216856cd4585da6a07354191ae85f8be02e670a10017f115ccb55d96f2ea2c1f095bbcf6b50ed8763945b78054989b016119c5371d918d682";
const RSA_BARE: &str = "a5812c29d3edf0b567636aaa501a960b977e774a0ea941e41401611d8b63464e99e2e908d8d8452b97112597f06f2adee029109bf6e421eb57add689b518532232199151ed822d1766013ea44ae5cb468b4a2288cf866f8c5954b729bfd34a8ca82d73bb397a75eb76687f7d9e506dab32e2da6231ece307fe8eb929c7648f73de6dd81c38aecb78a0e7ef0e11654fbadef88c7b206079afee70db8054ada079c6456826383dfd2504f05033b58cf1610a180953e501e137b47de5a77914cfaa33b8c588f24dcdb5789e7d1288ff009918f6c2808bfa1c57398d2e007a5e6d3004a3ca568a4ae2eb5cfa14083099e81cfcb53ead9ec6650a7c58e441d117d439";
const RSA_PSS_SALT0: &str = "2cdbb02d75a80df07dcca229363644caa4391cca30a3bc1281a3f4512dbe609727767c42e62cdad7d6bd38088b2c0b5dcb439fd2c337c31b47bbc39bda0e5c1486a773cde163fa2cad8047c06217541c993b21413e5cd341a4f66d2fb6e4755b52d25935dd90af667ce3d3e1825c8e6e40274dca7cf469ae17cbc3c1c1560d87a877a46353245128120beb1108cd0cafddae5b4ef9005601cc05f114da9de4cdaee08020d5b9be27e7b48f961e40664ef592990580bc1e1f2a8107c3d71d323df120720be6e862191a0aaa3b394b96efd2f45afa5f8211e2082865b8d3e3c28f99054976162ae8f42a903bfb82c17700db93fef603c4cd85c70a0be68fda9af8";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn rsa_signatures_are_read_with_every_digestinfo_variant() {
    let id = pkcs12::open(RSA_AES, "test").unwrap();
    let key = &id.cert.public_key;
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    for (name, sig) in [("standard", RSA_STD), ("no NULL", RSA_NO_NULL)] {
        assert!(key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &unhex(sig)).unwrap(), "{name}");
    }
    assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &unhex(RSA_BARE)).unwrap(), "bare digest");
    let pss = |salt_len| Scheme::RsaPss(PssParams { mgf: DigestAlg::Sha256, salt_len });
    assert!(key.verify(pss(Some(0)), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap());
    assert!(key.verify(pss(None), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap(), "salt length recovered");
    assert!(!key.verify(pss(Some(32)), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap(), "declared salt length is enforced");
    let other = DigestAlg::Sha256.digest(&[b"hellp"]);
    let mut flipped = unhex(RSA_STD);
    flipped[7] ^= 1;
    for sig in [RSA_STD, RSA_NO_NULL, RSA_BARE] {
        assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &other, &unhex(sig)).unwrap());
    }
    assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &flipped).unwrap());
    assert!(!key.verify(pss(None), DigestAlg::Sha256, &digest, &unhex(RSA_STD)).unwrap());
    // A signature of the rsa crate (blinded, our own padding) is the one OpenSSL would make: same bytes.
    let ours = id.key.sign(DigestAlg::Sha256, b"hello").unwrap();
    assert_eq!(ours, unhex(RSA_STD), "PKCS #1 v1.5 signatures are deterministic");

    // Hostile signature values are wrong, not crashes.
    let PublicKey::Rsa { n, .. } = key else { panic!("an RSA key") };
    let mut one_more = vec![1u8];
    one_more.extend(vec![0u8; n.len()]);
    let pss32 = pss(Some(32));
    for sig in [vec![], vec![0], vec![0; n.len()], vec![0xFF; n.len()], vec![0xFF; n.len() + 40], n.clone(), one_more] {
        for scheme in [Scheme::RsaPkcs1, pss32] {
            assert!(!key.verify(scheme, DigestAlg::Sha256, &digest, &sig).unwrap(), "{} bytes", sig.len());
        }
    }
    assert!(!key.verify(pss32, DigestAlg::Sha256, &[1, 2, 3], &unhex(RSA_STD)).unwrap());
}

#[test]
fn ecdsa_signatures_are_read_as_der_non_minimal_der_or_raw() {
    for (bytes, field) in [(EC_P256, 32usize), (EC_P384, 48)] {
        let id = pkcs12::open(bytes, "test").unwrap();
        let alg = id.key.preferred_digest();
        let msg = b"signed content";
        let digest = alg.digest(&[msg]);
        let sig = id.key.sign(alg, msg).unwrap();
        let pk = id.key.public_key();
        let mut notes = Vec::new();
        assert!(pk.verify_noting(Scheme::Ecdsa, alg, &digest, &sig, &mut notes).unwrap());
        assert!(notes.is_empty(), "DER as made needs no excuse: {notes:?}");
        let ints = Tlv::parse_all(&sig).unwrap().children().unwrap();
        let fixed = |i: &Tlv<'_>| {
            let m = i.uint_bytes();
            let mut v = vec![0u8; field - m.len()];
            v.extend_from_slice(m);
            v
        };
        let (r, s) = (fixed(&ints[0]), fixed(&ints[1]));
        let raw = [r.clone(), s.clone()].concat();
        let mut notes = Vec::new();
        assert!(pk.verify_noting(Scheme::Ecdsa, alg, &digest, &raw, &mut notes).unwrap(), "raw r‖s");
        assert!(matches!(notes.as_slice(), [n] if n.contains("raw")), "{notes:?}");
        let padded = |v: &[u8]| der::tlv(0x02, &[&[0u8, 0][..], v].concat());
        let mut notes = Vec::new();
        assert!(pk.verify_noting(Scheme::Ecdsa, alg, &digest, &der::seq(&[&padded(&r), &padded(&s)]), &mut notes).unwrap(), "non-minimal DER");
        assert!(matches!(notes.as_slice(), [n] if n.contains("canonical DER")), "{notes:?}");
        let mut bad = raw.clone();
        bad[3] ^= 0x40;
        assert!(!pk.verify(Scheme::Ecdsa, alg, &digest, &bad).unwrap());
        assert!(!pk.verify(Scheme::Ecdsa, alg, &DigestAlg::Sha256.digest(&[b"other"]), &raw).unwrap());
        assert!(!pk.verify(Scheme::Ecdsa, alg, &digest, &[1, 2, 3]).unwrap());
    }
}

#[test]
fn only_sha1_is_called_weak() {
    for alg in DigestAlg::ALL {
        match alg {
            DigestAlg::Sha1 => assert!(alg.weakness().unwrap().contains("SHA-1")),
            _ => assert_eq!(alg.weakness(), None, "{}", alg.name()),
        }
    }
}

// ── what a key store may sign with, and how its signatures become CMS ones (any platform) ───────

#[test]
fn key_store_signatures_become_cms_signatures_that_verify() {
    use p256::ecdsa::signature::hazmat::PrehashSigner;
    let msg = b"hello";
    let k = p256::ecdsa::SigningKey::from_slice(&[0x11; 32]).unwrap();
    let public = PublicKey::P256(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
    assert_eq!(public.store_signing_key(), Some(StoreKey::Ecdsa(256)));
    let digest = DigestAlg::Sha256.digest(&[msg]);
    let s: p256::ecdsa::Signature = k.sign_prehash(&digest).unwrap();
    // CNG returns r ‖ s, 32 bytes each for P-256.
    let der = public.store_signature_der(&s.to_bytes()).unwrap();
    assert_eq!(der.first(), Some(&0x30), "a DER SEQUENCE");
    assert!(public.verify(Scheme::Ecdsa, DigestAlg::Sha256, &digest, &der).unwrap());

    let rsa = PublicKey::Rsa { n: vec![0xc5; 256], e: vec![1, 0, 1] };
    assert_eq!(rsa.store_signing_key(), Some(StoreKey::Rsa));
    assert_eq!(rsa.store_signature_der(&[9; 256]).unwrap(), vec![9; 256], "RSA PKCS #1 v1.5 is already what CMS carries");

    // Wrong lengths, and right lengths whose r and s are zero or not below the group order.
    let p256: [&[u8]; 5] = [&[], &[1; 63], &[1; 65], &[0; 64], &[0xff; 64]];
    for raw in p256 {
        assert!(matches!(PublicKey::P256(Vec::new()).store_signature_der(raw), Err(SignError::Crypto(_))), "{} bytes", raw.len());
    }
}

// ── trust ──────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_self_signed_certificate_is_an_untrusted_root() {
    let id = new_id("Untrusted Test Signer");
    let no_extras: &[&[u8]] = &[];
    assert_eq!(trust::check(&id.cert.raw, no_extras, None), Trust::UntrustedRoot);
    // The same at a time inside and long before its validity: still not trusted, and never a panic.
    let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(86_400 * 365);
    assert_ne!(trust::check(&id.cert.raw, no_extras, Some(long_ago)), Trust::Trusted);
    // Garbage is "other", not a crash.
    assert!(!matches!(trust::check(b"junk", no_extras, None), Trust::Trusted));
}

// ── Windows: CNG signing through an ephemeral, in-memory import (no real store is touched) ─────

#[cfg(windows)]
#[test]
fn an_ephemeral_pfx_import_signs_through_cng() {
    use super::winstore;
    let dir = TempDir::new("cng");
    let id = new_id("CNG Test Signer");
    let file = dir.path("test.pfx");
    std::fs::write(&file, pkcs12::write(&id, "pw1").unwrap()).unwrap();
    let bytes = std::fs::read(&file).unwrap();

    assert!(matches!(winstore::import_pfx_ephemeral(&bytes, "wrong"), Err(SignError::WrongPassword)));
    let imported = winstore::import_pfx_ephemeral(&bytes, "pw1").unwrap();
    assert!(imported.key.is_external(), "the key stays in CNG");
    assert!(imported.key.pkcs8().is_empty(), "and cannot be read back");
    assert_eq!(imported.cert, id.cert);
    assert!(matches!(pkcs12::write(&imported, "x"), Err(SignError::Unsupported(_))));

    // RSA PKCS #1 v1.5 is deterministic: CNG and the rsa crate must produce the same signature.
    for alg in [DigestAlg::Sha256, DigestAlg::Sha384, DigestAlg::Sha512] {
        assert_eq!(imported.key.sign(alg, b"hello").unwrap(), id.key.sign(alg, b"hello").unwrap(), "{}", alg.name());
    }
    let digest = DigestAlg::Sha256.digest(&[b"range one", b"range two"]);
    let cms_der = cms::sign_detached(&imported.key, &imported.cert, &imported.chain, DigestAlg::Sha256, &digest).unwrap();
    let sd = cms::SignedData::parse(&cms_der).unwrap();
    let signer = sd.signer_certificate().unwrap();
    assert!(sd.verify_signature(signer, &digest).unwrap());
    let mut tampered = sd.clone();
    tampered.signer.signature[0] ^= 1;
    assert!(!tampered.verify_signature(signer, &digest).unwrap());
}

#[cfg(windows)]
#[test]
fn an_ephemeral_p256_pfx_signs_through_cng() {
    use super::winstore;
    let key = PrivateKey::generate_p256().unwrap();
    let now = Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    let cert = Certificate::self_signed(&Name::build("CNG P-256", "", "", "", ""), &key, now, 30, &[7, 7, 7]).unwrap();
    let id = DigitalId { key, cert, chain: Vec::new(), friendly_name: None };
    let imported = winstore::import_pfx_ephemeral(&pkcs12::write(&id, "pw").unwrap(), "pw").unwrap();
    let digest = DigestAlg::Sha256.digest(&[b"p256 content"]);
    let cms_der = cms::sign_detached(&imported.key, &imported.cert, &[], DigestAlg::Sha256, &digest).unwrap();
    let sd = cms::SignedData::parse(&cms_der).unwrap();
    assert!(sd.verify_signature(&imported.cert, &digest).unwrap());
}
