//! The canonical decode-output serialization behind `reference.json`,
//! re-expressed as library code so the vector generator and the C ABI
//! surface (`ffi`) share one implementation.
//!
//! Every vector in `reference.json` is computed through the crate's
//! public decode API from a committed conformance fixture
//! (`tests/fixtures/*.h264`): the decoded pictures' `y || cb || cr`
//! planes are concatenated in presentation order and SHA-256 hashed by
//! `tools/gen-reference`. No RNG, no time, no platform-dependent bytes
//! (the decoder is integer-only), so the output is byte-stable
//! everywhere.
//!
//! Canonical decode-output stream (the byte stream
//! [`crate::ffi::pith_h264_decode_planes`] hands to the language
//! SDKs; 12-byte header, big-endian):
//!
//! 1. `frames` as `u32` big-endian — the number of decoded pictures;
//! 2. `width` as `u32` big-endian;
//! 3. `height` as `u32` big-endian;
//! 4. the decoded planes, `y || cb || cr` concatenated over all
//!    frames in presentation order.
//!
//! Unlike `pith-png` (where the vector digest covers the whole
//! canonical stream), the `planes_sha256` a vector pins covers the
//! **planes tail only** (`canonical[12..]`), not the header — that is
//! the digest contract `reference.json` records, and the SDKs test
//! `sha256(raw[12..]) == planes_sha256` plus the header fields
//! separately.

use crate::Frame;

/// Concatenates every frame's planes, `y || cb || cr`, in presentation
/// order — exactly the bytes a `reference.json` vector's
/// `planes_sha256` digest covers.
#[must_use]
pub fn planes(frames: &[Frame]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    for f in frames {
        out.extend_from_slice(&f.y);
        out.extend_from_slice(&f.cb);
        out.extend_from_slice(&f.cr);
    }
    out
}

/// Decodes `stream` with the default [`crate::Limits`] and serializes
/// it into the canonical byte stream described in the module docs.
/// Every decode refusal — malformed Annex-B, unsupported feature,
/// truncated NAL — is `Err(())`: the caller only needs to know the
/// input is rejected, not why.
#[allow(clippy::result_unit_err)] // the unit error is the contract: rejection, unnamed
pub fn canonical_bytes(stream: &[u8]) -> Result<alloc::vec::Vec<u8>, ()> {
    let frames = crate::decode(stream).map_err(|_| ())?;
    let first = frames.first().ok_or(())?;
    // `decode` caps the picture count at `Limits::max_frames` (1024),
    // so the `u32` frame count cannot truncate.
    let mut out = alloc::vec::Vec::with_capacity(12 + planes(&frames).len());
    out.extend_from_slice(&(frames.len() as u32).to_be_bytes());
    out.extend_from_slice(&first.width.to_be_bytes());
    out.extend_from_slice(&first.height.to_be_bytes());
    out.extend_from_slice(&planes(&frames));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{canonical_bytes, planes};
    use crate::Frame;

    /// A synthetic frame with known plane sizes: `w*h` luma samples,
    /// `(w/2)*(h/2)` chroma samples each.
    fn frame(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            y: vec![1u8; (width * height) as usize],
            cb: vec![2u8; (width / 2 * height / 2) as usize],
            cr: vec![3u8; (width / 2 * height / 2) as usize],
        }
    }

    /// `planes` length math: `w*h + 2*(w/2)*(h/2)` bytes per frame,
    /// frame order preserved, plane order `y || cb || cr`.
    #[test]
    fn planes_length_and_order() {
        let frames = [frame(16, 16), frame(64, 48)];
        let out = planes(&frames);
        // w*h + 2*(w/2)*(h/2) bytes per frame.
        assert_eq!(out.len(), (16 * 16 + 2 * 8 * 8) + (64 * 48 + 2 * 32 * 24));
        let first_len = 16 * 16 + 2 * 8 * 8;
        // Frame 0: y, then cb, then cr.
        assert_eq!(&out[..16 * 16], &frames[0].y[..]);
        assert_eq!(out[16 * 16], 2, "cb follows y");
        assert_eq!(out[16 * 16 + 8 * 8], 3, "cr follows cb");
        assert_eq!(&out[16 * 16..16 * 16 + 8 * 8], &frames[0].cb[..]);
        assert_eq!(&out[16 * 16 + 8 * 8..first_len], &frames[0].cr[..]);
        // Frame 1's planes follow in the same order.
        let rest = &out[first_len..];
        assert_eq!(&rest[..64 * 48], &frames[1].y[..]);
        assert_eq!(rest[64 * 48], 2);
        assert_eq!(rest[64 * 48 + 32 * 24], 3);
    }

    /// Empty input produces no planes.
    #[test]
    fn planes_of_no_frames_is_empty() {
        assert!(planes(&[]).is_empty());
    }

    /// The canonical stream of a committed fixture is exactly the
    /// 12-byte header followed by the concatenated planes, and the
    /// header fields match the vector the generator recorded.
    #[test]
    fn canonical_bytes_of_fixture_is_header_plus_planes() {
        let path = format!("{}/tests/fixtures/flat16.h264", env!("CARGO_MANIFEST_DIR"));
        let stream = std::fs::read(&path).expect("fixture");
        let frames = crate::decode(&stream).expect("fixture must decode");
        let tail = planes(&frames);

        let canonical = canonical_bytes(&stream).expect("fixture must decode");
        assert_eq!(&canonical[..12], &[0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16]);
        assert_eq!(&canonical[12..], &tail[..]);

        // The digest contract: planes_sha256 covers the tail only.
        use pith_digest::sha256;
        let hashed = sha256(&tail).expect("sha256 of planes");
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(hashed.as_bytes()),
            "524961aab71df0b48291ed8507c78051cde1dad5ec6e305fdd083e7667726519"
        );
    }

    /// Garbage input is refused, not guessed at.
    #[test]
    fn canonical_bytes_rejects_garbage() {
        assert_eq!(canonical_bytes(&[0u8; 16]), Err(()));
        assert_eq!(canonical_bytes(&[]), Err(()));
    }
}
