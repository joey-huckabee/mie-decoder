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

#[test]
fn lenient_mode_unrecoverable_sync_loss_yields_terminal_error() {
    // L1-023 lenient-mode contract: when sync recovery exhausts within
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
    // reported. (Reader's getter is now exposed for the CLI's L1-024
    // exit-class summary.)
    assert!(reader.sync_losses() >= 1);
}

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

#[test]
fn payload_extraction_does_not_overrun_into_next_record() {
    // Regression test: a record whose Type Word claims a small word
    // count but whose Command Word claims a large data_word_count must
    // not cause payload extraction to read past the record boundary
    // into the next record. Before the fix, extract_payload was passed
    // the whole file slice and would happily consume bytes from the
    // following record as "data words", producing a garbage CSV row.
    //
    // Record A: wc=5 (10 bytes total: Type + 3 IRIG TS + Cmd, no data,
    //   no status), Cmd claims data_word_count=30.
    // Record B: a clean rt15_sa11_rcv (72 bytes).
    //
    // With the bug, Record A's CSV row would contain 30 "data words"
    // sourced from Record B's bytes. With the fix, payload extraction
    // is bounded to Record A's 10 bytes and returns 0 data words.
    let mut record_a = Vec::with_capacity(10);
    // Type Word LE: type=0x02, bus=A, wc=5, error=0  →  0x0502
    record_a.extend_from_slice(&0x0502u16.to_le_bytes());
    // IRIG TS upper: hour=10, day=0, freerun=0  →  0x000A
    record_a.extend_from_slice(&0x000Au16.to_le_bytes());
    // IRIG TS middle: minute=20<<10 | second=30<<4 | us_hi=0  →  0x51E0
    record_a.extend_from_slice(&0x51E0u16.to_le_bytes());
    // IRIG TS lower (microsecond low): 0
    record_a.extend_from_slice(&0u16.to_le_bytes());
    // Command Word: rt=5<<11 | dir=Recv=0 | sa=1<<5 | dwc=30  →  0x283E
    record_a.extend_from_slice(&0x283Eu16.to_le_bytes());
    assert_eq!(record_a.len(), 10);

    let mut bytes = Vec::new();
    bytes.extend(&record_a);
    bytes.extend(record_rt15_sa11_rcv());
    assert_eq!(bytes.len(), 82);

    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();

    // Both records pass validation (Record A has all five 5-check
    // heuristics satisfied: valid type, plausible wc, fits in file,
    // valid IRIG fields, valid look-ahead at offset 10).
    assert_eq!(msgs.len(), 2);

    // Record A: command word fields decoded, but no payload bytes
    // exist within the record's 10-byte budget. The fix should yield
    // ZERO data words; the bug would have yielded up to 30 from
    // Record B's bytes.
    let m0 = &msgs[0];
    assert_eq!(m0.file_offset, 0);
    assert_eq!(m0.command_word.unwrap().rt, 5);
    assert_eq!(m0.command_word.unwrap().subaddress, 1);
    assert_eq!(m0.command_word.unwrap().data_word_count, 30);
    assert_eq!(
        m0.data_words.len(),
        0,
        "payload extraction leaked from next record: data_words={:?}",
        m0.data_words.as_slice()
    );
    assert_eq!(m0.status_word, None);

    // Record B: still decodes correctly — the bug would have left it
    // intact on its own (it's the leak SOURCE, not victim).
    let m1 = &msgs[1];
    assert_eq!(m1.file_offset, 10);
    assert_eq!(m1.command_word.unwrap().rt, 15);
    assert_eq!(m1.command_word.unwrap().subaddress, 11);
    assert_eq!(m1.data_words.len(), 30);
    assert_eq!(m1.status_word, Some(0x7800));
}

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
