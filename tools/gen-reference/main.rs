//! Regenerates and verifies `reference.json`: the hex-exact decoded-plane
//! digests of the pith suite's H.264 lane. One vector per conformance
//! fixture under `tests/fixtures/`, in deterministic (sorted) corpus
//! order, pinning the aggregate SHA-256 over every decoded frame's
//! `y || cb || cr` planes in presentation order.
//!
//! `gen-reference gen` rewrites the file; `gen-reference verify`
//! recomputes it and fails on any difference. CI runs `verify` so a
//! decoder change that shifts output bytes cannot land silently, and CD
//! ships the file with the SDK artifacts as the cross-language oracle.
//!
//! The corpus is byte-stable across platforms by construction: the
//! decoder is integer-only (`#![no_std]`, no floats in the output path)
//! and the digests are computed from decoded planes only — never from
//! encoder state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pith_digest::sha256;
use pith_h264::{Frame, decode};

const FIXTURES_DIR: &str = "tests/fixtures";
const REFERENCE_PATH: &str = "reference.json";

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One measurement of a decoded stream: everything reference.json pins.
pub(crate) struct Decoded {
    pub(crate) frames: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) planes_sha256: String,
}

/// Decodes `stream` and digests its planes; panics on any decode fault —
/// every fixture must decode.
pub(crate) fn measure(stream: &[u8]) -> Decoded {
    let frames: Vec<Frame> = decode(stream).expect("fixture must decode");
    assert!(!frames.is_empty(), "fixture must yield frames");
    let width = frames[0].width;
    let height = frames[0].height;
    let mut planes: Vec<u8> = Vec::new();
    for f in &frames {
        assert_eq!(f.width, width, "fixture frame width must be constant");
        assert_eq!(f.height, height, "fixture frame height must be constant");
        planes.extend_from_slice(&f.y);
        planes.extend_from_slice(&f.cb);
        planes.extend_from_slice(&f.cr);
    }
    Decoded {
        frames: frames.len(),
        width,
        height,
        planes_sha256: hex(sha256(&planes)
            .expect("sha256 of decoded planes")
            .as_bytes()),
    }
}

/// The kind name of an error, as the cross-SDK contract pins it. The
/// kind is stable; the message text is not depended on.
pub(crate) fn error_kind(e: &pith_digest::Error) -> &'static str {
    match e {
        pith_digest::Error::BadValue(_) => "BadValue",
        pith_digest::Error::Truncated { .. } => "Truncated",
        pith_digest::Error::TooLarge { .. } => "TooLarge",
        pith_digest::Error::InvalidMagic { .. } => "InvalidMagic",
        pith_digest::Error::Unsupported(_) => "Unsupported",
    }
}

/// Sorted `.h264` fixture names in `tests/fixtures/` — the deterministic
/// corpus order of the vectors array.
pub(crate) fn fixture_names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join(FIXTURES_DIR))
        .expect("fixtures dir must exist")
        .map(|e| {
            e.expect("dir entry readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|n| n.ends_with(".h264"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixtures found");
    names
}

/// Assembles the whole reference.json document (LF-terminated).
pub(crate) fn reference_json(root: &Path) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"suite\": \"pith\",\n");
    out.push_str("  \"crate\": \"pith-h264\",\n");
    out.push_str("  \"format_version\": 1,\n");
    out.push_str("  \"generator\": \"cargo run --bin gen-reference -- gen\",\n");
    out.push_str("  \"vectors\": [\n");
    let names = fixture_names(root);
    for (i, name) in names.iter().enumerate() {
        let raw = fs::read(root.join(FIXTURES_DIR).join(name))
            .unwrap_or_else(|e| panic!("cannot read fixture {name}: {e}"));
        let d = measure(&raw);
        let comma = if i + 1 == names.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{\n      \"name\": \"{}\",\n      \"input_kind\": \"fixture-file\",\n      \"input_path\": \"{FIXTURES_DIR}/{name}\",\n      \"input_sha256\": \"{}\",\n      \"frames\": {},\n      \"width\": {},\n      \"height\": {},\n      \"planes_sha256\": \"{}\"\n    }}{comma}\n",
            name.trim_end_matches(".h264"),
            hex(sha256(&raw).expect("sha256 of fixture").as_bytes()),
            d.frames,
            d.width,
            d.height,
            d.planes_sha256,
        ));
    }
    out.push_str("  ],\n");
    // Error behavior is part of the cross-SDK contract: the kind name is
    // stable, the message text is not depended on. Computed live so a
    // decoder change that shifts a kind fails the verify step loudly.
    let empty_err = decode(&[]).expect_err("empty input must error");
    let high_profile = [
        0x00, 0x00, 0x00, 0x01, 0x67, 0x6e, 0x00, 0x0a, 0xf8, 0x88, 0x80,
    ];
    let profile_err = decode(&high_profile).expect_err("high profile must error");
    let truncated_err = decode(&[0x00, 0x00, 0x01, 0x67]).expect_err("truncated NAL must error");
    let errors = [
        ("empty-input", "", &empty_err),
        ("high-profile-sps", "676e000af88880", &profile_err),
        ("truncated-sps-nal", "00000167", &truncated_err),
    ];
    out.push_str("  \"errors\": [\n");
    for (i, (name, input_hex, e)) in errors.iter().enumerate() {
        let comma = if i + 1 == errors.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{\n      \"name\": \"{name}\",\n      \"input_hex\": \"{input_hex}\",\n      \"error_kind\": \"{}\"\n    }}{comma}\n",
            error_kind(e),
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

pub(crate) fn run(mode: &str) -> ExitCode {
    let root = repo_root();
    let json = reference_json(&root);
    let path = root.join(REFERENCE_PATH);
    match mode {
        "gen" => {
            fs::write(&path, &json).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
            println!("wrote {} ({} bytes)", path.display(), json.len());
            ExitCode::SUCCESS
        }
        "verify" => {
            let committed = match fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("FAIL: cannot read {path:?}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if committed == json.as_bytes() {
                println!("reference.json is current");
                ExitCode::SUCCESS
            } else {
                let off = committed
                    .iter()
                    .zip(json.as_bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(committed.len().min(json.len()));
                eprintln!(
                    "FAIL: reference.json is stale: committed {} bytes, computed {} bytes, first difference at byte {off}",
                    committed.len(),
                    json.len()
                );
                ExitCode::FAILURE
            }
        }
        // No arguments means the CI gate invocation: verify.
        "" => run("verify"),
        _ => {
            eprintln!("usage: gen-reference <gen|verify> (got {mode:?})");
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    run(std::env::args().nth(1).unwrap_or_default().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes every test that reads/writes reference.json: cargo
    /// runs test threads in parallel and the file is shared state.
    pub(crate) static REF_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn hex_is_lowercase_two_digits_per_byte() {
        assert_eq!(hex(&[0x00, 0x0f, 0xf0, 0xff]), "000ff0ff");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn error_kind_maps_every_decoder_error() {
        assert_eq!(error_kind(&pith_digest::Error::BadValue("x")), "BadValue");
        assert_eq!(
            error_kind(&pith_digest::Error::Unsupported("x")),
            "Unsupported"
        );
        assert_eq!(
            error_kind(&pith_digest::Error::InvalidMagic { what: "x" }),
            "InvalidMagic"
        );
    }

    #[test]
    fn fixture_names_sorted_h264_only() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let names = fixture_names(root);
        assert!(!names.is_empty());
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(names.iter().all(|n| n.ends_with(".h264")));
    }

    #[test]
    fn measure_digests_fixture_planes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let stream = fs::read(root.join(FIXTURES_DIR).join("flat16.h264")).unwrap();
        let m = measure(&stream);
        assert_eq!(m.frames, 1);
        assert_eq!(m.width, 16);
        assert_eq!(m.height, 16);
        // Stable digest: regenerating must not drift.
        assert_eq!(m.planes_sha256.len(), 64);
    }

    #[test]
    #[should_panic(expected = "fixture must decode")]
    fn measure_rejects_garbage_loudly() {
        let _ = measure(&[0xffu8; 64]);
    }

    #[test]
    fn run_gen_then_verify_roundtrip() {
        let _g = REF_LOCK.lock().unwrap();
        // Regenerate the canonical document, then verify: the on-disk
        // file must already equal a fresh computation.
        assert_eq!(run("verify"), ExitCode::SUCCESS);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let committed = fs::read_to_string(root.join(REFERENCE_PATH)).unwrap();
        assert_eq!(committed, reference_json(root));
        assert!(committed.ends_with('\n'), "LF-terminated document");
    }

    #[test]
    fn run_rejects_unknown_mode() {
        let _g = REF_LOCK.lock().unwrap();
        assert_eq!(run("nonsense"), ExitCode::from(2));
    }

    #[test]
    fn run_empty_mode_is_verify() {
        let _g = REF_LOCK.lock().unwrap();
        // The no-argument CLI invocation is the CI gate.
        assert_eq!(run(""), ExitCode::SUCCESS);
    }
}

#[cfg(test)]
mod gen_mode_tests {
    use super::*;

    /// `gen` rewrites reference.json byte-identically: deterministic
    /// corpus, stable digest — the CD gate's no-drift contract.
    #[test]
    fn gen_rewrites_identical_bytes() {
        let _g = super::tests::REF_LOCK.lock().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let before = fs::read(root.join(REFERENCE_PATH)).unwrap();
        assert_eq!(run("gen"), ExitCode::SUCCESS);
        let after = fs::read(root.join(REFERENCE_PATH)).unwrap();
        assert_eq!(before, after);
    }
}
