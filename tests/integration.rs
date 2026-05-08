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

// ── Fixtures (byte-exact from python-reference/tests/conftest.py) ─────

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
fn delta_tracker_per_rt_msg_key() {
    let mut bytes = Vec::new();
    bytes.extend(record_rt15_sa11_rcv()); // RT15 SA11 R
    bytes.extend(record_rt15_sa11_rcv()); // RT15 SA11 R again — should yield non-zero delta
    let f = TempFile::new(&bytes);
    let reader = MieFileReader::new(f.path()).unwrap();
    let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].delta, 0.0);
    // Same timestamp in both fixtures → delta should be exactly 0.0 (not negative)
    assert_eq!(msgs[1].delta, 0.0);
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
    let n = write_csv(reader.iter(), Some(&out_path)).unwrap();
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
