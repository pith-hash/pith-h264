//! End-to-end exercise of the C ABI surface (`pith_h264::ffi`) from a
//! separate test binary: the language SDKs bind these exports through
//! a foreign-caller boundary, so the happy path, the free and every
//! refusal run here the way a ctypes/koffi/cgo caller would drive
//! them. Mirrors the in-crate `ffi` unit tests; kept additive so the
//! workspace coverage gate sees the exports executed from a consumer
//! binary as well as the library's own harness.

use pith_h264::ffi::{
    PITH_E_INVALID, PITH_E_REJECTED, PITH_OK, pith_h264_decode_planes, pith_h264_free,
};

/// Decodes `data` through the raw C ABI and returns (status, buffer,
/// length). The buffer, when status is `PITH_OK`, must be released
/// with `pith_h264_free` by the caller.
fn raw_decode(data: &[u8]) -> (i32, *mut u8, usize) {
    let mut out: *mut u8 = core::ptr::null_mut();
    let mut out_len: usize = 0;
    let status =
        unsafe { pith_h264_decode_planes(data.as_ptr(), data.len(), &mut out, &mut out_len) };
    (status, out, out_len)
}

/// The FFI happy path on a committed conformance fixture: status OK,
/// the exact canonical length, the recorded 12-byte header, and byte
/// equality with the safe core's serialization.
#[test]
fn ffi_decode_planes_reproduces_the_canonical_stream() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/flat16.h264");
    let stream = std::fs::read(path).expect("fixture");

    let frames = pith_h264::decode(&stream).expect("fixture must decode");
    let mut expected = Vec::new();
    expected.extend_from_slice(&1u32.to_be_bytes());
    expected.extend_from_slice(&16u32.to_be_bytes());
    expected.extend_from_slice(&16u32.to_be_bytes());
    expected.extend_from_slice(&pith_h264::reference::planes(&frames));

    let (status, out, out_len) = raw_decode(&stream);
    assert_eq!(status, PITH_OK);
    // 1 frame of 16x16: 12-byte header + w*h luma + 2*(w/2)*(h/2) chroma.
    assert_eq!(out_len, 12 + 16 * 16 + 2 * 8 * 8);
    let canonical = unsafe { core::slice::from_raw_parts(out, out_len) };
    // The 12-byte header is the recorded flat16 geometry, big-endian:
    // 1 frame, 16 wide, 16 high.
    assert_eq!(&canonical[..12], &[0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 16]);
    assert_eq!(canonical, &expected[..]);
    unsafe { pith_h264_free(out, out_len) };
}

/// Null pointers are `PITH_E_INVALID`; non-null garbage is the
/// decoder's `PITH_E_REJECTED`; a null buffer is a legal free.
#[test]
fn ffi_refusals_never_panic() {
    let mut out: *mut u8 = core::ptr::null_mut();
    let mut out_len: usize = 0;
    let status = unsafe { pith_h264_decode_planes(core::ptr::null(), 0, &mut out, &mut out_len) };
    assert_eq!(status, PITH_E_INVALID);

    let garbage = [0u8; 16];
    let status = unsafe {
        pith_h264_decode_planes(
            garbage.as_ptr(),
            garbage.len(),
            core::ptr::null_mut(),
            &mut out_len,
        )
    };
    assert_eq!(status, PITH_E_INVALID);
    let status = unsafe {
        pith_h264_decode_planes(
            garbage.as_ptr(),
            garbage.len(),
            &mut out,
            core::ptr::null_mut(),
        )
    };
    assert_eq!(status, PITH_E_INVALID);

    let (status, out, out_len) = raw_decode(&garbage);
    assert_eq!(status, PITH_E_REJECTED);
    assert!(out.is_null() && out_len == 0);

    unsafe { pith_h264_free(core::ptr::null_mut(), 0) };
}

/// The three error inputs `reference.json` records — every decode()
/// failure kind the cross-SDK contract pins — come back as
/// `PITH_E_REJECTED` through the C ABI.
#[test]
fn ffi_rejects_the_recorded_error_inputs() {
    let inputs: [&[u8]; 3] = [
        &[],                                         // empty-input: InvalidMagic
        &[0x67, 0x6e, 0x00, 0x0a, 0xf8, 0x88, 0x80], // high-profile-sps: Unsupported
        &[0x00, 0x00, 0x01, 0x67],                   // truncated-sps-nal: Truncated
    ];
    for input in inputs {
        let (status, out, out_len) = raw_decode(input);
        assert_eq!(status, PITH_E_REJECTED);
        assert!(out.is_null() && out_len == 0);
    }
}
