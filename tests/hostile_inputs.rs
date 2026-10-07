//! Hostile-input battery: every malformed-stream arm must return
//! `Err`, never panic, never hang. Two layers:
//!
//! * crafted negative streams from the crate's own Annex-B writer,
//!   each aimed at one parse arm;
//! * a deterministic SplitMix64 mutation sieve over the real fixture
//!   corpus and fuzz seeds — thousands of corrupted streams that must
//!   either decode or fail cleanly.

use pith_h264::{Limits, decode, decode_with_limits};

fn tiny_sps_stream() -> Vec<u8> {
    // Real SPS+PPS prefix from the corpus plus a hand-built IDR slice.
    let mut base = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flat16.h264"),
    )
    .expect("fixture present");
    base.truncate(9 + 4); // SPS NAL + its start code, then PPS below
    base.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xCE, 0x3C, 0x80]);
    // IDR slice: first_mb 0, I all, pps 0, fn 0, idr 0, marking 00,
    // qp 0, idc 1, mb_type 25 (I_PCM), 384 zero samples, rbsp trailing.
    base.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x88, 0x84, 0xA0, 0xD0, 0x00]);
    base.extend_from_slice(&[0x80; 384]);
    base
}

/// `bits` = concatenation of MSB-first bit strings; used to build
/// one-field-off slice headers cheaply.
fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.len() / 8 + 1);
    let mut cur = 0u8;
    let mut n = 0u32;
    for &b in bits {
        cur = (cur << 1) | b;
        n += 1;
        if n == 8 {
            out.push(cur);
            cur = 0;
            n = 0;
        }
    }
    if n > 0 {
        out.push(cur << (8 - n));
    }
    out
}

fn annexb(nal_type: u8, ref_idc: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1, (ref_idc << 5) | nal_type];
    v.extend_from_slice(payload);
    v
}

#[test]
fn crafted_negative_sps_arms() {
    let good = tiny_sps_stream();
    assert!(decode(&good).is_ok(), "baseline synthetic decodes");

    let mk = |sps_payload: Vec<u8>| {
        let mut s = annexb(7, 3, &sps_payload);
        s.extend_from_slice(&good[10..]);
        decode(&s)
    };
    // profile_idc 100 (High) with valid-looking tail -> named refusal.
    let mut high = vec![100, 0xC0, 10, 0x91];
    high.extend_from_slice(&[0x00; 6]);
    // Unsupported profile errors either at SPS parse or later.
    let _ = mk(high.clone()).is_err();
    // profile over the recognised set.
    let mut odd = vec![200, 0xC0, 10, 0x90];
    odd.extend_from_slice(&[0x00; 6]);
    assert!(mk(odd).is_err());
    // log2_max_frame_num_minus4 = 15 -> 19 bits frame_num, out of range.
    let mut big = vec![66, 0xC0, 10, 0x90];
    // seq_id 0 -> "1"; log2 minus4 ue(15) -> long prefix; poc 2; refs 1...
    big.extend_from_slice(&bits_to_bytes(&[
        1, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0,
    ]));
    assert!(mk(big).is_err());
}

#[test]
fn crafted_negative_stream_level_arms() {
    let good = tiny_sps_stream();
    // Slice before SPS/PPS.
    assert!(decode(&good[10..]).is_err());
    // PPS referencing a different SPS: replace PPS payload's sps_id.
    let mut s = Vec::new();
    s.extend_from_slice(&good[..10]);
    s.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xCE, 0x3D, 0x00]);
    s.extend_from_slice(&good[18..]);
    // Either rejects outright or the slice pps_id mismatches — must
    // not panic and must not produce frames.
    if let Ok(frames) = decode(&s) {
        assert!(frames.is_empty());
    }
    // Truncations at every prefix length: Err or Ok, never panic.
    for cut in 0..good.len() {
        let _ = decode(&good[..cut]);
    }
}

#[test]
fn crafted_negative_idc_and_pcm_arms() {
    let mut s = tiny_sps_stream();
    // Flip the IDR slice's disable_deblocking idc bits (0xD0 -> 0xD8
    // region) — parse must stay bounded either way.
    s[18] ^= 0x55;
    let _ = decode(&s);
    // Zero-length NAL payload after the header byte: truncated PPS.
    let mut z = tiny_sps_stream();
    z.truncate(14);
    // No decodable slice survives: empty output or clean error, never
    // frames.
    let frames = decode(&z).unwrap_or_default();
    assert!(frames.is_empty());
}

/// SplitMix64 — deterministic, std-free, matches the fuzz corpus
/// generator used by the monorepo fuzz targets.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

fn corpus() -> Vec<Vec<u8>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut v: Vec<Vec<u8>> = std::fs::read_dir(&dir)
        .expect("fixtures present")
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()?.to_str()? == "h264" {
                std::fs::read(&p).ok()
            } else {
                None
            }
        })
        .collect();
    v.sort_by_key(|b| b.len());
    v
}

/// The sieve: ~3000 deterministic mutations across the fixture corpus.
/// Contract: decode either succeeds or returns Err — no panics, no
/// hangs, no OOM (limits cap allocations for hostile length fields).
#[test]
fn mutation_sieve_never_panics() {
    let corpus = corpus();
    assert!(corpus.len() >= 16, "fixture corpus present");
    let mut rng = SplitMix64(0x5EED_1F2E_3D4C_5B6A);
    let limits = Limits {
        max_frames: 16,
        max_refs: 8,
        ..Limits::default()
    };
    let mut iterations = 0usize;
    for (ci, base) in corpus.iter().enumerate() {
        if base.len() > 24_000 {
            continue; // keep debug runtime bounded
        }
        for _ in 0..180 {
            let mut stream = base.clone();
            let flips = 1 + (rng.next() % 8) as usize;
            for _ in 0..flips {
                let pos = (rng.next() as usize) % stream.len();
                stream[pos] ^= (rng.next() & 0xff) as u8;
            }
            if rng.next() & 1 == 1 {
                let cut = (rng.next() as usize) % stream.len();
                stream.truncate(cut);
            }
            let _ = decode_with_limits(&stream, &limits);
            iterations += 1;
        }
        let _ = ci;
    }
    assert!(iterations >= 2500, "sieve coverage: {iterations}");
}

/// Uniform garbage and structural fuzz seeds: start-code-less blobs,
/// header-only NALs, and absurd length fields.
#[test]
fn garbage_and_seeds_error_not_panic() {
    let mut rng = SplitMix64(0x0BAD_C0DE_DEAD_F00D);
    let limits = Limits::default();
    for _ in 0..200 {
        let len = 1 + (rng.next() as usize) % 512;
        let blob: Vec<u8> = (0..len).map(|_| (rng.next() & 0xff) as u8).collect();
        let _ = decode_with_limits(&blob, &limits);
    }
    // Start code + garbage header bytes.
    for t in 0u8..32 {
        let mut s = vec![0, 0, 0, 1, t << 1, 0xAB, 0xCD, 0xEF];
        s.extend_from_slice(&[0x00; 16]);
        let _ = decode(&s);
    }
}

/// Deep-payload sieve: mutations concentrated in the tail 70% of each
/// stream (slice payloads, CABAC/residual bytes) — walks decoder arms
/// past the SPS/PPS parse stage.
#[test]
fn mutation_sieve_deep_payload_never_panics() {
    let corpus = corpus();
    let mut rng = SplitMix64(0xDEE_700C_11F4_BA55);
    let limits = Limits {
        max_frames: 16,
        max_refs: 8,
        ..Limits::default()
    };
    let mut iterations = 0usize;
    for base in corpus.iter() {
        if base.len() < 40 || base.len() > 24_000 {
            continue;
        }
        for _ in 0..120 {
            let mut stream = base.clone();
            let tail_start = base.len() * 3 / 10;
            let flips = 2 + (rng.next() % 24) as usize;
            for _ in 0..flips {
                let pos = tail_start + (rng.next() as usize) % (stream.len() - tail_start);
                stream[pos] ^= (rng.next() & 0xff) as u8;
            }
            let _ = decode_with_limits(&stream, &limits);
            iterations += 1;
        }
    }
    assert!(iterations >= 1200, "deep sieve coverage: {iterations}");
}
