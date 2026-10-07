//! MMCO op 3 (spec 8.2.5.4.3, short-term → long-term marking): a
//! hand-built, spec-exact Annex-B stream whose reference marking
//! exercises the long-term path end-to-end — the same construction
//! style the FLAC lane uses for its synthetic vectors, because no
//! offline encoder emits MMCO 3.
//!
//! The stream (16x16, `log2_max_frame_num` 4, `max_num_ref_frames` 2,
//! poc type 2, CAVLC, deblocking disabled, one MB per picture):
//!
//! | fn | slice | content | marking after decode | DPB after |
//! |----|-------|---------|----------------------|-----------|
//! | 0 | IDR | I_PCM `A` | — | F0 ST |
//! | 1 | P | I_PCM `C` | mmco 3: F0 → LT idx 0 | F0 LT0, F1 ST |
//! | 2 | P | I_PCM `B` | — (sliding window: ST 2 ≤ 2) | F0 LT0, F1, F2 |
//! | 3 | P | I_PCM `B` | — (window: ST 3 > 2 evicts F1) | F0 LT0, F2, F3 |
//! | 4 | P | P_Skip (copies F3 = `B`) | mmco 3 (ltpn 1, idx 1): F2 → LT1 | F0 LT0, F2 LT1, F3, F4 |
//! | 5 | P | P_L0_16x16 `ref_idx` 3 of 4 → F2 = `B` | mmco 1: F4 unused | F0 LT0, F2 LT1, F3, F5 |
//! | 6 | P | P_Skip (copies F5 = `B`) | — (window evicts F3) | F0 LT0, F2 LT1, F5, F6 |
//!
//! Assertions pin the whole op-3 contract:
//!
//! * frame 4's marking reads the two-field payload (`long_term_pic_num`
//!   1, `long_term_frame_idx` 1) — a parser that skips the payload
//!   desyncs and cannot reproduce the frame sequence;
//! * frame 5 references a long-term picture through an explicit
//!   `ref_idx` — impossible unless F2 was marked long-term *and*
//!   appended to RefPicList0 after every short-term entry;
//! * the sliding window (frames 2/3/6) evicts only short-term
//!   pictures while two long-term pictures persist in the buffer;
//! * every P_Skip output is the pixel-exact copy of the picture it
//!   must reference (`A`/`B`/`C` never cross).

use pith_h264::{Frame, decode};

/// MSB-first bit writer with ue/se coders and RBSP trailing bits.
struct Bw {
    bytes: Vec<u8>,
    bit: u32,
}

impl Bw {
    fn new() -> Bw {
        Bw {
            bytes: Vec::new(),
            bit: 0,
        }
    }
    fn bit(&mut self, b: bool) {
        if self.bit == 0 {
            self.bytes.push(0);
        }
        if b {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> self.bit;
        }
        self.bit = (self.bit + 1) % 8;
    }
    fn bits(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 != 0);
        }
    }
    fn ue(&mut self, mut v: u32) {
        v += 1;
        let n = 31 - v.leading_zeros();
        for _ in 0..n {
            self.bit(false);
        }
        self.bits(v, n + 1);
    }
    fn se(&mut self, v: i32) {
        self.ue(if v <= 0 {
            (-v * 2) as u32
        } else {
            (v * 2 - 1) as u32
        });
    }
    fn u8_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.bits(b as u32, 8);
        }
    }
    fn trailing(&mut self) {
        self.bit(true);
        while self.bit != 0 {
            self.bit(false);
        }
    }
    fn pcm_align(&mut self) {
        while self.bit != 0 {
            self.bit(false);
        }
    }
}

/// RBSP → EBSP emulation-prevention byte insertion (spec 7.3.1).
fn ebsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 8);
    let mut zeros = 0usize;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn nal(ref_idc: u8, typ: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, (ref_idc << 5) | typ];
    out.extend(ebsp(rbsp));
    out
}

fn sps() -> Vec<u8> {
    let mut b = Bw::new();
    b.bits(66, 8); // profile_idc baseline
    b.bits(0xC0, 8); // constraint_set0/1
    b.bits(10, 8); // level_idc 1.0
    b.ue(0); // seq_parameter_set_id
    b.ue(0); // log2_max_frame_num_minus4 -> frame_num u(4)
    b.ue(2); // pic_order_cnt_type 2
    b.ue(2); // max_num_ref_frames
    b.bit(false); // gaps_in_frame_num_value_allowed
    b.ue(0); // pic_width_in_mbs_minus1 -> 16
    b.ue(0); // pic_height_in_map_units_minus1 -> 16
    b.bit(true); // frame_mbs_only
    b.bit(true); // direct_8x8_inference
    b.bit(false); // frame_cropping
    b.bit(false); // vui
    b.trailing();
    nal(3, 7, &b.bytes)
}

fn pps() -> Vec<u8> {
    let mut b = Bw::new();
    b.ue(0); // pps id
    b.ue(0); // sps id
    b.bit(false); // entropy_coding_mode (CAVLC)
    b.bit(false); // bottom_field_pic_order
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1 -> 1
    b.ue(0); // num_ref_idx_l1_default_active_minus1 -> 1
    b.bit(false); // weighted_pred
    b.bits(0, 2); // weighted_bipred_idc
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.bit(true); // deblocking_filter_control_present
    b.bit(false); // constrained_intra_pred
    b.bit(false); // redundant_pic_cnt_present
    b.trailing();
    nal(3, 8, &b.bytes)
}

/// 384 raw I_PCM sample bytes: luma `y` then (cb, cr) chroma.
fn pcm(y: u8, cb: u8, cr: u8) -> Vec<u8> {
    let mut v = vec![y; 256];
    v.extend(std::iter::repeat_n(cb, 64));
    v.extend(std::iter::repeat_n(cr, 64));
    v
}

/// One slice NAL. `data` writes the slice-data fields into the same
/// bit writer, so `pcm_align` sees the true stream position.
fn slice(
    idr: bool,
    frame_num: u32,
    adaptive: Option<&[(u8, u32, u32)]>, // (mmco op, a, b)
    override_refs: Option<u32>,          // num_ref_idx_l0_active_minus1
    data: impl FnOnce(&mut Bw),
) -> Vec<u8> {
    let mut b = Bw::new();
    b.ue(0); // first_mb_in_slice
    b.ue(if idr { 7 } else { 5 }); // slice_type: I all / P all
    b.ue(0); // pps id
    b.bits(frame_num, 4); // frame_num (log2_max_frame_num 4)
    if idr {
        b.ue(0); // idr_pic_id
    }
    if !idr {
        match override_refs {
            Some(minus1) => {
                b.bit(true);
                b.ue(minus1);
            }
            None => b.bit(false),
        }
        b.bit(false); // ref_pic_list_modification_flag_l0 = 0
    }
    // dec_ref_pic_marking (all slices carry nal_ref_idc != 0).
    if idr {
        b.bit(false); // no_output_of_prior_pics
        b.bit(false); // long_term_reference_flag (unsupported -> must stay 0)
    } else {
        match adaptive {
            None => b.bit(false),
            Some(cmds) => {
                b.bit(true);
                for &(op, a, bv) in cmds {
                    b.ue(op as u32);
                    match op {
                        1 => b.ue(a), // difference_of_pic_nums_minus1
                        3 => {
                            b.ue(a); // long_term_pic_num
                            b.ue(bv); // long_term_frame_idx
                        }
                        _ => panic!("test builder supports mmco 1/3 only"),
                    }
                }
                b.ue(0); // terminator
            }
        }
    }
    b.se(0); // slice_qp_delta
    b.ue(1); // disable_deblocking_filter_idc = 1 (no filtering)
    data(&mut b);
    b.trailing();
    nal(if idr { 3 } else { 2 }, if idr { 5 } else { 1 }, &b.bytes)
}

fn idr_pcm_slice(y: u8, cb: u8, cr: u8) -> Vec<u8> {
    slice(true, 0, None, None, |d| {
        d.ue(25); // mb_type I_PCM
        d.pcm_align();
        d.u8_bytes(&pcm(y, cb, cr));
    })
}

/// P slice whose single MB is I_PCM (P-slice mb_type 30).
fn p_pcm_slice(
    frame_num: u8,
    y: u8,
    cb: u8,
    cr: u8,
    marking: Option<&[(u8, u32, u32)]>,
) -> Vec<u8> {
    slice(false, frame_num as u32, marking, None, |d| {
        d.ue(0); // mb_skip_run 0
        d.ue(30); // mb_type 30 -> I_PCM
        d.pcm_align();
        d.u8_bytes(&pcm(y, cb, cr));
    })
}

/// P slice whose single MB is P_Skip.
fn p_skip_slice(frame_num: u8, marking: Option<&[(u8, u32, u32)]>) -> Vec<u8> {
    slice(false, frame_num as u32, marking, None, |d| {
        d.ue(1); // mb_skip_run 1 -> the single MB is skipped
    })
}

/// P slice: one P_L0_16x16 MB predicting from `ref_idx` with zero MVD
/// and no residual (`num_ref_idx_l0_active` = cap).
fn p_inter_slice(frame_num: u8, cap_minus1: u32, ref_idx: u32) -> Vec<u8> {
    slice(false, frame_num as u32, None, Some(cap_minus1), |d| {
        d.ue(0); // mb_skip_run 0
        d.ue(0); // mb_type P_L0_16x16
        d.ue(ref_idx); // ref_idx_l0 (te == ue for active count > 1)
        d.se(0);
        d.se(0); // mvd_l0 (0, 0)
        d.ue(0); // coded_block_pattern 0 (inter mapping)
    })
}

fn plane(frame: &Frame) -> Vec<u8> {
    let mut v = frame.y.clone();
    v.extend_from_slice(&frame.cb);
    v.extend_from_slice(&frame.cr);
    v
}

#[test]
fn mmco3_marks_short_term_long_term() {
    // A = 0x80/0x80, C = 0xC0/0x40, B = zero luma + 0x01 chroma (the
    // zero run also exercises emulation-prevention insertion).
    let stream = [
        sps(),
        pps(),
        idr_pcm_slice(0x80, 0x80, 0x80), // f0 = A
        p_pcm_slice(1, 0xC0, 0x40, 0x40, Some(&[(3, 0, 0)])), // f1 = C, F0 -> LT0
        p_pcm_slice(2, 0x00, 0x01, 0x01, None), // f2 = B, window keeps all
        p_pcm_slice(3, 0x00, 0x01, 0x01, None), // f3 = B, window evicts F1
        p_skip_slice(4, Some(&[(3, 1, 1)])), // f4 copies F3 = B, F2 -> LT1
        p_inter_slice(5, 3, 3),          // f5 predicts F2 (LT1) = B, mmco 1 kills F4
        p_skip_slice(6, None),           // f6 copies F5 = B, window evicts F3
    ]
    .concat();

    let frames = decode(&stream).expect("synthetic mmco-3 stream must decode");
    assert_eq!(frames.len(), 7, "frame count");
    for f in &frames {
        assert_eq!(f.width, 16);
        assert_eq!(f.height, 16);
    }
    let a = plane(&frames[0]);
    let b = plane(&frames[2]);
    let c = plane(&frames[1]);
    // Distinct content per pattern: A (flat grey), C (light grey),
    // B (black luma + near-black chroma).
    assert!(a != b && b != c && a != c, "patterns must be distinct");
    // I_PCM frames carry their raw samples verbatim.
    assert_eq!(a, pcm(0x80, 0x80, 0x80), "f0 == A");
    assert_eq!(c, pcm(0xC0, 0x40, 0x40), "f1 == C");
    assert_eq!(b, pcm(0x00, 0x01, 0x01), "f2 == B");
    // f3 = I_PCM B again.
    assert_eq!(plane(&frames[3]), b, "f3 == B");
    // f4: P_Skip must reference F3 (short-term, newest) -> B. If the
    // mmco-3 payload of f4 were skipped, every later element desyncs
    // and the decode errors out long before here.
    assert_eq!(plane(&frames[4]), b, "f4 copies F3");
    // f5: P_L0_16x16 with ref_idx 3 into a 4-entry list. The list is
    // [F4, F3] short-term then [F0 (LT0), F2 (LT1)] long-term, so
    // position 3 is F2: the mmco-3 long-term marking of f4 must have
    // landed AND long-term pictures must sit after every short-term
    // entry in RefPicList0.
    assert_eq!(plane(&frames[5]), b, "f5 references F2 via long-term slot");
    // f6: P_Skip copies f5 = B; the sliding window behind it must have
    // evicted F3 (short-term) while both long-term pictures persist.
    assert_eq!(plane(&frames[6]), b, "f6 copies f5");

    // Determinism: a second decode is byte-identical.
    let again = decode(&stream).expect("second decode");
    assert_eq!(frames, again);
}
