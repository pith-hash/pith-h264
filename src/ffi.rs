//! The C ABI surface of `pith-h264`: the entry points the Python
//! (ctypes), Node (koffi) and Go (cgo) SDKs bind through.
//!
//! The suite's FFI convention, defined by this module and mirrored by
//! every `pith-*` cdylib:
//!
//! * one flat set of `#[unsafe(no_mangle)] pub unsafe extern "C"`
//!   functions — raw pointers plus lengths, no structs across the
//!   boundary;
//! * every function returns a status code (see the constants below),
//!   never a `Result`, never a panic: a `panic = "abort"` cdylib must
//!   not be reachable from a foreign caller;
//! * an operation either hands ownership to the caller (and ships a
//!   matching `_free` — [`pith_h264_free`] here) or writes into
//!   caller-provided out-parameters;
//! * the `unsafe` allowance is confined to this module; every core
//!   module stays unsafe-free behind the crate-root `#![deny]`.
//!
//! The wire format [`pith_h264_decode_planes`] hands out is the
//! canonical decode-output stream of [`crate::reference`] (12-byte
//! big-endian header: frames, width, height; then the planes tail) —
//! and the `planes_sha256` a `reference.json` vector pins covers the
//! **planes tail only** (`canonical[12..]`), never the header. That is
//! a deliberate difference from `pith-png`, whose vector digest covers
//! the whole stream: the h264 digest contract predates the header and
//! lives in `reference.json`.
//!
//! Decoding uses the crate's conservative default [`Limits`] (64 MiB
//! of input, 8192x4096 pictures, 1024 frames) — a hashing pipeline
//! never wants an unbounded decode, and the FFI surface is no
//! exception.

#![allow(unsafe_code)]

use crate::reference::canonical_bytes;

/// Status: success.
pub const PITH_OK: i32 = 0;
/// Status: a caller argument is invalid — a null pointer.
pub const PITH_E_INVALID: i32 = -1;
/// Status: the core decoder refused the input (malformed Annex-B
/// stream: bad magic, unsupported profile or feature, truncated NAL).
pub const PITH_E_REJECTED: i32 = -2;

/// Decodes an Annex-B H.264 byte stream into the canonical byte stream
/// the `reference.json` vectors are defined over.
///
/// `data` points at `len` bytes of the complete stream. On success the
/// function allocates a buffer, writes its address through `out`, its
/// length through `out_len`, and returns [`PITH_OK`]; the caller owns
/// the buffer and must release it with [`pith_h264_free`], passing
/// back the same pointer *and* length. The buffer layout is the
/// canonical decode-output serialization of [`crate::reference`]:
/// `frames`/`width`/`height` `u32` big-endian, then the planes tail
/// (`y || cb || cr` over all frames, presentation order) — the bytes
/// `planes_sha256` covers (see the module docs for the tail-only
/// digest rule).
///
/// # Safety
///
/// `data` must point to `len` readable bytes; `out` to one writable
/// pointer; `out_len` to one writable `usize`. All must stay valid for
/// the duration of the call; the function retains nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pith_h264_decode_planes(
    data: *const u8,
    len: usize,
    out: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if data.is_null() || out.is_null() || out_len.is_null() {
        return PITH_E_INVALID;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data, len) };
    match decode_and_serialize(bytes) {
        Ok(canonical) => {
            let len = canonical.len();
            // Hand the exact-length buffer to the caller; `pith_h264_free`
            // reconstructs the boxed slice from the same length.
            let ptr = alloc::boxed::Box::into_raw(canonical.into_boxed_slice());
            unsafe {
                *out = ptr.cast::<u8>();
                *out_len = len;
            }
            PITH_OK
        }
        Err(status) => status,
    }
}

/// Releases a buffer handed out by [`pith_h264_decode_planes`].
///
/// # Safety
///
/// `ptr` must be a pointer returned by [`pith_h264_decode_planes`]
/// with the `out_len` value that came back with it, and must not have
/// been released (or otherwise freed) before. Null is accepted and
/// ignored, so callers can free unconditionally on the error path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pith_h264_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    drop(unsafe { alloc::boxed::Box::from_raw(slice) });
}

/// The safe core of [`pith_h264_decode_planes`]: decode, then
/// serialize canonically. Decoding failures map to [`PITH_E_REJECTED`].
fn decode_and_serialize(bytes: &[u8]) -> Result<alloc::vec::Vec<u8>, i32> {
    canonical_bytes(bytes).map_err(|_| PITH_E_REJECTED)
}

#[cfg(test)]
mod tests {
    use super::{
        PITH_E_INVALID, PITH_E_REJECTED, PITH_OK, decode_and_serialize, pith_h264_decode_planes,
        pith_h264_free,
    };

    /// A committed conformance fixture, decoded end-to-end through the
    /// raw FFI: status OK, the length matches the serialization, and
    /// the buffer round-trips through `pith_h264_free`.
    #[test]
    fn ffi_decode_reproduces_the_canonical_stream() {
        let path = format!("{}/tests/fixtures/flat16.h264", env!("CARGO_MANIFEST_DIR"));
        let stream = std::fs::read(&path).expect("fixture");
        let expected = decode_and_serialize(&stream).expect("decode");

        let mut out: *mut u8 = core::ptr::null_mut();
        let mut out_len: usize = 0;
        let status = unsafe {
            pith_h264_decode_planes(stream.as_ptr(), stream.len(), &mut out, &mut out_len)
        };
        assert_eq!(status, PITH_OK);
        assert_eq!(out_len, expected.len());
        let handed_back = unsafe { core::slice::from_raw_parts(out, out_len) };
        assert_eq!(handed_back, expected.as_slice());
        // The first 12 bytes are the documented header: 1 frame, 16x16,
        // big-endian — the flat16 vector's recorded geometry.
        assert_eq!(&handed_back[..12], &[0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16]);
        unsafe { pith_h264_free(out, out_len) };
    }

    /// Null pointers are [`PITH_E_INVALID`]; garbage input is
    /// [`PITH_E_REJECTED`]; a null buffer is a legal free.
    #[test]
    fn ffi_refusals() {
        let mut out: *mut u8 = core::ptr::null_mut();
        let mut out_len: usize = 0;
        let null_data =
            unsafe { pith_h264_decode_planes(core::ptr::null(), 0, &mut out, &mut out_len) };
        assert_eq!(null_data, PITH_E_INVALID);

        let stream = [0u8; 16];
        let null_out = unsafe {
            pith_h264_decode_planes(
                stream.as_ptr(),
                stream.len(),
                core::ptr::null_mut(),
                &mut out_len,
            )
        };
        assert_eq!(null_out, PITH_E_INVALID);

        let null_out_len = unsafe {
            pith_h264_decode_planes(
                stream.as_ptr(),
                stream.len(),
                &mut out,
                core::ptr::null_mut(),
            )
        };
        assert_eq!(null_out_len, PITH_E_INVALID);

        let garbage = unsafe {
            pith_h264_decode_planes(stream.as_ptr(), stream.len(), &mut out, &mut out_len)
        };
        assert_eq!(garbage, PITH_E_REJECTED);

        unsafe { pith_h264_free(core::ptr::null_mut(), 0) };
    }

    /// The three recorded error inputs — every `decode()` failure kind
    /// the cross-SDK contract pins — come back as [`PITH_E_REJECTED`].
    #[test]
    fn ffi_rejects_the_recorded_error_inputs() {
        let inputs: [&[u8]; 3] = [
            &[],                                         // empty-input: InvalidMagic
            &[0x67, 0x6e, 0x00, 0x0a, 0xf8, 0x88, 0x80], // high-profile-sps: Unsupported
            &[0x00, 0x00, 0x01, 0x67],                   // truncated-sps-nal: Truncated
        ];
        for input in inputs {
            assert_eq!(decode_and_serialize(input), Err(PITH_E_REJECTED));
            let mut out: *mut u8 = core::ptr::null_mut();
            let mut out_len: usize = 0;
            let status = unsafe {
                pith_h264_decode_planes(input.as_ptr(), input.len(), &mut out, &mut out_len)
            };
            assert_eq!(status, PITH_E_REJECTED);
        }
    }

    /// The safe core rejects malformed input instead of panicking.
    #[test]
    fn safe_core_rejects_garbage() {
        assert_eq!(
            decode_and_serialize(&[0x00, 0x00, 0x00, 0x01, 0x67, 0xff, 0xff]),
            Err(PITH_E_REJECTED)
        );
    }
}
