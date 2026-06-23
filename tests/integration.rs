//! End-to-end integration tests using byte-exact fixtures from the
//! Python reference's `tests/conftest.py`. Each fixture has been
//! cross-referenced against vendor-generated CSV output, so they serve
//! as oracles for the Rust port.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mie_decoder::filter::{FilterConfig, FilterIterExt};
use mie_decoder::models::{Bus, Direction, MessageFormat};
use mie_decoder::reader::MieFileReader;
use mie_decoder::writer::write_csv;

/// Requirements: L3-RS-013
///
/// The crate root re-exports the public decode entry point and core types
/// via `pub use` (src/lib.rs), so downstream crates can name them without
/// the internal module path. Each helper accepts a *module-path* type but
/// is bound to a function pointer over the *crate-root* path; that binding
/// compiles only if the root path resolves AND is the same type as the
/// module path (a genuine re-export, not a coincidental name).
#[test]
fn crate_root_reexports_public_decode_api() {
    fn takes_reader(_: mie_decoder::reader::MieFileReader) {}
    let _: fn(mie_decoder::MieFileReader) = takes_reader;

    fn takes_message(_: mie_decoder::models::MieMessage) {}
    let _: fn(mie_decoder::MieMessage) = takes_message;

    fn takes_error(_: mie_decoder::error::MieError) {}
    let _: fn(mie_decoder::MieError) = takes_error;
}

// ── Fixtures (byte-exact from python/tests/conftest.py) ───────────────

fn record_rt15_sa11_rcv() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("02240F1826DB21F6"); // Type + IRIG TS
    s.push_str("7E79"); // Cmd 0x797E (RT15 R SA11 30dw)
    s.push_str("0004");
    s.push_str("0000");
    s.push_str("0000");
    s.push_str("2F00");
    s.push_str("22CA");
    s.push_str("2F00");
    s.push_str("22CA");
    for _ in 0..22 {
        s.push_str("0000");
    }
    s.push_str("71C7");
    s.push_str("0078"); // Status 0x7800
    hex(&s)
}

fn record_rt15_sa22_rcv() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("02110F1826DB38F7"); // Type 0x1102 (wc=17), TS
    s.push_str("CB7A"); // Cmd 0x7ACB (RT15 R SA22 11dw)
    s.push_str("0010");
    s.push_str("0000");
    s.push_str("0700");
    s.push_str("0008");
    for _ in 0..5 {
        s.push_str("0000");
    }
    s.push_str("C880");
    s.push_str("E803");
    s.push_str("0078"); // Status
    hex(&s)
}

fn record_rt15_sa22_xmt() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("04240F1826DBE3F9"); // Type 0x2404 (wc=36, type=0x04 transmit), TS
    s.push_str("DE7E"); // Cmd 0x7EDE (RT15 T SA22 30dw)
    s.push_str("0078"); // Status (transmit puts status before data)
    s.push_str("2010");
    s.push_str("8241");
    s.push_str("0000");
    s.push_str("0815");
    for _ in 0..4 {
        s.push_str("0000");
    }
    s.push_str("00FE");
    for _ in 0..9 {
        s.push_str("0000");
    }
    s.push_str("0300");
    for _ in 0..6 {
        s.push_str("0000");
    }
    s.push_str("0020");
    for _ in 0..4 {
        s.push_str("0000");
    }
    hex(&s)
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

// ── Temp file helper ──────────────────────────────────────────────────

struct TempFile(PathBuf);
impl TempFile {
    fn new(bytes: &[u8]) -> Self {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("mie-int-{pid}-{n}.bin"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

/// Requirements: L2-RDR-007
#[test]
fn single_receive_record_decodes_to_expected_fields() {
    let bytes = record_rt15_sa11_rcv();
    assert_eq!(bytes.len(), 72);
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1);
    let m = &msgs[0];
    assert_eq!(m.command_word.unwrap().rt, 15);
    assert_eq!(m.command_word.unwrap().subaddress, 11);
    assert_eq!(m.command_word.unwrap().direction, Direction::Receive);
    assert_eq!(m.message_format, MessageFormat::Receive);
    assert_eq!(m.bus(), Bus::A);
    assert_eq!(m.data_words.len(), 30);
    assert_eq!(m.data_words.as_slice()[0], 0x0400);
    assert_eq!(m.data_words.as_slice()[3], 0x002F);
    assert_eq!(m.data_words.as_slice()[4], 0xCA22);
    assert_eq!(m.data_words.as_slice()[29], 0xC771);
    assert_eq!(m.status_word, Some(0x7800));
    assert_eq!(m.error_label(), "");
}

/// Requirements: L2-RDR-008
#[test]
fn single_transmit_record_layout() {
    let bytes = record_rt15_sa22_xmt();
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1);
    let m = &msgs[0];
    assert_eq!(m.command_word.unwrap().direction, Direction::Transmit);
    assert_eq!(m.command_word.unwrap().subaddress, 22);
    assert_eq!(m.message_format, MessageFormat::Transmit);
    assert_eq!(m.status_word, Some(0x7800));
    assert_eq!(m.data_words.len(), 30);
}

/// Requirements: L2-RDR-015
#[test]
fn multi_record_stream() {
    let mut bytes = Vec::new();
    bytes.extend(record_rt15_sa11_rcv());
    bytes.extend(record_rt15_sa22_rcv());
    bytes.extend(record_rt15_sa22_xmt());
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].command_word.unwrap().subaddress, 11);
    assert_eq!(msgs[1].command_word.unwrap().subaddress, 22);
    assert_eq!(msgs[2].command_word.unwrap().subaddress, 22);
    assert_eq!(msgs[2].command_word.unwrap().direction, Direction::Transmit);
    // file_offsets cumulative
    assert_eq!(msgs[0].file_offset, 0);
    assert_eq!(msgs[1].file_offset, 72);
    assert_eq!(msgs[2].file_offset, 72 + 34); // sa22 rcv = 17 words = 34 bytes
}

/// Requirements: L2-SYN-011, L1-EXIT-004
#[test]
fn lenient_mode_unrecoverable_sync_loss_yields_terminal_error() {
    // L1-EXIT-004 lenient-mode contract: when sync recovery exhausts within
    // the 64 KB scan window, the iterator must yield a terminal
    // Err(UnrecoverableSyncLoss) item before stopping. Previously this
    // returned None silently and the CLI exited 0 with truncated data.
    use mie_decoder::error::{MieError, MieErrorKind};

    // Two valid records back-to-back so the first record's look-ahead
    // check sees the second record's Type Word and accepts. Then 70 KB
    // of 0xFF — guarantees recover_sync from the second-record boundary
    // exhausts the 64 KB scan window without finding any valid Type
    // Word.
    let mut bytes = record_rt15_sa11_rcv();
    bytes.extend(record_rt15_sa11_rcv());
    bytes.extend(vec![0xFFu8; 70_000]);
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let mut it = reader.iter();

    // First record decodes normally.
    match it.next() {
        Some(Ok(msg)) => assert_eq!(msg.command_word.unwrap().rt, 15),
        other => panic!("expected first record OK, got {other:?}"),
    }

    // Second call: validation fails on the 0xFF tail, recover_sync
    // walks 64 KB without finding sync, terminal Err surfaces.
    match it.next() {
        Some(Err(e)) => {
            assert_eq!(e.kind(), MieErrorKind::UnrecoverableSyncLoss);
            if let MieError::UnrecoverableSyncLoss { sync_losses, .. } = e {
                assert!(sync_losses >= 1);
            } else {
                unreachable!();
            }
        }
        other => panic!("expected Some(Err(UnrecoverableSyncLoss)), got {other:?}"),
    }

    // Subsequent calls: None forever.
    assert!(it.next().is_none());
    assert!(it.next().is_none());

    drop(it);
    // Reader-level counter is consistent with what the terminal error
    // reported. (Reader's getter is now exposed for the CLI's L1-EXIT-005
    // exit-class summary.)
    assert!(reader.sync_losses() >= 1);
}

/// L1-SYN-002: recovery scanning is forward-only and bounded — across a
/// full decode the cumulative scan never re-traverses already-scanned
/// bytes. We exercise repeated recoveries (valid records separated by
/// short recoverable garbage) and assert the observable consequence: the
/// decoded record offsets advance strictly forward and stay within the
/// file, and the recovery count is bounded (one per corruption region,
/// never an unbounded re-scan).
/// Requirements: L1-SYN-002
#[test]
fn recovery_scan_is_forward_only_and_bounded() {
    // Three RR blocks separated by short recoverable garbage:
    //   RR [garbage] RR [garbage] RR
    // Two valid records per block so the leading record passes its
    // two-record look-ahead; each 0xFF run fails validation at the
    // post-block boundary, so recover_sync walks forward (well within the
    // 64 KB per-recovery cap) to the next block.
    const GARBAGE: usize = 16;
    let rec = record_rt15_sa11_rcv();
    let mut block = rec.clone();
    block.extend(&rec);
    let mut bytes = Vec::new();
    bytes.extend(&block);
    bytes.extend(vec![0xFFu8; GARBAGE]);
    bytes.extend(&block);
    bytes.extend(vec![0xFFu8; GARBAGE]);
    bytes.extend(&block);
    let file_len = bytes.len() as u64;

    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader
        .iter()
        .collect::<Result<_, _>>()
        .expect("recoverable corruption must decode to completion, not error");

    // Recovery fired (more than one block decoded) but bounded.
    assert!(msgs.len() >= 2, "recovery should reach later blocks");

    // Forward-only: offsets strictly increase and never exceed the file —
    // the reader never rewinds into already-scanned bytes (the core of the
    // L1-SYN-002 cumulative bound).
    for pair in msgs.windows(2) {
        assert!(
            pair[1].file_offset > pair[0].file_offset,
            "record offsets must advance strictly forward: {} then {}",
            pair[0].file_offset,
            pair[1].file_offset
        );
    }
    assert!(msgs.last().unwrap().file_offset < file_len);

    // Bounded: one recovery per corruption region (two regions here),
    // never an unbounded re-scan. A forward-only scanner can recover at
    // most once per 2-byte step, far below file_len/2.
    let losses = reader.sync_losses();
    assert!(
        (1..=2).contains(&losses),
        "expected 1-2 recoveries (one per corruption region), got {losses}"
    );
}

/// L2-DEC-009: payload extraction is bounded by the Type Word's declared
/// extent and never consumes bytes from the following record. A Command
/// Word that declares more data words than the Type Word's `word_count`
/// can hold is rejected by the L2-SYN-022 capacity invariant *before*
/// extraction runs, and the reader additionally slices to the record
/// extent (`&self.data[..record_end]`) — so a malformed record can never
/// overrun into its successor.
/// Requirements: L2-DEC-009
#[test]
fn payload_extraction_does_not_overrun_into_next_record() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::models::TimestampFormat;
    use mie_decoder::reader::ReaderOptions;

    // R1: Type Word declares word_count = 10 words (20 bytes), but the
    // Command Word 0x797E declares data_word_count = 30 — far more payload
    // than 10 words can hold. R2: a normal valid record immediately after.
    let mut r1 = Vec::new();
    r1.extend_from_slice(&0x0A02u16.to_le_bytes()); // Type: wc=10, type=0x02 (BC->RT)
    r1.extend_from_slice(&[0x0F, 0x18, 0x26, 0xDB, 0x21, 0xF6]); // IRIG ts (3 words)
    r1.extend_from_slice(&0x797Eu16.to_le_bytes()); // Cmd: RT15 R SA11 dwc=30
    r1.extend_from_slice(&[0u8; 10]); // 5 payload words → total 10 words = 20 bytes
    assert_eq!(r1.len(), 20);

    let r2 = record_rt15_sa11_rcv();
    let mut bytes = r1.clone();
    bytes.extend_from_slice(&r2);
    let f = TempFile::new(&bytes);

    // Strict: the over-declaration is rejected (capacity invariant) rather
    // than silently decoded into an overrun.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            strict: true,
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    match reader.iter().next() {
        Some(Err(e)) => assert_eq!(
            e.kind(),
            MieErrorKind::PayloadError,
            "over-declaring record should be a capacity rejection, got {:?}",
            e.kind()
        ),
        other => panic!("expected Some(Err(PayloadError)), got {other:?}"),
    }

    // Lenient: R1 is skipped and the following R2 decodes intact at its
    // true offset — proving R1 consumed nothing beyond its 20-byte extent.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1, "only the valid R2 survives");
    assert_eq!(
        msgs[0].file_offset, 20,
        "R2 begins exactly after R1's 20-byte declared extent"
    );
    assert_eq!(msgs[0].command_word.unwrap().rt, 15);
}

/// L2-DEC-009 / L1-ROB-001 / L2-SYN-027: an RT-to-RT record whose *second*
/// Command Word over-declares `data_word_count` must not read past the Type
/// Word's declared extent. The L2-SYN-022 capacity invariant is computed from
/// Cmd1, but RT-to-RT extraction takes its count from Cmd2 (the transmit
/// command); fuzzed bytes can keep Cmd1's count small (so the capacity check
/// passes and the record fits the file) while Cmd2 claims 30 words. Extraction
/// reads from the record-bounded slice (`&self.data[..record_end]`) so it
/// completes safely (L2-DEC-009); the over-claim is then a Cmd1/Cmd2
/// `data_word_count` disagreement, which the post-extract L2-SYN-027 invariant
/// rejects (strict errors, lenient skips). Mirrors the Python
/// `test_rt_to_rt_cmd2_overclaim_does_not_overrun`; complements
/// `payload_extraction_does_not_overrun_into_next_record` (the Cmd1 path the
/// capacity invariant catches pre-extract).
/// Requirements: L2-DEC-009, L1-ROB-001, L2-SYN-027
#[test]
fn rt_to_rt_cmd2_overclaim_does_not_overrun() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::models::TimestampFormat;
    use mie_decoder::reader::ReaderOptions;

    // R1: Type Word word_count = 10 (20 bytes), type 0x08 (RT-to-RT). Cmd1
    // 0x7961 declares dwc = 1 (small → passes the Cmd1-based capacity check);
    // Cmd2 0x797E declares dwc = 30 (the over-claim). R2: a valid record.
    let mut r1 = Vec::new();
    r1.extend_from_slice(&0x0A08u16.to_le_bytes()); // Type: wc=10, type=0x08 (RT_TO_RT)
    r1.extend_from_slice(&[0x0F, 0x18, 0x26, 0xDB, 0x21, 0xF6]); // IRIG ts (3 words)
    r1.extend_from_slice(&0x7961u16.to_le_bytes()); // Cmd1: RT15 R SA11 dwc=1
    r1.extend_from_slice(&0x797Eu16.to_le_bytes()); // Cmd2: RT15 R SA11 dwc=30 (over-claim)
    r1.extend_from_slice(&[0u8; 2]); // tx_status
    r1.extend_from_slice(&[0u8; 6]); // 3 padding words → total 10 words = 20 bytes
    assert_eq!(r1.len(), 20);

    let mut bytes = r1.clone();
    bytes.extend_from_slice(&record_rt15_sa11_rcv());
    let f = TempFile::new(&bytes);

    // Strict: extraction completes without a panic/overrun (bounded reads),
    // then L2-SYN-027 rejects the Cmd1/Cmd2 mismatch.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            strict: true,
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    match reader.iter().next() {
        Some(Err(e)) => assert_eq!(e.kind(), MieErrorKind::PayloadError),
        other => panic!("expected Some(Err(PayloadError)), got {other:?}"),
    }

    // Lenient: R1 is skipped; R2 decodes intact at its true offset — proving
    // R1's Cmd2 over-claim consumed nothing beyond its 20-byte declared extent.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1, "only the valid R2 survives");
    assert_eq!(msgs[0].file_offset, 20);
    assert_eq!(msgs[0].command_word.unwrap().rt, 15);
}

/// L2-SYN-027: an RT-to-RT record whose Cmd1 and Cmd2 disagree on
/// `data_word_count` is rejected end-to-end — even when the record is large
/// enough that neither the L2-SYN-022 capacity check nor the record-bounded
/// reads would fire. Isolates the new invariant from the over-claim/bounds
/// path. Mirrors the Python `test_rt_to_rt_cmd_word_count_mismatch_rejected`.
/// Requirements: L2-SYN-027
#[test]
fn rt_to_rt_cmd_word_count_mismatch_rejected() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::models::TimestampFormat;
    use mie_decoder::reader::ReaderOptions;

    // R1: word_count = 13 (26 bytes), type 0x08. Cmd1 0x7963 (RT15 R SA11
    // dwc=3); Cmd2 0x7965 (RT15 R SA11 dwc=5, direction Receive so L2-SYN-023
    // passes). word_count=13 clears the Cmd1-based capacity minimum
    // (1+3+1+(3+3)=11) and holds Cmd2's full 5-word payload + rx_status, so
    // only the Cmd1/Cmd2 mismatch is at fault.
    let mut r1 = Vec::new();
    r1.extend_from_slice(&0x0D08u16.to_le_bytes()); // Type: wc=13, type=0x08 (RT_TO_RT)
    r1.extend_from_slice(&[0x0F, 0x18, 0x26, 0xDB, 0x21, 0xF6]); // IRIG ts (3 words)
    r1.extend_from_slice(&0x7963u16.to_le_bytes()); // Cmd1: RT15 R SA11 dwc=3
    r1.extend_from_slice(&0x7965u16.to_le_bytes()); // Cmd2: RT15 R SA11 dwc=5
    r1.extend_from_slice(&[0u8; 2]); // tx_status
    r1.extend_from_slice(&[0u8; 10]); // 5 data words
    r1.extend_from_slice(&[0u8; 2]); // rx_status → total 13 words = 26 bytes
    assert_eq!(r1.len(), 26);

    let mut bytes = r1.clone();
    bytes.extend_from_slice(&record_rt15_sa11_rcv());
    let f = TempFile::new(&bytes);

    // Strict rejects the mismatch.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            strict: true,
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    match reader.iter().next() {
        Some(Err(e)) => assert_eq!(e.kind(), MieErrorKind::PayloadError),
        other => panic!("expected Some(Err(PayloadError)), got {other:?}"),
    }

    // Lenient skips R1; only the valid R2 survives, at offset 26.
    let reader = MieFileReader::with_options(
        f.path(),
        ReaderOptions {
            time_format: TimestampFormat::Irig,
            ..Default::default()
        },
    )
    .unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1, "only the valid R2 survives");
    assert_eq!(msgs[0].file_offset, 26);
    assert_eq!(msgs[0].command_word.unwrap().rt, 15);
}

/// Requirements: L2-RDR-009
#[test]
fn delta_tracker_per_rt_msg_key() {
    let mut bytes = Vec::new();
    bytes.extend(record_rt15_sa11_rcv()); // RT15 SA11 R
    bytes.extend(record_rt15_sa11_rcv()); // RT15 SA11 R again — should yield non-zero delta
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].delta, Some(0.0));
    // Same timestamp in both fixtures → delta should be exactly 0.0 (not negative)
    assert_eq!(msgs[1].delta, Some(0.0));
}

/// Requirements: L2-FLT-001
#[test]
fn filtering_drops_excluded_rts() {
    let mut bytes = Vec::new();
    bytes.extend(record_rt15_sa11_rcv());
    bytes.extend(record_rt15_sa22_xmt());
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();

    let cfg = FilterConfig {
        exclude_subaddresses: vec![11],
        ..Default::default()
    };
    let msgs: Vec<_> = reader
        .iter()
        .filter_messages(cfg)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].command_word.unwrap().subaddress, 22);
}

/// Requirements: L2-WRT-001
#[test]
fn csv_output_has_one_row_per_message_plus_header() {
    let mut bytes = Vec::new();
    bytes.extend(record_rt15_sa11_rcv());
    bytes.extend(record_rt15_sa22_rcv());
    bytes.extend(record_rt15_sa22_xmt());
    let f = TempFile::new(&bytes);

    let out_path = std::env::temp_dir().join(format!("mie-int-out-{}.csv", std::process::id()));
    let reader = MieFileReader::new(f.path()).unwrap();
    let n = write_csv(reader.iter(), Some(&out_path), Default::default())
        .unwrap()
        .normal_count;
    assert_eq!(n, 3);

    let csv = std::fs::read_to_string(&out_path).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 4); // 1 header + 3 data
    assert!(lines[0].starts_with("TIME_STAMP,RT,MSG"));
    // Each data line begins with timestamp + RT 15
    for row in &lines[1..] {
        assert!(row.contains(",15,"), "row missing RT15: {row}");
    }
    let _ = std::fs::remove_file(&out_path);
}

/// Requirements: L2-SYN-015
#[test]
fn corrupt_irig_record_skipped_by_per_record_validation() {
    // Regression test for the validation-parity fix: a record that
    // passes the coarse 3-check filter (valid type, valid word_count,
    // fits in file) but has an out-of-range IRIG hour (31 > 23) must
    // be rejected by per-record validation and skipped via sync
    // recovery — not emitted as a garbage row.
    let mut corrupt = record_rt15_sa11_rcv();
    // Byte 2 is the low byte of the IRIG upper word (LE). The hour
    // field is bits 0..4 of that word. Setting it to 0x1F makes
    // hour = 31, which violates `hour < 24`. The two-record look-ahead
    // would still pass (the next record is valid), so the IRIG range
    // check is the sole discriminator here.
    corrupt[2] = (corrupt[2] & 0xE0) | 0x1F;

    let mut bytes = Vec::new();
    bytes.extend(corrupt); // corrupt-IRIG record (offset 0)
    bytes.extend(record_rt15_sa11_rcv()); // valid record (offset 72)
    let f = TempFile::new(&bytes);

    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();

    // The corrupt record should be dropped via sync recovery; only the
    // valid one is emitted.
    assert_eq!(msgs.len(), 1, "expected corrupt-IRIG record to be skipped");
    assert_eq!(
        msgs[0].file_offset, 72,
        "expected the valid record at offset 72"
    );
}

/// Requirements: L2-SYN-022
#[test]
fn payload_capacity_mismatch_skipped_in_lenient_mode() {
    // Originally a regression test for payload-extraction overrun: a
    // record whose Type Word claims wc=5 but whose Command Word
    // declares data_word_count=30 used to let extract_payload consume
    // bytes from the next record. The extract_payload bounding (Phase
    // 2-era) plus the new L2-SYN-022 capacity check (Phase 7a)
    // both defend against this. The capacity check now fires first:
    // in lenient mode the bad record is logged and skipped before
    // extract_payload runs.
    //
    // This test pins the lenient-mode behavior end-to-end. The strict
    // case is covered by a per-impl unit test and a conformance
    // fixture.
    let mut record_a = Vec::with_capacity(10);
    record_a.extend_from_slice(&0x0502u16.to_le_bytes()); // TW: type 0x02, wc=5
    record_a.extend_from_slice(&0x002Au16.to_le_bytes()); // IRIG upper (day=1, hour=10)
    record_a.extend_from_slice(&0x51E0u16.to_le_bytes()); // IRIG middle
    record_a.extend_from_slice(&0u16.to_le_bytes()); // IRIG lower
    record_a.extend_from_slice(&0x283Eu16.to_le_bytes()); // Cmd: rt=5 R sa=1 dwc=30
    assert_eq!(record_a.len(), 10);

    let mut bytes = Vec::new();
    bytes.extend(&record_a);
    bytes.extend(record_rt15_sa11_rcv()); // Record B at offset 10
    assert_eq!(bytes.len(), 82);

    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();

    // Record A is rejected by L2-SYN-022 (wc=5 < 1+3+1+31=36).
    // Lenient mode WARN+skips it and continues. Only Record B emits.
    assert_eq!(msgs.len(), 1);
    let m = &msgs[0];
    assert_eq!(m.file_offset, 10);
    assert_eq!(m.command_word.unwrap().rt, 15);
    assert_eq!(m.command_word.unwrap().subaddress, 11);
    assert_eq!(m.data_words.len(), 30);
    assert_eq!(m.status_word, Some(0x7800));
}

/// Requirements: L2-SYN-011, L1-EXIT-002
#[test]
fn non_mie_file_surfaces_error_not_silent_zero_messages() {
    // Regression test for the team's "Cargo.toml" reproducer: passing a
    // non-MIE file (this fixture mimics a TOML manifest) used to silently
    // produce zero messages and exit successfully. The fix surfaces a
    // NoValidRecords error from the iterator so `count` and `decode`
    // return non-zero exit codes and tell the user what went wrong.
    let toml = b"[package]\nname = \"mie-decoder\"\nversion = \"1.0.0\"\nedition = \"2024\"\n\n[dependencies]\nmemmap2 = \"0.9\"\n";
    // Pad with 0xFF so the rest of the file can't coincidentally form
    // a valid Type Word (low byte 0xFF & 0x7F = 0x7F, not in the
    // valid type set). Padding with spaces would not work — pairs of
    // 0x20 0x20 happen to parse as valid SPURIOUS_DATA Type Words with
    // word_count=32, which is a real surprise about how permissive the
    // 5-check heuristic is on highly regular inputs.
    let mut bytes = toml.to_vec();
    bytes.resize(1024, 0xFF);
    let f = TempFile::new(&bytes);

    let reader = MieFileReader::new(f.path()).unwrap();
    let collected: Result<Vec<_>, _> = reader.iter().collect();

    match collected {
        Err(e) => {
            assert!(
                e.to_string().contains("No valid MIE records"),
                "expected NoValidRecords-shaped error, got: {e}"
            );
        }
        Ok(msgs) => panic!(
            "expected an error on a non-MIE file, but got {} message(s)",
            msgs.len()
        ),
    }
}

/// Requirements: L2-SYN-006
#[test]
fn header_skip_via_proprietary_prefix() {
    let mut bytes = Vec::with_capacity(32 + 72);
    bytes.extend_from_slice(b"DDC-EQUIPMENT-NAME\0\0PADD\0\0\0\0\0\0"); // 28 bytes — 14 words
    let header_len = bytes.len();
    bytes.extend(record_rt15_sa11_rcv());
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].file_offset, header_len as u64);
}

// ── L1-ROB-001 fuzz harness ──────────────────────────────────────────────
//
// Deterministic xorshift64 PRNG keeps the test fully reproducible and
// avoids pulling in `rand` (the crate stays at a single external dep,
// per L3-RS-002). Every iteration:
//   1. generates a random byte sequence (32 B - 8 KB),
//   2. writes it to a temp file,
//   3. opens it with MieFileReader,
//   4. iterates to completion,
// and asserts that ANY outcome other than a panic is acceptable —
// `Err(MieError::*)` items are the *expected* response to random
// bytes; we just need to confirm we never panic, segfault, or enter
// an unbounded loop. With 2 KiB iterations × up to 8 KB inputs the
// total throughput is ~16 MB of random bytes; comfortably fits in a
// test budget.
//
// The PRNG seed is hard-coded so a failure can be reproduced exactly
// (and locked in via the panic-printed seed when triaging).
/// Requirements: L1-ROB-001
#[test]
fn fuzz_arbitrary_bytes_never_panic() {
    fn xorshift64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    let seed: u64 = 0x0DDCD1ECDDC0DEC0;
    let mut state = seed;
    // 256 iterations runs in a few seconds and consistently exercises
    // every invariant + recovery branch (verified by inspecting the
    // WARN/ERROR log stream). The scheduled CI fuzz job overrides this
    // via the MIE_FUZZ_ITERATIONS env var for a longer burn-in; the
    // default-suite cost stays bounded. The PRNG is deterministic, so a
    // burn-in is a strict superset of the default run (same first 256).
    let iterations: usize = std::env::var("MIE_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(256);

    for i in 0..iterations {
        // Sizes range from 32 B (slightly above MIN_RECORD_BYTES_STANDARD)
        // to ~8 KB. The lower bound keeps record headers reachable; the
        // upper bound keeps each iteration fast.
        let size = 32 + (xorshift64(&mut state) as usize % 8192);
        let mut bytes = vec![0u8; size];
        let mut j = 0;
        while j + 8 <= size {
            let r = xorshift64(&mut state);
            bytes[j..j + 8].copy_from_slice(&r.to_le_bytes());
            j += 8;
        }
        // Fill any tail.
        while j < size {
            bytes[j] = (xorshift64(&mut state) & 0xFF) as u8;
            j += 1;
        }

        let f = TempFile::new(&bytes);

        // Use catch_unwind so an unexpected panic is surfaced with the
        // reproducer seed instead of bringing down the whole test
        // process at the first failure.
        let result = std::panic::catch_unwind(|| {
            // Reader construction itself may fail on FileEmpty etc. —
            // that's a documented error path, not a panic.
            if let Ok(reader) = MieFileReader::new(f.path()) {
                // Cap iteration count as a defense-in-depth bound:
                // if the iterator somehow enters an unbounded loop,
                // this surfaces it as a failed assertion rather than
                // hanging the test runner.
                let mut yielded = 0u64;
                for item in reader.iter() {
                    // We accept any Result; we just must not panic.
                    let _ = item;
                    yielded += 1;
                    assert!(
                        yielded < 100_000,
                        "iterator yielded over 100k items on a {size}-byte input — \
                         possible unbounded loop (seed=0x{seed:X}, iter={i})"
                    );
                }
            }
        });

        if result.is_err() {
            panic!(
                "MieFileReader panicked on random input (seed=0x{seed:X}, iter={i}, \
                 size={size}). First 32 bytes: {:02X?}",
                &bytes[..bytes.len().min(32)]
            );
        }
    }
}

/// L1-ROB-001 for the `dump` subcommand: the record-aware and raw hex dumps
/// must tolerate arbitrary bytes without panicking. The record dump's header
/// reads use `read_u16(...).unwrap_or(0)`, a `checked_add` loop guard, and
/// slice to the record extent for the body — it never reads payload by a
/// Command Word's `data_word_count`, so it has no over-claim/overrun class
/// like the reader's `extract_payload`. This test guards that property
/// against regression. Sizes are skewed small to exercise the truncation /
/// loop-guard paths densely. Mirrors the Python
/// `test_dump_arbitrary_bytes_never_raise_unexpected_exceptions`.
/// Requirements: L1-ROB-001, L2-CLI-009
#[test]
fn dump_arbitrary_bytes_never_panics() {
    fn xorshift64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    let seed: u64 = 0x0DDCD1ECDDC0DEC0; // same seed family as the reader harness
    let mut state = seed;

    // Honor MIE_FUZZ_ITERATIONS for the scheduled burn-in, same as the reader
    // harness (deterministic PRNG, so a burn-in is a superset of the default).
    let iterations: usize = std::env::var("MIE_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(256);

    for i in 0..iterations {
        let size = xorshift64(&mut state) as usize % 512; // small band → dense guard coverage
        let mut bytes = vec![0u8; size];
        let mut j = 0;
        while j + 8 <= size {
            let r = xorshift64(&mut state);
            bytes[j..j + 8].copy_from_slice(&r.to_le_bytes());
            j += 8;
        }
        while j < size {
            bytes[j] = (xorshift64(&mut state) & 0xFF) as u8;
            j += 1;
        }

        let f = TempFile::new(&bytes);
        let result = std::panic::catch_unwind(|| {
            // Both dumps may return Err (e.g. FileEmpty) — a documented error
            // path, not a panic. We sink output into a Vec and discard it.
            let mut sink = Vec::new();
            let _ = mie_decoder::dump::hex_dump_records(f.path(), Some(64), 0, &mut sink);
            sink.clear();
            let _ = mie_decoder::dump::hex_dump_raw(f.path(), 0, None, &mut sink);
        });

        if result.is_err() {
            panic!(
                "dump panicked on random input (seed=0x{seed:X}, iter={i}, size={size}). \
                 First 32 bytes: {:02X?}",
                &bytes[..bytes.len().min(32)]
            );
        }
    }
}

// ── L1-MRG / L2-MRG: multi-file time-sorted merge ─────────────────────────

/// An RT15 SA11 Receive record placed at a chosen IRIG instant, by patching
/// the timestamp triple of the canonical fixture (bytes 2..8 = the three IRIG
/// timestamp words). Lets merge tests position records at specific times.
fn rt15_record_at(
    day: u16,
    hour: u8,
    minute: u8,
    second: u8,
    micro: u32,
    freerun: bool,
) -> Vec<u8> {
    let mut rec = record_rt15_sa11_rcv();
    let fr = u16::from(freerun) << 15;
    let upper: u16 = fr | ((day & 0x1FF) << 5) | u16::from(hour & 0x1F);
    let middle: u16 = (u16::from(minute & 0x3F) << 10)
        | (u16::from(second & 0x3F) << 4)
        | ((micro >> 16) as u16 & 0xF);
    let lower: u16 = (micro & 0xFFFF) as u16;
    rec[2..4].copy_from_slice(&upper.to_le_bytes());
    rec[4..6].copy_from_slice(&middle.to_le_bytes());
    rec[6..8].copy_from_slice(&lower.to_le_bytes());
    rec
}

/// Requirements: L1-MRG-001, L2-MRG-002, L2-MRG-005
#[test]
fn merge_orders_records_across_files_by_absolute_time() {
    use mie_decoder::merge::MergedRecordIter;

    // File A: t=100µs, 300µs. File B: t=200µs, 400µs. Same day/h/m/s so the
    // microsecond field is the discriminator; merged order must interleave.
    let a = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 300, false),
    ]
    .concat();
    let b = [
        rt15_record_at(192, 15, 54, 50, 200, false),
        rt15_record_at(192, 15, 54, 50, 400, false),
    ]
    .concat();
    let fa = TempFile::new(&a);
    let fb = TempFile::new(&b);
    let readers = vec![
        MieFileReader::new(fa.path()).unwrap(),
        MieFileReader::new(fb.path()).unwrap(),
    ];

    let merged = MergedRecordIter::new(&readers, None, false, false).unwrap();
    let msgs: Vec<_> = merged.collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 4, "all four records survive the merge");

    // Absolute microseconds include the day/hour/min/sec base; the proof of a
    // correct interleave (A:100,300 + B:200,400 → 100,200,300,400) is that the
    // merged keys are strictly increasing.
    let us: Vec<u64> = msgs
        .iter()
        .map(|m| m.timestamp.to_microseconds(None).unwrap())
        .collect();
    assert!(
        us.windows(2).all(|w| w[0] < w[1]),
        "merged stream is not strictly time-ordered: {us:?}"
    );

    // Global DELTA (L2-MRG-005): first occurrence 0.0, then non-negative gaps
    // on the unified timeline (all four share one RT/SA/dir key).
    assert_eq!(msgs[0].delta, Some(0.0));
    for m in &msgs[1..] {
        assert!(m.delta.unwrap() >= 0.0);
    }
}

/// Requirements: L2-MRG-001
#[test]
fn merge_single_input_is_unchanged() {
    use mie_decoder::merge::MergedRecordIter;
    // A one-file "merge" yields exactly the file's records, in order.
    let a = [
        rt15_record_at(192, 15, 54, 50, 10, false),
        rt15_record_at(192, 15, 54, 50, 20, false),
    ]
    .concat();
    let fa = TempFile::new(&a);
    let readers = vec![MieFileReader::new(fa.path()).unwrap()];
    let merged = MergedRecordIter::new(&readers, None, false, false).unwrap();
    let msgs: Vec<_> = merged.collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 2);
}

/// Requirements: L1-MRG-002, L2-MRG-003
#[test]
fn merge_rejects_freerun_leading_input() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::merge::MergedRecordIter;

    let good = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 300, false),
    ]
    .concat();
    // Leading record carries the freerun bit → no calendar time.
    let freerun = [
        rt15_record_at(0, 0, 0, 0, 0, true),
        rt15_record_at(0, 0, 0, 1, 0, true),
    ]
    .concat();
    let fa = TempFile::new(&good);
    let fb = TempFile::new(&freerun);
    let readers = vec![
        MieFileReader::new(fa.path()).unwrap(),
        MieFileReader::new(fb.path()).unwrap(),
    ];
    match MergedRecordIter::new(&readers, None, false, false) {
        Err(e) => assert_eq!(e.kind(), MieErrorKind::IncompatibleMergeInputs),
        Ok(_) => panic!("expected IncompatibleMergeInputs for a freerun-leading input"),
    }
}

/// Requirements: L1-MRG-002, L2-MRG-003
#[test]
fn merge_rejects_standard_format_input() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::merge::MergedRecordIter;
    use mie_decoder::models::TimestampFormat;
    use mie_decoder::reader::ReaderOptions;

    let a = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 300, false),
    ]
    .concat();
    let fa = TempFile::new(&a);
    // Forcing the Standard timestamp format makes the records decode as
    // Standard timestamps, which carry no shared epoch → not mergeable.
    let readers = vec![
        MieFileReader::with_options(
            fa.path(),
            ReaderOptions {
                time_format: TimestampFormat::Standard,
                ..Default::default()
            },
        )
        .unwrap(),
        MieFileReader::with_options(
            fa.path(),
            ReaderOptions {
                time_format: TimestampFormat::Standard,
                ..Default::default()
            },
        )
        .unwrap(),
    ];
    match MergedRecordIter::new(&readers, None, false, false) {
        Err(e) => assert_eq!(e.kind(), MieErrorKind::IncompatibleMergeInputs),
        Ok(_) => panic!("expected IncompatibleMergeInputs for a Standard-format input"),
    }
}

/// Requirements: L2-MRG-001
#[test]
fn read_manifest_skips_blanks_and_comments() {
    let body = "# a comment\n\nfile1.mie\n  file2.mie  \n# another\nfile3.mie\n";
    let f = TempFile::new(body.as_bytes());
    let paths = mie_decoder::merge::read_manifest(f.path()).unwrap();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("file1.mie"),
            PathBuf::from("file2.mie"),
            PathBuf::from("file3.mie"),
        ]
    );
}

/// L1-ROB-001 for the merge input-resolution surface: a manifest of arbitrary
/// bytes, an arbitrary glob pattern, and arbitrary glob_match inputs must
/// never panic — only return Ok/Err (or a bool). Deterministic PRNG.
/// Requirements: L1-ROB-001, L2-MRG-001
#[test]
fn merge_input_resolution_tolerates_arbitrary_bytes() {
    fn xorshift64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
    let mut state = 0x0DDCD1ECDDC0DEC0u64;
    for _ in 0..512 {
        let n = (xorshift64(&mut state) % 96) as usize;
        let bytes: Vec<u8> = (0..n)
            .map(|_| (xorshift64(&mut state) & 0xFF) as u8)
            .collect();
        let f = TempFile::new(&bytes);
        // read_manifest: Ok (parsed lines) or Err (non-UTF8) — never panic.
        let _ = mie_decoder::merge::read_manifest(f.path());
        // Treat the bytes (lossily) as a glob pattern: matcher + expansion
        // must not panic on any input.
        let pat = String::from_utf8_lossy(&bytes);
        let _ = mie_decoder::merge::glob_match(&pat, "some.name.mie");
        let _ = mie_decoder::merge::expand_glob(&pat);
    }
}

/// L2-MRG-004 / L1-EXIT-004: with --allow-partial, a merge whose input hits an
/// unrecoverable sync loss truncates that file, completes from the rest, and
/// the writer commits the combined output as `.partial`. (Forcing an
/// unrecoverable loss needs >64 KB of non-resyncing garbage, so this is a
/// library test rather than a small conformance hex fixture.)
/// Requirements: L2-MRG-004, L1-EXIT-004
#[test]
fn merge_allow_partial_writes_partial_on_file_failure() {
    use mie_decoder::merge::MergedRecordIter;
    use mie_decoder::writer::{WriteOptions, write_csv};

    // File A: good records at 100µs, 300µs. File B: good records at 200µs,
    // 400µs, then 70 KB of 0xFF → recover_sync exhausts the 64 KB window
    // (unrecoverable) after B yields its first record.
    let a = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 300, false),
    ]
    .concat();
    let mut b = [
        rt15_record_at(192, 15, 54, 50, 200, false),
        rt15_record_at(192, 15, 54, 50, 400, false),
    ]
    .concat();
    b.extend(vec![0xFFu8; 70_000]);
    let fa = TempFile::new(&a);
    let fb = TempFile::new(&b);
    let readers = vec![
        MieFileReader::new(fa.path()).unwrap(),
        MieFileReader::new(fb.path()).unwrap(),
    ];

    let merged = MergedRecordIter::new(&readers, None, true, false).unwrap();
    let out = TempFile::new(b"");
    let opts = WriteOptions {
        input_path: None,
        no_clobber: false,
        allow_partial: true,
    };
    let outcome = write_csv(merged, Some(out.path()), opts).unwrap();
    assert!(
        outcome.partial.is_some(),
        "--allow-partial should commit a .partial on the file failure"
    );
    // A's 100 + B's 200 + A's 300 reached the writer before B's terminal loss.
    assert_eq!(outcome.normal_count, 3);
    let partial = std::path::PathBuf::from(format!("{}.partial", out.path().display()));
    assert!(partial.exists(), "the .partial output file should exist");
    let _ = std::fs::remove_file(&partial);
}

/// L2-MRG-006: an input whose records step backward in time (not internally
/// time-sorted) is a data-quality anomaly. In lenient mode the merge WARNs and
/// still emits every record (never re-sorts), so all records survive.
/// Requirements: L2-MRG-006
#[test]
fn merge_warns_on_within_file_backward_step() {
    use mie_decoder::merge::MergedRecordIter;

    // One file whose microsecond keys step 100 → 200 → 150 (the third record
    // is older than the second): a within-file backward step.
    let a = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 200, false),
        rt15_record_at(192, 15, 54, 50, 150, false),
    ]
    .concat();
    let fa = TempFile::new(&a);
    let readers = vec![MieFileReader::new(fa.path()).unwrap()];

    let merged = MergedRecordIter::new(&readers, None, false, false).unwrap();
    let msgs: Vec<_> = merged.collect::<Result<_, _>>().unwrap();
    // Lenient: advisory only — every record is still emitted (no failure).
    assert_eq!(
        msgs.len(),
        3,
        "lenient mode keeps all records despite the WARN"
    );
}

/// L2-MRG-006: in strict mode the same within-file backward step is a record
/// error that fails the batch (exit-1 class), mirroring how strict already
/// treats per-record / structural-invariant failures.
/// Requirements: L2-MRG-006
#[test]
fn merge_strict_fails_on_within_file_backward_step() {
    use mie_decoder::error::MieErrorKind;
    use mie_decoder::merge::MergedRecordIter;

    let a = [
        rt15_record_at(192, 15, 54, 50, 100, false),
        rt15_record_at(192, 15, 54, 50, 200, false),
        rt15_record_at(192, 15, 54, 50, 150, false),
    ]
    .concat();
    let fa = TempFile::new(&a);
    let readers = vec![MieFileReader::new(fa.path()).unwrap()];

    // strict = true. The first two records pop cleanly; pulling the backward
    // third record arms a pending error that surfaces as a terminal Err.
    let merged = MergedRecordIter::new(&readers, None, false, true).unwrap();
    let mut saw_err = false;
    for item in merged {
        if let Err(e) = item {
            assert_eq!(e.kind(), MieErrorKind::NonMonotonicInput);
            saw_err = true;
            break;
        }
    }
    assert!(
        saw_err,
        "strict mode should surface a NonMonotonicInput error"
    );
}
