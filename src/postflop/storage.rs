//! Solver storage: per-node regrets and cumulative strategies, either as
//! `f32` (8 bytes per action and hand) or compressed to 16 bits (4 bytes).
//!
//! Compressed blocks store each value as a fraction of the block's largest
//! magnitude: `i16` regrets and `u16` cumulative strategies, each with one
//! `f32` scale per node. Blocks are decoded, updated in `f32` and re-encoded.

use std::borrow::Cow;
use std::cell::UnsafeCell;

/// A flat buffer that solver threads write into concurrently.
///
/// Safety contract: every player node owns a disjoint block, and parallel
/// recursion only splits at chance nodes, whose children are disjoint
/// subtrees, so no two threads touch the same block at the same time.
pub(crate) struct SharedBuf<T>(UnsafeCell<Vec<T>>);

unsafe impl<T: Send> Sync for SharedBuf<T> {}

impl<T: Copy + Default> SharedBuf<T> {
    pub fn zeros(len: usize) -> Self {
        SharedBuf(UnsafeCell::new(vec![T::default(); len]))
    }

    pub fn from_vec(v: Vec<T>) -> Self {
        SharedBuf(UnsafeCell::new(v))
    }

    /// # Safety
    /// The caller must hold the only live reference to this range (see the type docs).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice_mut(&self, off: usize, len: usize) -> &mut [T] {
        unsafe { &mut (&mut *self.0.get())[off..off + len] }
    }

    /// # Safety
    /// No thread may be writing this range concurrently.
    pub unsafe fn slice(&self, off: usize, len: usize) -> &[T] {
        unsafe { &(&*self.0.get())[off..off + len] }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.0.get_mut()
    }
}

/// Location of one player node's `actions x hands` block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub off: usize,
    pub len: usize,
    /// Index of the player node (for per-node scales).
    pub pidx: usize,
}

pub(crate) enum Storage {
    Full {
        regrets: SharedBuf<f32>,
        cum: SharedBuf<f32>,
        /// `cum` already holds normalized average strategies.
        normalized: bool,
    },
    Compressed {
        regrets: SharedBuf<i16>,
        cum: SharedBuf<u16>,
        /// Two per player node: regret scale, cumulative-strategy scale.
        scales: SharedBuf<f32>,
    },
}

pub(crate) fn bytes_per_entry(compressed: bool) -> u64 {
    if compressed { 4 } else { 8 }
}

fn encode_i16(src: &[f32], dst: &mut [i16]) -> f32 {
    let max = src.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    if max <= 0.0 || !max.is_finite() {
        dst.fill(0);
        return 0.0;
    }
    let scale = max / i16::MAX as f32;
    let inv = 1.0 / scale;
    for (d, &s) in dst.iter_mut().zip(src) {
        *d = (s * inv).round() as i16;
    }
    scale
}

fn encode_u16(src: &[f32], dst: &mut [u16]) -> f32 {
    let max = src.iter().fold(0.0f32, |m, &x| m.max(x));
    if max <= 0.0 || !max.is_finite() {
        dst.fill(0);
        return 0.0;
    }
    let scale = max / u16::MAX as f32;
    let inv = 1.0 / scale;
    for (d, &s) in dst.iter_mut().zip(src) {
        *d = (s * inv).round() as u16;
    }
    scale
}

impl Storage {
    pub fn new(len: usize, player_nodes: usize, compressed: bool) -> Self {
        if compressed {
            Storage::Compressed {
                regrets: SharedBuf::zeros(len),
                cum: SharedBuf::zeros(len),
                scales: SharedBuf::zeros(2 * player_nodes),
            }
        } else {
            Storage::Full { regrets: SharedBuf::zeros(len), cum: SharedBuf::zeros(len), normalized: false }
        }
    }

    /// Storage holding a normalized average strategy only (a loaded solution).
    pub fn from_average(avg: Vec<f32>) -> Self {
        Storage::Full { regrets: SharedBuf::zeros(0), cum: SharedBuf::from_vec(avg), normalized: true }
    }

    /// # Safety
    /// No thread may be writing block `b` concurrently.
    pub unsafe fn regrets(&self, b: Block) -> Cow<'_, [f32]> {
        match self {
            Storage::Full { regrets, .. } => Cow::Borrowed(unsafe { regrets.slice(b.off, b.len) }),
            Storage::Compressed { regrets, scales, .. } => {
                let s = unsafe { scales.slice(2 * b.pidx, 1)[0] };
                let q = unsafe { regrets.slice(b.off, b.len) };
                Cow::Owned(q.iter().map(|&x| x as f32 * s).collect())
            }
        }
    }

    /// Calls `f(regrets, cumulative_strategy)` on block `b` and stores the result.
    ///
    /// # Safety
    /// The caller must have exclusive access to block `b`.
    pub unsafe fn update(&self, b: Block, f: impl FnOnce(&mut [f32], &mut [f32])) {
        match self {
            Storage::Full { regrets, cum, .. } => unsafe {
                f(regrets.slice_mut(b.off, b.len), cum.slice_mut(b.off, b.len));
            },
            Storage::Compressed { regrets, cum, scales } => unsafe {
                let sc = scales.slice_mut(2 * b.pidx, 2);
                let rq = regrets.slice_mut(b.off, b.len);
                let cq = cum.slice_mut(b.off, b.len);
                let mut r: Vec<f32> = rq.iter().map(|&x| x as f32 * sc[0]).collect();
                let mut c: Vec<f32> = cq.iter().map(|&x| x as f32 * sc[1]).collect();
                f(&mut r, &mut c);
                sc[0] = encode_i16(&r, rq);
                sc[1] = encode_u16(&c, cq);
            },
        }
    }

    /// Normalized average strategy of block `b`, `actions x hands`.
    ///
    /// # Safety
    /// No thread may be writing block `b` concurrently.
    pub unsafe fn average(&self, b: Block, na: usize, nh: usize) -> Vec<f32> {
        match self {
            Storage::Full { cum, normalized: true, .. } => unsafe { cum.slice(b.off, b.len).to_vec() },
            Storage::Full { cum, .. } => normalize_strategy(unsafe { cum.slice(b.off, b.len) }, na, nh),
            Storage::Compressed { cum, .. } => {
                // The block scale cancels out in the normalization.
                let raw: Vec<f32> = unsafe { cum.slice(b.off, b.len) }.iter().map(|&x| x as f32).collect();
                normalize_strategy(&raw, na, nh)
            }
        }
    }

    /// Frees the regrets and, for full storage, normalizes `cum` in place.
    /// `blocks` must list every player node with its action and hand counts.
    pub fn finalize(&mut self, blocks: &[(Block, usize, usize)]) {
        use rayon::prelude::*;
        match self {
            Storage::Full { regrets, cum, normalized } => {
                *regrets = SharedBuf::zeros(0);
                if *normalized {
                    return;
                }
                let data = cum.as_mut_slice();
                let mut sorted: Vec<&(Block, usize, usize)> = blocks.iter().collect();
                sorted.sort_unstable_by_key(|(b, _, _)| b.off);
                let mut slices = Vec::with_capacity(sorted.len());
                let mut rest: &mut [f32] = data;
                let mut pos = 0;
                for (b, na, nh) in sorted {
                    let (_, tail) = rest.split_at_mut(b.off - pos);
                    let (blk, tail) = tail.split_at_mut(b.len);
                    slices.push((blk, *na, *nh));
                    rest = tail;
                    pos = b.off + b.len;
                }
                slices.into_par_iter().for_each(|(blk, na, nh)| {
                    let n = normalize_strategy(blk, na, nh);
                    blk.copy_from_slice(&n);
                });
                *normalized = true;
            }
            Storage::Compressed { regrets, .. } => *regrets = SharedBuf::zeros(0),
        }
    }
}

pub(crate) fn normalize_strategy(raw: &[f32], na: usize, nh: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; na * nh];
    for h in 0..nh {
        let sum: f32 = (0..na).map(|a| raw[a * nh + h]).sum();
        if sum > 0.0 {
            for a in 0..na {
                out[a * nh + h] = raw[a * nh + h] / sum;
            }
        } else {
            for a in 0..na {
                out[a * nh + h] = 1.0 / na as f32;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_roundtrip_is_close() {
        let s = Storage::new(6, 1, true);
        let b = Block { off: 0, len: 6, pidx: 0 };
        let vals = [1.5f32, -3.0, 0.001, 250.0, -0.5, 0.0];
        unsafe {
            s.update(b, |r, c| {
                r.copy_from_slice(&vals);
                c.copy_from_slice(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
            });
            let r = s.regrets(b);
            for (x, y) in r.iter().zip(&vals) {
                assert!((x - y).abs() <= 250.0 / 32767.0, "{x} vs {y}");
            }
            let avg = s.average(b, 2, 3);
            // Hand 0: cum 0 and 3 -> (0, 1); hand 1: 1 and 4 -> (0.2, 0.8).
            assert!((avg[0] - 0.0).abs() < 1e-3 && (avg[3] - 1.0).abs() < 1e-3);
            assert!((avg[1] - 0.2).abs() < 1e-3 && (avg[4] - 0.8).abs() < 1e-3);
        }
    }
}
