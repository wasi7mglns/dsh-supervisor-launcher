//! Release artifact acceptance (local end-to-end rehearsal) -- 2026-09-11
//!
//! Goal: prove that what OUR release pipeline produces is exactly what
//! Tauri updater will accept -- without installing anything or touching the
//! user system.
//!
//! Uses Tauri own dependencies on purpose:
//!   * signature: `minisign-verify` -- the SAME crate tauri-plugin-updater uses
//!     (its internal verify_signature does base64-decode -> PublicKey::decode -> verify)
//!   * manifest: `tauri_plugin_updater::RemoteRelease` -- parsed by Tauri own Deserialize
//!
//! NOTE: all log messages are ASCII on purpose. Rust 2021 lexes a fullwidth
//! character adjacent to a `{}` placeholder as a prefixed identifier and fails.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Mirror of Tauri internal `base64_to_string`: base64-decode, then UTF-8 text.
fn base64_to_string(s: &str) -> String {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .expect("base64 decode failed");
    String::from_utf8(decoded).expect("base64 payload is not UTF-8")
}

/// Read the updater pubkey from tauri.conf.json (config is source of truth).
fn configured_pubkey() -> String {
    let p = repo_root().join("tauri.conf.json");
    let raw = std::fs::read_to_string(&p).expect("read tauri.conf.json failed");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("tauri.conf.json invalid JSON");
    v["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("tauri.conf.json missing plugins.updater.pubkey")
        .to_string()
}

/// Same semantics as Tauri verify_signature.
fn verify_like_tauri(data: &[u8], signature_b64: &str, pubkey_b64: &str) -> Result<(), String> {
    let pk_text = base64_to_string(pubkey_b64);
    let public_key = PublicKey::decode(&pk_text).map_err(|e| format!("pubkey decode: {e}"))?;
    let sig_text = base64_to_string(signature_b64);
    let signature = Signature::decode(&sig_text).map_err(|e| format!("sig decode: {e}"))?;
    public_key
        .verify(data, &signature, true)
        .map_err(|e| format!("verify: {e}"))
}

/// Locate an updatable artifact plus its .sig in the build output.
fn find_artifact() -> Option<(PathBuf, PathBuf)> {
    let dirs = [
        repo_root().join("target/release/bundle/deb"),
        repo_root().join("target/release/bundle/rpm"),
    ];
    let mut best: Option<(PathBuf, PathBuf)> = None;
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.ends_with(".deb") || name.ends_with(".rpm") {
                let sig = PathBuf::from(format!("{}.sig", p.display()));
                if sig.exists() {
                    let bigger = best.as_ref().map_or(true, |(b, _)| {
                        std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
                            > std::fs::metadata(b).map(|m| m.len()).unwrap_or(0)
                    });
                    if bigger {
                        best = Some((p, sig));
                    }
                }
            }
        }
    }
    best
}

fn build_manifest(artifact: &Path, sig_text: &str, version: &str) -> String {
    let file = artifact.file_name().unwrap().to_string_lossy().to_string();
    let url = format!(
        "https://unpkg.com/@dsh-sup/shell-linux-x64@{version}/artifact/{file}"
    );
    serde_json::json!({
        "version": version,
        "notes": "local rehearsal manifest",
        "pub_date": "2026-09-11T00:00:00Z",
        "platforms": {
            "linux-x86_64": { "url": url, "signature": sig_text }
        }
    })
    .to_string()
}

#[test]
fn v1_manifest_parses_with_tauri_own_deserializer() {
    let Some((artifact, sig_path)) = find_artifact() else {
        eprintln!("SKIP no built artifact found; run tauri build --bundles deb");
        return;
    };
    let sig_text = std::fs::read_to_string(&sig_path).expect("read sig failed").trim().to_string();
    let manifest = build_manifest(&artifact, &sig_text, "0.2.0");

    let release: tauri_plugin_updater::RemoteRelease =
        serde_json::from_str(&manifest).expect("Tauri cannot parse our manifest");

    assert_eq!(release.version.to_string(), "0.2.0", "version parse mismatch");
    let url = release.download_url("linux-x86_64").expect("missing linux-x86_64");
    let fname = artifact.file_name().unwrap().to_string_lossy().to_string();
    assert!(url.as_str().contains(&fname), "url does not point at artifact");
    let sig = release.signature("linux-x86_64").expect("missing signature");
    assert_eq!(sig.trim(), sig_text, "manifest signature differs from sig file");
    eprintln!("V1 PASS manifest parsed by Tauri RemoteRelease");
}

#[test]
fn v2_signature_verifies_with_configured_pubkey() {
    let Some((artifact, sig_path)) = find_artifact() else {
        eprintln!("SKIP no built artifact found");
        return;
    };
    let data = std::fs::read(&artifact).expect("read artifact failed");
    let sig_text = std::fs::read_to_string(&sig_path).expect("read sig failed");
    let pubkey = configured_pubkey();

    if let Err(e) = verify_like_tauri(&data, &sig_text, &pubkey) {
        panic!("V2 FAIL signature does not match configured pubkey: {e}");
    }
    eprintln!("V2 PASS signature verified with config pubkey, {} bytes", data.len());
}

#[test]
fn v3_tampered_artifact_must_fail_verification() {
    let Some((artifact, sig_path)) = find_artifact() else {
        eprintln!("SKIP no built artifact found");
        return;
    };
    let mut data = std::fs::read(&artifact).expect("read artifact failed");
    let sig_text = std::fs::read_to_string(&sig_path).expect("read sig failed");
    let pubkey = configured_pubkey();

    let mid = data.len() / 2;
    data[mid] ^= 0xFF;

    let r = verify_like_tauri(&data, &sig_text, &pubkey);
    assert!(r.is_err(), "V3 FAIL tampered artifact verified");
    eprintln!("V3 PASS tampered artifact rejected: {}", r.unwrap_err());
}

#[test]
fn v4_wrong_pubkey_must_fail_verification() {
    let Some((artifact, sig_path)) = find_artifact() else {
        eprintln!("SKIP no built artifact found");
        return;
    };
    let data = std::fs::read(&artifact).expect("read artifact failed");
    let sig_text = std::fs::read_to_string(&sig_path).expect("read sig failed");

    let pk_text = base64_to_string(&configured_pubkey());
    let mut bytes = pk_text.clone().into_bytes();
    if let Some(last) = bytes.last_mut() {
        *last = if *last == b'A' { b'B' } else { b'A' };
    }
    let altered = String::from_utf8(bytes).expect("altered pubkey not utf8");
    let altered_b64 = base64::engine::general_purpose::STANDARD.encode(altered.as_bytes());

    let r = verify_like_tauri(&data, &sig_text, &altered_b64);
    assert!(r.is_err(), "V4 FAIL verification passed with a different pubkey");
    eprintln!("V4 PASS verification failed with wrong pubkey");
}

#[test]
fn v5_pubkey_comes_from_config_and_is_wellformed() {
    let pk = configured_pubkey();
    assert!(!pk.is_empty(), "config has no pubkey");
    assert_eq!(pk.len(), 152, "pubkey length unexpected: {}", pk.len());

    let text = base64_to_string(&pk);
    assert!(text.contains("minisign"), "pubkey text lacks minisign marker");
    PublicKey::decode(&text).expect("configured pubkey cannot be decoded");
    eprintln!("V5 PASS configured pubkey well-formed, {} chars", pk.len());
}

// ---------------------------------------------------------------------------
// V6: the REAL toolchain output (generated by shell-release/make-manifest.js +
// assemble-shell-pkg.js) must round-trip through Tauri own deserializer, and the
// signature it carries must verify against the artifact it points to.
//
// This is the test that closes the loop between our JS release tools and Tauri.
// Controlled by env var SHELL_REHEARSAL_DIR so normal runs stay self-contained;
// CI sets it after assembling a release.
// ---------------------------------------------------------------------------
#[test]
fn v6_real_toolchain_manifest_roundtrip() {
    let Ok(dir) = std::env::var("SHELL_REHEARSAL_DIR") else {
        eprintln!("SKIP SHELL_REHEARSAL_DIR not set");
        return;
    };
    let dir = PathBuf::from(dir);
    let manifest_path = dir.join("shell-manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", manifest_path.display()));

    // 1) Tauri own deserializer must accept the toolchain output
    let release: tauri_plugin_updater::RemoteRelease =
        serde_json::from_str(&raw).expect("V6 FAIL Tauri rejected toolchain manifest");

    // 2) platform key + url + signature present
    let url = release.download_url("linux-x86_64").expect("V6 FAIL missing linux-x86_64");
    let sig = release.signature("linux-x86_64").expect("V6 FAIL missing signature");

    // 3) the artifact referenced by the manifest exists locally and verifies
    let fname = url.path_segments().and_then(|s| s.last()).unwrap_or("");
    let artifact = dir.join("@dsh-sup/shell-linux-x64/artifact").join(fname);
    let data = std::fs::read(&artifact)
        .unwrap_or_else(|e| panic!("V6 FAIL read {} failed: {e}", artifact.display()));
    let pubkey = configured_pubkey();
    if let Err(e) = verify_like_tauri(&data, sig, &pubkey) {
        panic!("V6 FAIL toolchain signature does not verify: {e}");
    }

    eprintln!(
        "V6 PASS toolchain manifest accepted by Tauri and signature verified ({} bytes)",
        data.len()
    );
}
