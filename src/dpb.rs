//! Decoded picture buffer: reference marking (sliding window + MMCO
//! ops 1–4), PicNum derivation, and RefPicList0 initialisation plus
//! slice reordering (spec 8.2.4).
//!
//! Short-term and long-term frames are tracked: MMCO op 3 moves a
//! short-term picture to a long-term slot (spec 8.2.5.4.3, including
//! the same-`LongTermFrameIdx` displacement and sliding-window
//! interplay), and long-term pictures join the reference lists after
//! every short-term picture. The remaining long-term assignments —
//! MMCO op 6 (current picture to long-term), op 4 caps and long-term
//! list reordering — surface as [`Error::Unsupported`] rather than
//! silently marking wrong, as does `long_term_reference_flag` on IDR
//! (refused at parse).

use crate::slice::SliceHeader;
use alloc::vec::Vec;
use pith_digest::{Error, Result};

/// Colocation data of one macroblock, kept for temporal direct
/// prediction (spec 8.4.1.2): the colocated picture's L0/L1 motion
/// vectors and reference indices at 4x4-block granularity.
#[derive(Clone, Debug)]
pub(crate) struct ColocMb {
    /// L0 motion vectors per 4x4 block (group-major).
    pub mv_l0: [[i16; 2]; 16],
    /// L1 motion vectors per 4x4 block.
    pub mv_l1: [[i16; 2]; 16],
    /// L0 reference index of each 8x8 group's top-left 4x4 block
    /// (`0xff` = this list unused) — the granularity the direct-
    /// prediction colocation rules read (reference `ref_index[]`).
    pub ref_l0: [u8; 4],
    /// L1 reference indices per 8x8 group.
    pub ref_l1: [u8; 4],
    /// `true` when the colocated MB was intra (mv/ref ignored, direct
    /// prediction falls back to zeros).
    pub intra: bool,
}

/// One reference picture in the DPB: planes + PicNum/POC bookkeeping
/// and the colocation data B-slice direct prediction needs.
#[derive(Clone, Debug)]
pub(crate) struct RefFrame {
    /// `frame_num` the picture was coded with.
    pub frame_num: u32,
    /// `PicNum` of the *current* decode pass: `FrameNum` adjusted by
    /// `FrameNumWrap` (spec 8.2.4.1); recomputed per slice.
    pub pic_num: u32,
    /// Picture order count (spec 8.2.1): RefPicList1 ordering,
    /// temporal-direct `DistScaleFactor` and implicit weighted
    /// prediction all key on POC, not `frame_num`.
    pub poc: i64,
    /// Luma plane (coded width × coded height, row-major).
    pub y: Vec<u8>,
    /// Cb plane.
    pub cb: Vec<u8>,
    /// Cr plane.
    pub cr: Vec<u8>,
    /// Colocation data per macroblock (empty for pictures decoded
    /// before the first B slice appears — temporal direct tolerates
    /// it via the intra/zero path).
    pub coloc: Vec<ColocMb>,
    /// `frame_num`s of this picture's own RefPicList0 at decode time
    /// — the `map_col_to_list0` indirection temporal direct uses to
    /// resolve `refIdxCol` (spec 8.2.5.4 note: the colocated picture's
    /// reference lists are not stored, only their identities).
    pub ref_l0_fns: Vec<u32>,
    /// Same for RefPicList1.
    pub ref_l1_fns: Vec<u32>,
    /// Long-term marking (spec 8.2.5.4.3): `Some(LongTermFrameIdx)`
    /// once an MMCO op 3 moved the picture out of the short-term set;
    /// long-term pictures survive the sliding window and join the
    /// reference lists after every short-term picture.
    pub long_term: Option<u32>,
}

/// The DPB: `refs` is the set of short-term reference frames plus the
/// sliding-window/marked bookkeeping.
#[derive(Default)]
pub(crate) struct Dpb {
    /// Short-term reference frames, insertion order = decode order.
    pub refs: Vec<RefFrame>,
    /// `MaxFrameNum` = `1 << log2_max_frame_num` of the active SPS.
    pub max_frame_num: u32,
    /// `max_num_ref_frames` honoured (min of SPS and Limits).
    pub max_refs: usize,
}

impl Dpb {
    /// DPB state for a new SPS (or IDR): everything is dropped, sizes
    /// updated.
    pub(crate) fn reset(&mut self, max_frame_num: u32, max_refs: u32, limit: u32) {
        self.refs.clear();
        self.max_frame_num = max_frame_num.max(1);
        self.max_refs = (max_refs.min(limit).max(1)) as usize;
    }

    /// IDR boundary: mark every reference unused-for-reference before
    /// the new picture (spec 8.2.5.2; `no_output_of_prior_pics`/`long
    /// term_reference_flag` variations don't apply — we reject the
    /// long-term flag at parse and output doesn't interleave with DPB).
    pub(crate) fn flush(&mut self) {
        self.refs.clear();
    }

    /// Recomputes `PicNum` for every stored reference against the
    /// current picture's `frame_num` (spec 8.2.4.1: PicNum = frame_num
    /// − MaxFrameNum when frame_num > FrameNum, else frame_num).
    /// `FrameNumWrap` applies to short-term pictures only — a long-term
    /// picture keeps `PicNum == frame_num`.
    fn set_pic_nums(&mut self, frame_num: u32) {
        for r in self.refs.iter_mut() {
            r.pic_num = if r.long_term.is_none() && r.frame_num > frame_num {
                r.frame_num.wrapping_sub(self.max_frame_num)
            } else {
                r.frame_num
            };
        }
    }

    /// Applies this slice's reference-picture marking after the picture
    /// is decoded. `cur` is the just-finished frame (already in `refs`
    /// via [`push`]).
    pub(crate) fn apply_marking(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        nal_ref_idc: u8,
    ) -> Result<()> {
        if nal_ref_idc == 0 {
            return Ok(());
        }
        if h.adaptive_marking {
            for m in &h.mmco {
                match m.op {
                    // mark_short_term_unused: PicNum = FrameNum − diff
                    // (picNumX per 8.2.5.4.1); short-term pictures only.
                    1 => {
                        let target = (i64::from(frame_num) - i64::from(m.difference_of_pic_nums))
                            .rem_euclid(i64::from(self.max_frame_num))
                            as u32;
                        self.refs
                            .retain(|r| r.long_term.is_some() || r.pic_num != target);
                    }
                    2 => return Err(Error::Unsupported("h264 MMCO long-term pic mark")),
                    // 3 (spec 8.2.5.4.3): move the short-term picture
                    // `PicNumX = CurrPicNum − (long_term_pic_num + 1)`
                    // to the long-term slot `long_term_frame_idx`. A
                    // long-term picture already holding that slot is
                    // marked unused-for-reference; a missing target is
                    // tolerated only when the slot already holds the
                    // very picture (same frame_num) — ffmpeg's
                    // `execute_ref_pic_marking` semantics.
                    3 => {
                        let target = (i64::from(frame_num)
                            - i64::from(m.long_term_pic_num.wrapping_add(1)))
                        .rem_euclid(i64::from(self.max_frame_num))
                            as u32;
                        let already = self.refs.iter().any(|r| {
                            r.long_term == Some(m.long_term_frame_idx) && r.frame_num == target
                        });
                        if already {
                            continue;
                        }
                        match self
                            .refs
                            .iter_mut()
                            .find(|r| r.long_term.is_none() && r.pic_num == target)
                        {
                            Some(r) => r.long_term = Some(m.long_term_frame_idx),
                            None => {
                                return Err(Error::BadValue(
                                    "mmco 3 target not a short-term reference",
                                ));
                            }
                        }
                        // Displace every other long-term picture at
                        // this slot.
                        let idx = m.long_term_frame_idx;
                        self.refs
                            .retain(|r| r.long_term != Some(idx) || r.frame_num == target);
                    }
                    6 => return Err(Error::Unsupported("h264 long-term reference marking")),
                    4 => return Err(Error::Unsupported("h264 MMCO max_long_term_frame_idx")),
                    // 5: reset — all references (short- and long-term)
                    // unused + POC counters; our bookkeeping only needs
                    // the clear.
                    5 => self.refs.clear(),
                    _ => return Err(Error::BadValue("mmco op out of range")),
                }
            }
        }
        // Sliding window (spec 8.2.5.3): when adaptive marking did not
        // run, evict the lowest-PicNum SHORT-TERM reference until the
        // short-term set fits within max_refs. Long-term pictures are
        // never sliding-window candidates and do not count toward the
        // short-term ceiling.
        if !h.adaptive_marking {
            while self.refs.iter().filter(|r| r.long_term.is_none()).count() > self.max_refs {
                let mut lowest: Option<usize> = None;
                for (i, r) in self.refs.iter().enumerate() {
                    if r.long_term.is_none()
                        && lowest.is_none_or(|l| r.pic_num < self.refs[l].pic_num)
                    {
                        lowest = Some(i);
                    }
                }
                match lowest {
                    Some(i) => {
                        self.refs.remove(i);
                    }
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// Inserts the finished frame as a reference (caller skips when
    /// `nal_ref_idc == 0`) and returns its index.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push(
        &mut self,
        frame_num: u32,
        poc: i64,
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
        coloc: Vec<ColocMb>,
        ref_l0_fns: Vec<u32>,
        ref_l1_fns: Vec<u32>,
    ) -> usize {
        self.refs.push(RefFrame {
            frame_num,
            pic_num: frame_num,
            poc,
            y,
            cb,
            cr,
            coloc,
            ref_l0_fns,
            ref_l1_fns,
            long_term: None,
        });
        self.refs.len() - 1
    }

    /// Builds the initial RefPicList0 for a P or B slice and applies
    /// the slice's reordering commands (spec 8.2.4.2 + 8.2.4.3.1).
    ///
    /// Returns indices into `self.refs` in list order.
    pub(crate) fn ref_list0(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        cur_poc: i64,
        is_b: bool,
    ) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        // Short-term first (spec 8.2.4.2.1/8.2.4.2.3), long-term after:
        // P slices order the short-term set by PicNum descending, B
        // slices by the before/after-POC halves; long-term pictures
        // then follow in ascending LongTermFrameIdx.
        let mut st: Vec<usize> = (0..self.refs.len())
            .filter(|&i| self.refs[i].long_term.is_none())
            .collect();
        let mut lt: Vec<usize> = (0..self.refs.len())
            .filter(|&i| self.refs[i].long_term.is_some())
            .collect();
        if is_b {
            // B slices (spec 8.2.4.2.3): short-term refs with
            // poc < cur_poc first in *decreasing* POC, then
            // poc > cur_poc in *increasing* POC.
            st.sort_by(|&a, &b| {
                let (pa, pb) = (self.refs[a].poc, self.refs[b].poc);
                let (ba, bb) = (pa < cur_poc, pb < cur_poc);
                match (ba, bb) {
                    (true, true) => pb.cmp(&pa),
                    (false, false) => pa.cmp(&pb),
                    (true, false) => core::cmp::Ordering::Less,
                    (false, true) => core::cmp::Ordering::Greater,
                }
            });
        } else {
            // Initial order: PicNum descending (short-term only).
            st.sort_by(|&a, &b| self.refs[b].pic_num.cmp(&self.refs[a].pic_num));
        }
        lt.sort_by(|&a, &b| self.refs[a].long_term.cmp(&self.refs[b].long_term));
        let mut order = st;
        order.extend(lt);
        // The reorder machinery works on `num_ref_idx_active` slots —
        // when the DPB holds fewer pictures the tail slots repeat the
        // last entry (spec 8.2.4.1's fill rule, as the reference
        // decoder's `default_ref` does), and a command targeting a
        // slot index >= cap is malformed.
        let cap = h.num_ref_idx_l0_active as usize;
        if cap == 0 {
            return Err(Error::BadValue("num_ref_idx_l0_active zero"));
        }
        while order.len() < cap {
            let last = *order.last().unwrap_or(&0);
            order.push(last);
        }
        // ffmpeg's ref list is always exactly `num_ref_idx_active`
        // entries; the reorder shift window must not see the tail.
        order.truncate(cap);
        self.apply_reorder(&mut order, &h.reorder_l0, frame_num, cap)?;
        Ok(order)
    }

    /// Builds the initial RefPicList1 for a B slice (spec 8.2.4.2.3):
    /// refs with poc > cur_poc first in increasing POC, then
    /// poc <= cur_poc in decreasing POC — the mirror image of L0 —
    /// followed by the L1 reordering commands and the mandated
    /// "if entry 0 equals L0[0], swap entries 0 and 1" rule.
    pub(crate) fn ref_list1(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        cur_poc: i64,
        l0: &[usize],
    ) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        // Short-term entries first (after-cur ascending, then
        // before-or-equal descending), long-term by ascending
        // LongTermFrameIdx appended after them (spec 8.2.4.2.3).
        let mut st: Vec<usize> = (0..self.refs.len())
            .filter(|&i| self.refs[i].long_term.is_none())
            .collect();
        let mut lt: Vec<usize> = (0..self.refs.len())
            .filter(|&i| self.refs[i].long_term.is_some())
            .collect();
        st.sort_by(|&a, &b| {
            let (pa, pb) = (self.refs[a].poc, self.refs[b].poc);
            // After-cur first (ascending), then before-or-equal
            // (descending).
            let (aa, ab) = (pa > cur_poc, pb > cur_poc);
            match (aa, ab) {
                (true, true) => pa.cmp(&pb),
                (false, false) => pb.cmp(&pa),
                (true, false) => core::cmp::Ordering::Less,
                (false, true) => core::cmp::Ordering::Greater,
            }
        });
        lt.sort_by(|&a, &b| self.refs[a].long_term.cmp(&self.refs[b].long_term));
        let mut order = st;
        order.extend(lt);
        let cap = h.num_ref_idx_l1_active as usize;
        if cap == 0 {
            return Err(Error::BadValue("num_ref_idx_l1_active zero"));
        }
        while order.len() < cap {
            let last = *order.last().unwrap_or(&0);
            order.push(last);
        }
        order.truncate(cap);
        self.apply_reorder(&mut order, &h.reorder_l1, frame_num, cap)?;
        // Spec 8.2.4.2.3: when RefPicList1[0] == RefPicList0[0] and
        // both lists have entries, swap the first two L1 entries so
        // a same-picture pair does not stall direct prediction.
        if order.len() > 1 && !l0.is_empty() && order[0] == l0[0] {
            order.swap(0, 1);
        }
        Ok(order)
    }

    /// Reference-picture-list reordering (spec 8.2.4.3.1), shared by
    /// L0 and L1: `pred` starts at the current picture's PicNum and
    /// each `abs_diff_pic_num_minus1` command walks it while the
    /// picked entry is moved to `refIdxLX`.
    fn apply_reorder(
        &mut self,
        order: &mut Vec<usize>,
        cmds: &[(u32, u32)],
        frame_num: u32,
        cap: usize,
    ) -> Result<()> {
        if cmds.is_empty() {
            return Ok(());
        }
        let mut list = order.clone();
        let mut pred = i64::from(frame_num);
        let mut idx: usize = 0;
        for &(idc, val) in cmds {
            if idc == 0 || idc == 1 {
                let v = i64::from(val) + 1;
                pred = if idc == 0 {
                    (pred - v).rem_euclid(i64::from(self.max_frame_num))
                } else {
                    (pred + v).rem_euclid(i64::from(self.max_frame_num))
                };
                let want = pred as u32;
                // ffmpeg `h264_refs.c` reorder semantics (the reference
                // decoder): resolve the target picture by PicNum among
                // all refs (any list position), then search only the
                // TAIL `list[idx..n-1)` for it. Found: shift the tail
                // right and place it at `idx` (moves it forward). Not
                // found: still shift + place — the picture lands at
                // `idx` even when it already sits earlier in the list
                // (a deliberate duplicate; remove+insert across the
                // whole list is WRONG and picks the wrong reference).
                let resolved = self
                    .refs
                    .iter()
                    .position(|r| r.long_term.is_none() && r.pic_num == want)
                    .ok_or(Error::BadValue("ref reorder target not in list"))?;
                if idx >= cap {
                    return Err(Error::BadValue("ref reorder index out of range"));
                }
                let n = list.len();
                let mut pos = idx;
                let mut found = false;
                while pos + 1 < n {
                    let j = list[pos];
                    if self.refs[j].long_term.is_none() && self.refs[j].pic_num == want {
                        found = true;
                        break;
                    }
                    pos += 1;
                }
                if !found {
                    pos = n.saturating_sub(1).max(idx);
                }
                for j in (idx + 1..=pos).rev() {
                    list[j] = list[j - 1];
                }
                list[idx] = resolved;
                idx += 1;
            } else {
                return Err(Error::Unsupported("h264 long-term ref reorder"));
            }
        }
        *order = list;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slice::{Mmco, SliceHeader};
    use alloc::{vec, vec::Vec};

    /// Minimal P-slice header with the given marking command list.
    fn marking_header(adaptive: bool, mmco: Vec<Mmco>) -> SliceHeader {
        SliceHeader {
            first_mb: 0,
            slice_type: crate::slice::SliceType::P,
            pps_id: 0,
            frame_num: 0,
            idr_pic_id: 0,
            pic_order_cnt_lsb: 0,
            delta_poc_bottom: 0,
            delta_poc0: 0,
            delta_poc1: 0,
            num_ref_override: false,
            num_ref_idx_l0_active: 1,
            num_ref_idx_l1_active: 1,
            reorder_l0: alloc::vec::Vec::new(),
            reorder_l1: alloc::vec::Vec::new(),
            wp_l0: None,
            wp_l1: None,
            wp_denom: (0, 0),
            direct_spatial: false,
            cabac_init_idc: 0,
            idr_marking: (false, false),
            adaptive_marking: adaptive,
            mmco,
            slice_qp_delta: 0,
            disable_deblock_idc: 1,
            offset_a: 0,
            offset_b: 0,
        }
    }

    fn mmco3(long_term_pic_num: u32, idx: u32) -> Mmco {
        Mmco {
            op: 3,
            difference_of_pic_nums: 0,
            long_term_pic_num,
            long_term_frame_idx: idx,
            max_long_term_frame_idx: 0,
        }
    }

    fn dpb() -> Dpb {
        let mut d = Dpb {
            refs: Vec::new(),
            max_frame_num: 16,
            max_refs: 2,
        };
        d.reset(16, 2, 16);
        d
    }

    fn push_fn(d: &mut Dpb, frame_num: u32) -> usize {
        d.push(
            frame_num,
            i64::from(frame_num),
            vec![0; 16],
            vec![0; 4],
            vec![0; 4],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    /// Op 3 moves the named short-term picture to the long-term slot
    /// (spec 8.2.5.4.3): PicNumX = CurrPicNum − (long_term_pic_num + 1).
    #[test]
    fn mmco3_moves_short_term_to_long_term() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        push_fn(&mut d, 1);
        // CurrPicNum = 2: PicNumX = 2 - (0 + 1) = 1 -> frame_num 1.
        let h = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h, 2, 3).expect("marking");
        let moved = d.refs.iter().find(|r| r.frame_num == 1).expect("kept");
        assert_eq!(moved.long_term, Some(0), "frame 1 marked long-term idx 0");
        assert!(
            d.refs
                .iter()
                .all(|r| r.frame_num != 0 || r.long_term.is_none())
        );
    }

    /// The picture already long-term at that idx is displaced: marked
    /// unused and removed, exactly one occupant per idx (spec 8.2.5.4.3
    /// "any other DPB picture with the same LongTermFrameIdx is marked
    /// unused"; ffmpeg `execute_ref_pic_marking` removes it).
    #[test]
    fn mmco3_displaces_existing_long_term_at_idx() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        let h0 = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h0, 1, 3).expect("F0 -> LT0");
        push_fn(&mut d, 1);
        let h1 = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h1, 2, 3).expect("F1 -> LT0");
        assert!(d.refs.iter().all(|r| r.frame_num != 0), "old LT0 displaced");
        let occupant = d.refs.iter().find(|r| r.long_term == Some(0)).expect("new");
        assert_eq!(occupant.frame_num, 1, "exactly one picture at LT idx 0");
    }

    /// ffmpeg tolerance: op 3 naming a picture that is ALREADY
    /// long-term at that idx with the same frame_num is a silent
    /// no-op, not an error (`find_short` failure is only fatal for
    /// genuinely short-term targets).
    #[test]
    fn mmco3_already_long_term_is_noop() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        let h = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h, 1, 3).expect("F0 -> LT0");
        // Same command again: PicNumX = 1 - 1 = 0, already LT at idx 0.
        d.apply_marking(&h, 1, 3).expect("tolerated");
        assert_eq!(d.refs.len(), 1, "still exactly one picture");
        assert_eq!(d.refs[0].long_term, Some(0));
    }

    /// Op 3 with no matching short-term picture is a bitstream error.
    #[test]
    fn mmco3_missing_target_is_error() {
        let mut d = dpb();
        push_fn(&mut d, 3);
        // CurrPicNum = 4: PicNumX = 4 - (1 + 1) = 2 -> nothing there.
        let h = marking_header(true, vec![mmco3(1, 0)]);
        assert!(d.apply_marking(&h, 4, 3).is_err());
    }

    /// Op 1 removes only short-term pictures: a long-term picture with
    /// the same derived PicNum survives (spec 8.2.5.4.1 operates on the
    /// short-term set; `remove_long` never fires from op 1).
    #[test]
    fn mmco1_spares_long_term() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        let h0 = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h0, 1, 3).expect("F0 -> LT0");
        // CurrPicNum = 2: op 1 diff 1 -> PicNumX = 1; F0 (LT) has
        // PicNum 0 and would not match anyway, but assert the LT flag
        // also shields it from any wrap-collision.
        let m = Mmco {
            op: 1,
            difference_of_pic_nums: 0,
            long_term_pic_num: 0,
            long_term_frame_idx: 0,
            max_long_term_frame_idx: 0,
        };
        let h = marking_header(true, vec![m]);
        d.apply_marking(&h, 2, 3).expect("op 1");
        assert_eq!(d.refs.len(), 1);
        assert_eq!(d.refs[0].long_term, Some(0), "long-term survives op 1");
    }

    /// The sliding window evicts only short-term pictures: long-term
    /// pictures persist past the max_refs ceiling and do not count
    /// toward it (spec 8.2.5.3 evicts the oldest *short-term* frame).
    #[test]
    fn sliding_window_spares_long_term() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        let h0 = marking_header(true, vec![mmco3(0, 0)]);
        d.apply_marking(&h0, 1, 3).expect("F0 -> LT0");
        push_fn(&mut d, 2);
        push_fn(&mut d, 3);
        let h = marking_header(false, Vec::new());
        d.apply_marking(&h, 4, 3).expect("window");
        assert!(d.refs.iter().any(|r| r.long_term == Some(0)), "LT kept");
        let st: Vec<_> = d.refs.iter().filter(|r| r.long_term.is_none()).collect();
        assert!(st.len() <= 2, "short-term ceiling");
        assert!(st.iter().all(|r| r.frame_num >= 2), "oldest ST evicted");
    }

    /// RefPicList0: short-term entries first (PicNum descending), then
    /// long-term entries by ascending idx (spec 8.2.4.2.1).
    #[test]
    fn ref_list0_appends_long_term_after_short_term() {
        let mut d = dpb();
        push_fn(&mut d, 0);
        let h0 = marking_header(true, vec![mmco3(0, 1)]);
        d.apply_marking(&h0, 1, 3).expect("F0 -> LT1");
        push_fn(&mut d, 2);
        push_fn(&mut d, 3);
        // The list is capped at num_ref_idx_l0_active, so request all
        // three stored pictures (2 short-term + 1 long-term).
        let mut h = marking_header(false, Vec::new());
        h.num_ref_idx_l0_active = 3;
        let list = d.ref_list0(&h, 4, 4, false).expect("list");
        let lt_pos = list
            .iter()
            .position(|&i| d.refs[i].long_term.is_some())
            .expect("LT in list");
        assert!(
            list[..lt_pos]
                .iter()
                .all(|&i| d.refs[i].long_term.is_none()),
            "all short-term before long-term"
        );
        assert_eq!(d.refs[list[lt_pos]].long_term, Some(1));
    }
}
