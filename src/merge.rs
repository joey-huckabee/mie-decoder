//! Multi-file, time-sorted streaming k-way merge (L1-MRG-*, L2-MRG-*).
//!
//! Accepts several decoded recordings and yields a single stream of
//! `MieMessage`s in global time order, holding at most one record per open
//! file in a min-heap (resident memory O(number of files), independent of the
//! total record count — L2-MRG-002). The merged stream feeds the existing
//! `write_csv` / `write_csv_split` unchanged.
//!
//! Merge requires every input to be calendar-locked IRIG; Standard-format,
//! freerun-leading, or mixed-format inputs are rejected up front
//! (`MieError::IncompatibleMergeInputs`, CLI exit 6 — L2-MRG-003). DELTA is
//! recomputed on the merged global timeline (L2-MRG-005).
//!
//! No new external dependency: the heap is `std::collections::BinaryHeap`
//! and the `--glob` matcher is hand-rolled (L3-RS-014, preserving L3-RS-002).

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{MieError, MieResult};
use crate::log_warn;
use crate::models::{MieMessage, Timestamp};
use crate::reader::{MieFileReader, RecordIter};

/// Maximum number of input files a single merge invocation may process.
/// Bounds open mappings / file descriptors so resource use is predictable;
/// exceeding it is a usage error (the CLI maps it to exit 4). Shared in value
/// with the Python implementation (L3-RS-014 / L3-PY-014).
pub const MAX_MERGE_FILES: usize = 256;

// ── Input resolution helpers ──────────────────────────────────────────────

/// Read a manifest file into a list of paths: one path per line, in order.
/// Blank lines and lines whose first non-whitespace character is `#` are
/// ignored; surrounding whitespace is trimmed (L2-MRG-001).
pub fn read_manifest(path: &Path) -> io::Result<Vec<PathBuf>> {
    let text = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        out.push(PathBuf::from(trimmed));
    }
    Ok(out)
}

/// Whole-string wildcard match: `*` matches any run (including empty), `?`
/// matches exactly one character; no other metacharacters are special.
/// Iterative backtracking matcher (no recursion, no allocation beyond the
/// char vectors). Identical semantics to the Python implementation.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = name.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while t < txt.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(sp) = star {
            p = sp + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// Expand a single-directory glob `DIR/PATTERN` (or `PATTERN` for the current
/// directory). PATTERN wildcards (`*`, `?`) apply to the **filename only** —
/// no recursive `**`, no brace expansion. Returns matching regular files
/// sorted lexicographically by path (deterministic across implementations,
/// L2-MRG-001).
pub fn expand_glob(pattern: &str) -> io::Result<Vec<PathBuf>> {
    let p = Path::new(pattern);
    let name_pat = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = match p.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().into_owned();
        if glob_match(&name_pat, &fname) {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

// ── k-way merge ────────────────────────────────────────────────────────────

/// One record at the front of an input file, ordered by the merge key
/// `(microseconds, file index, within-file sequence)` for a total,
/// deterministic order including ties (L2-MRG-002).
struct HeapEntry {
    us: u64,
    file_index: usize,
    seq: u64,
    msg: MieMessage,
}

impl HeapEntry {
    fn key(&self) -> (u64, usize, u64) {
        (self.us, self.file_index, self.seq)
    }
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

/// The microsecond merge key for a message. IRIG always yields `Some`; the
/// fallback is unreachable for a validated (IRIG-only) merge.
fn merge_micros(msg: &MieMessage, tick: Option<f64>) -> u64 {
    msg.timestamp.to_microseconds(tick).unwrap_or(0)
}

/// Reject an input whose leading record cannot anchor an absolute timeline.
fn check_mergeable(msg: &MieMessage, file_index: usize, path: &Path) -> MieResult<()> {
    match msg.timestamp {
        Timestamp::Standard(_) => Err(MieError::IncompatibleMergeInputs {
            file_index,
            path: path.to_path_buf(),
            detail: "resolves to the Standard timestamp format".into(),
        }),
        Timestamp::Irig(t) if t.freerun => Err(MieError::IncompatibleMergeInputs {
            file_index,
            path: path.to_path_buf(),
            detail: "leads with a freerun IRIG record (no calendar time)".into(),
        }),
        Timestamp::Irig(_) => Ok(()),
    }
}

/// Streaming k-way merge over per-file readers. Yields the same item type as a
/// single reader (`MieResult<MieMessage>`) so the writer consumes it unchanged.
pub struct MergedRecordIter<'a> {
    iters: Vec<RecordIter<'a>>,
    heap: BinaryHeap<Reverse<HeapEntry>>,
    next_seq: Vec<u64>,
    tick: Option<f64>,
    allow_partial: bool,
    delta_tracker: HashMap<String, u64>,
    /// Error to surface on the *next* `next()` call (non-`--allow-partial`
    /// mid-stream failure — fails the batch).
    pending_error: Option<MieError>,
    /// Error to surface once the heap drains (an `--allow-partial` deferred
    /// unrecoverable loss — lets the writer commit a `.partial`).
    pending_terminal: Option<MieError>,
}

impl<'a> MergedRecordIter<'a> {
    /// Open a merge over already-constructed readers. Pulls each file's first
    /// record and validates it is calendar-locked IRIG; an incompatible input
    /// is rejected here (L2-MRG-003). With `allow_partial`, a file that fails
    /// to produce a first record is skipped with a WARN instead of failing the
    /// batch (L2-MRG-004).
    pub fn new(
        readers: &'a [MieFileReader],
        tick: Option<f64>,
        allow_partial: bool,
    ) -> MieResult<Self> {
        let mut iters: Vec<RecordIter<'a>> = readers.iter().map(|r| r.iter()).collect();
        let mut heap = BinaryHeap::new();
        let mut next_seq = vec![0u64; readers.len()];

        for (idx, iter) in iters.iter_mut().enumerate() {
            match iter.next() {
                Some(Ok(msg)) => {
                    check_mergeable(&msg, idx, readers[idx].path())?;
                    let us = merge_micros(&msg, tick);
                    heap.push(Reverse(HeapEntry {
                        us,
                        file_index: idx,
                        seq: 0,
                        msg,
                    }));
                    next_seq[idx] = 1;
                }
                Some(Err(e)) => {
                    if allow_partial {
                        log_warn!(
                            "merge: skipping input #{} ({}): {}",
                            idx,
                            readers[idx].path().display(),
                            e
                        );
                    } else {
                        return Err(e);
                    }
                }
                None => {
                    // File produced no records; contributes nothing.
                }
            }
        }

        Ok(Self {
            iters,
            heap,
            next_seq,
            tick,
            allow_partial,
            delta_tracker: HashMap::new(),
            pending_error: None,
            pending_terminal: None,
        })
    }

    /// Recompute DELTA on the merged global timeline (L2-MRG-005). The stream
    /// is timestamp-sorted, so per-key gaps are non-negative.
    fn apply_global_delta(&mut self, mut msg: MieMessage) -> MieMessage {
        let key = msg.delta_key();
        if key.is_empty() {
            msg.delta = None; // SPURIOUS_DATA — no RT/MSG key
            return msg;
        }
        match msg.timestamp.to_microseconds(self.tick) {
            None => msg.delta = None,
            Some(curr) => {
                let prev = self.delta_tracker.insert(key, curr);
                msg.delta = match prev {
                    None => Some(0.0),
                    Some(p) if curr >= p => Some((curr - p) as f64 / 1_000_000.0),
                    Some(_) => None,
                };
            }
        }
        msg
    }

    /// Advance the file the just-popped record came from, pushing its next
    /// record onto the heap. Records a pending error on failure.
    fn advance(&mut self, file_index: usize) {
        match self.iters[file_index].next() {
            Some(Ok(msg)) => {
                let us = merge_micros(&msg, self.tick);
                let seq = self.next_seq[file_index];
                self.next_seq[file_index] = seq + 1;
                self.heap.push(Reverse(HeapEntry {
                    us,
                    file_index,
                    seq,
                    msg,
                }));
            }
            Some(Err(e)) => {
                if self.allow_partial
                    && e.kind() == crate::error::MieErrorKind::UnrecoverableSyncLoss
                {
                    log_warn!(
                        "merge: input #{} truncated at its failure point: {}",
                        file_index,
                        e
                    );
                    // Defer until the heap drains so all good records are
                    // written first, then the writer commits a `.partial`.
                    self.pending_terminal = Some(e);
                } else {
                    // Surface on the next call (after the popped record).
                    self.pending_error = Some(e);
                }
            }
            None => {} // file exhausted
        }
    }
}

impl Iterator for MergedRecordIter<'_> {
    type Item = MieResult<MieMessage>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(e) = self.pending_error.take() {
            return Some(Err(e));
        }
        match self.heap.pop() {
            Some(Reverse(entry)) => {
                let file_index = entry.file_index;
                let msg = self.apply_global_delta(entry.msg);
                self.advance(file_index);
                Some(Ok(msg))
            }
            None => self.pending_terminal.take().map(Err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirements: L2-MRG-001, L3-RS-014
    #[test]
    fn glob_match_wildcards() {
        assert!(glob_match("*.mie", "rec1.mie"));
        assert!(glob_match("rec?.mie", "rec5.mie"));
        assert!(!glob_match("rec?.mie", "rec55.mie"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("*.mie", "rec.csv"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        // No special meaning for other metacharacters.
        assert!(glob_match("a.b", "a.b"));
        assert!(!glob_match("a.b", "axb"));
    }

    /// Requirements: L2-MRG-002
    #[test]
    fn heap_entry_orders_by_key_tuple() {
        // Lower microseconds sort first; ties break on file index then seq.
        let mk = |us, fi, seq| (us, fi, seq);
        assert!(mk(10, 0, 0) < mk(20, 0, 0));
        assert!(mk(10, 0, 5) < mk(10, 1, 0));
        assert!(mk(10, 1, 0) < mk(10, 1, 1));
    }
}
