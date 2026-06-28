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
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use crate::error::{MieError, MieResult};
use crate::log_warn;
use crate::models::{CommandWord, MieMessage, Timestamp, TypeWord};
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

// ── Cross-recorder duplicate collapsing (L2-MRG-007) ─────────────────────────

/// Content identity of a message for cross-recorder de-duplication: the bits a
/// recorder reads off the wire — Type Word (message type, bus, word count, error
/// flag, raw), Command/Status Words, Error Word, and the valid data words.
/// Timestamp, file offset, MUX, and DELTA are intentionally excluded — the
/// timestamp drives the window (not equality), and the rest are per-recorder.
#[derive(PartialEq, Eq)]
struct DedupKey {
    type_word: TypeWord,
    command_word: Option<CommandWord>,
    command_word_2: Option<CommandWord>,
    status_word: Option<u16>,
    status_word_2: Option<u16>,
    error_word: Option<u16>,
    data_words: Vec<u16>,
}

impl DedupKey {
    fn of(msg: &MieMessage) -> Self {
        Self {
            type_word: msg.type_word,
            command_word: msg.command_word,
            command_word_2: msg.command_word_2,
            status_word: msg.status_word,
            status_word_2: msg.status_word_2,
            error_word: msg.error_word,
            data_words: msg.data_words.as_slice().to_vec(),
        }
    }
}

/// Sliding time-window de-duplicator over the merged stream (L2-MRG-007). Holds
/// the survivors emitted within `window_us` microseconds as `(us, file_index,
/// key)`; in a sorted stream older survivors can never match a later record and
/// are evicted from the front, so resident memory stays bounded by the window
/// (not the record count). Matching uses the **absolute** time distance, so a
/// lenient non-monotonic input (L2-MRG-006) that steps backward neither panics
/// nor collapses records that lie outside the window — collapse is best-effort
/// on such "known bad" order.
struct DedupWindow {
    window_us: u64,
    survivors: VecDeque<(u64, usize, DedupKey)>,
}

impl DedupWindow {
    fn new(window_us: u64) -> Self {
        Self {
            window_us,
            survivors: VecDeque::new(),
        }
    }

    /// Returns true if `msg` (at `us`, from `file_index`) duplicates a recent
    /// survivor from a **different** input within the window — i.e. the same bus
    /// transaction witnessed by another recorder. Same-file identical content is
    /// never a duplicate. A non-duplicate is recorded as a survivor.
    fn is_duplicate(&mut self, us: u64, file_index: usize, msg: &MieMessage) -> bool {
        // Evict survivors that can no longer fall within the window of the
        // current (or, in a sorted stream, any later) record. `saturating_sub`
        // keeps this safe under lenient non-monotonic input (L2-MRG-006): a
        // record whose timestamp steps backward must not underflow `us - buf_us`.
        while let Some(&(buf_us, _, _)) = self.survivors.front() {
            if us.saturating_sub(buf_us) > self.window_us {
                self.survivors.pop_front();
            } else {
                break;
            }
        }
        let key = DedupKey::of(msg);
        // A survivor matches only if it is within the window in ABSOLUTE time
        // (`abs_diff`): the merged stream may step backward (non-monotonic
        // input), so the distance must be order-independent, never a one-sided
        // subtraction. In a sorted stream every retained survivor already has
        // `us - buf_us <= window`, so this is identical to the prior behavior.
        if self.survivors.iter().any(|(buf_us, fi, k)| {
            *fi != file_index && buf_us.abs_diff(us) <= self.window_us && *k == key
        }) {
            return true;
        }
        self.survivors.push_back((us, file_index, key));
        false
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
    /// In strict mode a within-file backward timestamp step (L2-MRG-006) is a
    /// record error that fails the batch; in lenient mode it only WARNs.
    strict: bool,
    /// Microsecond key of the previous record pulled from each input file, in
    /// capture order. `None` until a file's first record is seen. Used to
    /// detect a within-file backward step (L2-MRG-006).
    prev_us: Vec<Option<u64>>,
    /// One-time-per-file guard so a non-monotonic input WARNs at most once
    /// (lenient mode), mirroring the single-file non-monotonic-DELTA WARN.
    warned_backward: Vec<bool>,
    /// Input paths in resolved order, for naming a file in the L2-MRG-006
    /// non-monotonic WARN / error (the per-file readers are not retained).
    paths: Vec<PathBuf>,
    delta_tracker: HashMap<String, u64>,
    /// Error to surface on the *next* `next()` call (non-`--allow-partial`
    /// mid-stream failure — fails the batch).
    pending_error: Option<MieError>,
    /// Error to surface once the heap drains (an `--allow-partial` deferred
    /// unrecoverable loss — lets the writer commit a `.partial`).
    pending_terminal: Option<MieError>,
    /// Cross-recorder duplicate collapsing (L2-MRG-007). `Some` when enabled via
    /// `--collapse-duplicates`; `None` keeps every row (the default).
    dedup: Option<DedupWindow>,
    /// Count of records suppressed as cross-recorder duplicates, for the CLI's
    /// end-of-run summary. Shared so the CLI can read it after the iterator is
    /// consumed by the writer (mirrors the `sync_losses` `AtomicU64` pattern).
    collapsed: Arc<AtomicU64>,
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
        strict: bool,
    ) -> MieResult<Self> {
        let mut iters: Vec<RecordIter<'a>> = readers.iter().map(|r| r.iter()).collect();
        let mut heap = BinaryHeap::new();
        let mut next_seq = vec![0u64; readers.len()];
        let mut prev_us = vec![None; readers.len()];

        for (idx, iter) in iters.iter_mut().enumerate() {
            match iter.next() {
                Some(Ok(msg)) => {
                    check_mergeable(&msg, idx, readers[idx].path())?;
                    let us = merge_micros(&msg, tick);
                    prev_us[idx] = Some(us);
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

        let warned_backward = vec![false; readers.len()];
        let paths = readers.iter().map(|r| r.path().to_path_buf()).collect();
        Ok(Self {
            iters,
            heap,
            next_seq,
            tick,
            allow_partial,
            strict,
            prev_us,
            warned_backward,
            paths,
            delta_tracker: HashMap::new(),
            pending_error: None,
            pending_terminal: None,
            dedup: None,
            collapsed: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Enable cross-recorder duplicate collapsing on this merge (L2-MRG-007),
    /// builder-style so `new` keeps a stable signature. `enabled == false` (the
    /// default) is a no-op; `window_us` is the timestamp tolerance.
    pub fn collapse(mut self, enabled: bool, window_us: u64) -> Self {
        self.dedup = enabled.then(|| DedupWindow::new(window_us));
        self
    }

    /// A shared handle to the suppressed-duplicate counter (L2-MRG-007). The CLI
    /// clones this before the iterator is consumed by the writer, then reads it
    /// afterward for the end-of-run summary.
    pub fn collapsed_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.collapsed)
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
                // L2-MRG-006: each input is assumed internally time-sorted
                // (capture order is chronological). A backward step means the
                // merged output may be out of order for this file. Strict mode
                // fails the batch; lenient mode WARNs once per file.
                if let Some(prev) = self.prev_us[file_index]
                    && us < prev
                {
                    if self.strict {
                        self.pending_error = Some(MieError::NonMonotonicInput {
                            file_index,
                            path: self.paths[file_index].clone(),
                            prev_us: prev,
                            curr_us: us,
                        });
                    } else if !self.warned_backward[file_index] {
                        self.warned_backward[file_index] = true;
                        log_warn!(
                            "merge: input #{} ({}) is not internally time-sorted: \
                                 timestamp stepped backward (prev_us={} curr_us={}) — merged \
                                 output may be out of order for this input \
                                 (further occurrences suppressed)",
                            file_index,
                            self.paths[file_index].display(),
                            prev,
                            us
                        );
                    }
                }
                self.prev_us[file_index] = Some(us);
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
        loop {
            if let Some(e) = self.pending_error.take() {
                return Some(Err(e));
            }
            let Some(Reverse(entry)) = self.heap.pop() else {
                return self.pending_terminal.take().map(Err);
            };
            let file_index = entry.file_index;
            // Collapse cross-recorder duplicates *before* the global-DELTA stage
            // (L2-MRG-007): a suppressed duplicate must not advance the per-key
            // DELTA tracker, so DELTA is measured across the deduped timeline.
            if let Some(dedup) = self.dedup.as_mut()
                && dedup.is_duplicate(entry.us, file_index, &entry.msg)
            {
                self.collapsed.fetch_add(1, AtomicOrdering::Relaxed);
                self.advance(file_index);
                continue;
            }
            let msg = self.apply_global_delta(entry.msg);
            self.advance(file_index);
            return Some(Ok(msg));
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
