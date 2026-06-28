//! CLI acceptance tests: spawn the actual built `mie-decoder` binary
//! as a subprocess and assert on exit code, stdout, stderr, and
//! filesystem effects.
//!
//! Sits one level above the cross-implementation conformance suite
//! (`tests/conformance/`). Conformance proves the Rust and Python
//! CLIs produce byte-identical CSV; this file covers Rust-only CLI
//! behaviors that conformance can't assert (`--no-clobber`,
//! input/output collision rejection, exit-class taxonomy, `--help` /
//! `--version`) plus a smoke-level happy-path decode to confirm the
//! binary is wired together end-to-end.
//!
//! Runs on every platform `cargo test --all-targets` runs on — Cargo
//! exposes the built binary path via `env!("CARGO_BIN_EXE_<name>")`
//! and appends `.exe` on Windows automatically, so no per-OS code
//! paths are needed here.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_mie-decoder");

// ── Fixtures ─────────────────────────────────────────────────────────

/// One valid 72-byte RT15 SA11 receive record. Byte-exact with the
/// `record_rt15_sa11_rcv` fixture in `rust/tests/integration.rs` —
/// duplicated here so the CLI suite has no link-time dependency on
/// the integration suite.
fn one_valid_record() -> Vec<u8> {
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

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// One errored RT15 SA11 record: Type Word with bit 14 set (error),
/// IRIG timestamp byte-identical with `one_valid_record`, Cmd Word
/// 0x797E, two zero data words, and a trailing Error Word of 0x011E
/// (Manchester/Parity). 16 bytes total. Mirrors the
/// `errored_record_rt15_sa11_us(...)` builder in
/// `python/tests/conftest.py`.
fn errored_record() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("02480F1826DB21F6"); // Type 0x4802 (err bit, wc=8) + IRIG TS
    s.push_str("7E79"); // Cmd Word 0x797E
    s.push_str("00000000"); // 2 zero data words
    s.push_str("1E01"); // Error Word 0x011E
    hex(&s)
}

/// `one_valid_record` with the IRIG freerun bit (bit 15 of the upper
/// timestamp word) set — a record with no calendar anchor. A merge rejects a
/// freerun-leading input because it can't share an absolute timeline.
fn freerun_record() -> Vec<u8> {
    let mut r = one_valid_record();
    r[3] |= 0x80; // set bit 15 of the little-endian upper timestamp word
    r
}

// ── Scratch directory helper ─────────────────────────────────────────

/// Per-test scratch directory, removed on drop. Tests work inside one
/// of these instead of inventing unique names per artifact — paths
/// can be plain `dir/input.mie`, `dir/output.csv`.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("mie-cli-{pid}-{n}"));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        p
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── Subprocess helper ────────────────────────────────────────────────

fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let out = Command::new(BIN)
        .args(args)
        .output()
        .expect("failed to spawn mie-decoder binary");
    if !out.stderr.is_empty() {
        // Surface stderr in test output so a Windows CI failure can
        // be triaged from the runner logs without re-running locally.
        eprintln!(
            "--- mie-decoder stderr ---\n{}\n--------------------------",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    out
}

fn exit_code(o: &Output) -> i32 {
    o.status
        .code()
        .expect("process exited via signal, not a code")
}

// ── Tests ────────────────────────────────────────────────────────────

/// Requirements: L2-CLI-001, L2-CLI-008, L2-CLI-009
#[test]
fn help_exits_zero_and_lists_all_subcommands() {
    let out = run(["--help"]);
    assert_eq!(exit_code(&out), 0, "--help must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["decode", "count", "dump"] {
        assert!(
            stdout.contains(sub),
            "--help output missing subcommand '{sub}'\n--- stdout ---\n{stdout}"
        );
    }
}

/// Requirements: L2-CLI-005
#[test]
fn version_prints_crate_version() {
    let out = run(["--version"]);
    assert_eq!(exit_code(&out), 0, "--version must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let expected = env!("CARGO_PKG_VERSION");
    assert!(
        stdout.contains(expected),
        "--version output missing crate version '{expected}'\n--- stdout ---\n{stdout}"
    );
}

/// Requirements: L2-CLI-001, L2-CLI-002, L2-WRT-001
#[test]
fn decode_happy_path_writes_csv_with_header_and_one_row() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0, "decode happy path must exit 0");

    let csv = std::fs::read_to_string(&output).expect("output CSV not created");
    // Don't assert the full header byte-for-byte — that's the
    // conformance suite's job. Just confirm the writer ran and
    // produced a header line plus the one data row.
    assert!(
        csv.contains("MSG"),
        "CSV missing MSG header column\n--- csv ---\n{csv}"
    );
    assert!(
        csv.lines().count() >= 2,
        "CSV has fewer than 2 lines (header + data)\n--- csv ---\n{csv}"
    );
}

/// A multi-file merge whose inputs can't share an absolute timeline (a
/// freerun-leading file here) is rejected before any output, exit 6.
/// Requirements: L1-EXIT-009, L2-MRG-003, L2-CLI-011
#[test]
fn merge_incompatible_inputs_exit_6() {
    let tmp = TempDir::new();
    let mut good = one_valid_record();
    good.extend(one_valid_record());
    let mut freerun = freerun_record();
    freerun.extend(freerun_record());
    let g = tmp.write("good.mie", &good);
    let f = tmp.write("freerun.mie", &freerun);
    let output = tmp.path().join("merged.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        g.as_os_str(),
        f.as_os_str(),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(
        exit_code(&out),
        6,
        "incompatible (freerun) merge inputs must exit 6"
    );
    assert!(
        !output.exists(),
        "no output file should be created when the merge is rejected"
    );
}

/// Requirements: L2-MRG-004
///
/// A merge `decode` where one input fails at *priming* (a non-MIE first record)
/// under `--allow-partial` writes the combined output as `<out>.partial`, leaves
/// the plain `<out>` absent, and exits 0. Regression for the reported symptom
/// (pre-fix: a plain `out.csv` + exit 0, no `.partial`).
#[test]
fn merge_allow_partial_priming_writes_dot_partial() {
    let tmp = TempDir::new();
    let mut good = one_valid_record();
    good.extend(one_valid_record());
    let g = tmp.write("good.mie", &good);
    let b = tmp.write("bad.mie", &vec![0xFFu8; 4096]); // non-MIE first record
    let output = tmp.path().join("merged.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        g.as_os_str(),
        b.as_os_str(),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
        std::ffi::OsStr::new("--allow-partial"),
    ]);
    assert_eq!(
        exit_code(&out),
        0,
        "--allow-partial downgrades the priming failure to exit 0"
    );
    let partial = PathBuf::from(format!("{}.partial", output.display()));
    assert!(
        partial.exists(),
        "the combined output must be committed as .partial"
    );
    assert!(!output.exists(), "the plain output must NOT be written");
}

/// Requirements: L2-MRG-004
///
/// A merge where one input fails at *open* (an empty 0-byte file) under
/// `--allow-partial` likewise writes a `.partial` and exits 0 — the per-file
/// failure is tolerated whether it occurs at open, priming, or mid-file.
#[test]
fn merge_allow_partial_open_failure_writes_dot_partial() {
    let tmp = TempDir::new();
    let mut good = one_valid_record();
    good.extend(one_valid_record());
    let g = tmp.write("good.mie", &good);
    let e = tmp.write("empty.mie", b""); // 0-byte → fails at open
    let output = tmp.path().join("merged.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        g.as_os_str(),
        e.as_os_str(),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
        std::ffi::OsStr::new("--allow-partial"),
    ]);
    assert_eq!(exit_code(&out), 0);
    let partial = PathBuf::from(format!("{}.partial", output.display()));
    assert!(
        partial.exists(),
        "an open-failure merge must commit a .partial"
    );
    assert!(!output.exists());
}

/// Requirements: L2-WRT-014
#[test]
fn no_clobber_refuses_to_overwrite_existing_output() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let sentinel = b"SENTINEL CONTENT FROM PREVIOUS RUN\n";
    let output = tmp.write("out.csv", sentinel);

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--no-clobber"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_ne!(
        exit_code(&out),
        0,
        "--no-clobber must NOT exit 0 when target already exists"
    );

    let preserved = std::fs::read(&output).expect("output file disappeared");
    assert_eq!(
        preserved, sentinel,
        "--no-clobber must not modify the existing file"
    );
}

/// Requirements: L2-WRT-016
#[test]
fn rejects_input_equal_to_output_path() {
    let tmp = TempDir::new();
    let original = one_valid_record();
    let same = tmp.write("rec.mie", &original);

    let out = run([
        std::ffi::OsStr::new("decode"),
        same.as_os_str(),
        std::ffi::OsStr::new("-o"),
        same.as_os_str(),
    ]);
    assert_ne!(
        exit_code(&out),
        0,
        "decode must reject identical input and output paths"
    );

    // Belt-and-braces: even if the collision check were to fail
    // open, the input file must still be intact. Operator data
    // doesn't get destroyed by a CLI typo.
    let preserved = std::fs::read(&same).expect("input file disappeared");
    assert_eq!(
        preserved, original,
        "input file was modified despite the collision rejection"
    );
}

/// Requirements: L2-CLI-008, L3-RS-008
///
/// Two-channel output contract:
/// - stdout: ONLY the integer record count (machine-parseable for
///   pipelines: `n=$(mie-decoder count rec.mie)`).
/// - stderr: human-readable status line including the input path
///   so an interactive operator still sees context. Always emitted
///   (not gated by --log-level).
#[test]
fn count_subcommand_emits_integer_to_stdout_and_status_to_stderr() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());

    let out = run([std::ffi::OsStr::new("count"), input.as_os_str()]);
    assert_eq!(exit_code(&out), 0, "count must exit 0 on valid input");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "1",
        "stdout must contain only the integer count (got: {stdout:?})"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("counted 1 messages"),
        "stderr must include the human-readable status line (got: {stderr:?})"
    );
}

/// Requirements: L2-CLI-005, L1-EXIT-007
#[test]
fn no_args_invocation_is_usage_error_exit_4() {
    // No subcommand is a usage error (L1-EXIT-007), not a no-valid-records
    // condition (L1-EXIT-002) — it must exit with the usage code 4, not
    // merely some non-zero code.
    let out = run(Vec::<&str>::new());
    assert_eq!(
        exit_code(&out),
        4,
        "invoking the binary with no subcommand must be a usage error (exit 4)"
    );
}

// ── Filter behavior (Rust-only include side per L3-RS-010) ───────────

/// Helper: count data rows in a CSV (lines minus the one-line header).
fn data_row_count(csv: &str) -> usize {
    csv.lines().count().saturating_sub(1)
}

/// Requirements: L2-FLT-001, L3-RS-010
#[test]
fn include_rts_filter_keeps_only_matching_records() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());

    // `--include-rts 15` matches the fixture's RT15: row retained.
    let kept_out = tmp.path().join("kept.csv");
    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--include-rts"),
        std::ffi::OsStr::new("15"),
        std::ffi::OsStr::new("-o"),
        kept_out.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0);
    let csv = std::fs::read_to_string(&kept_out).unwrap();
    assert_eq!(
        data_row_count(&csv),
        1,
        "RT15 record should be kept by --include-rts 15\n--- csv ---\n{csv}"
    );

    // `--include-rts 7` excludes RT15 (no match): zero data rows.
    let dropped_out = tmp.path().join("dropped.csv");
    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--include-rts"),
        std::ffi::OsStr::new("7"),
        std::ffi::OsStr::new("-o"),
        dropped_out.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0);
    let csv = std::fs::read_to_string(&dropped_out).unwrap();
    assert_eq!(
        data_row_count(&csv),
        0,
        "RT15 record should NOT pass --include-rts 7\n--- csv ---\n{csv}"
    );
}

/// Requirements: L2-FLT-001
#[test]
fn exclude_rts_filter_drops_matching_records() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--exclude-rts"),
        std::ffi::OsStr::new("15"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0);
    let csv = std::fs::read_to_string(&output).unwrap();
    assert_eq!(
        data_row_count(&csv),
        0,
        "RT15 record should be dropped by --exclude-rts 15\n--- csv ---\n{csv}"
    );
}

// ── Exit-class summary line (L1-EXIT-005) ────────────────────────────

/// Requirements: L1-EXIT-005, L2-CLI-006
///
/// The exit-class summary line is emitted via `log_info!` so it
/// only surfaces at INFO level or below. Default is WARN, so the
/// test explicitly raises the level. This exercises both the
/// log-level CLI flag and the summary-line format.
///
/// L2-CLI-006 cited because the summary line satisfies the
/// "human-readable diagnostics on stderr" obligation; tagging it
/// here lets the trace matrix attribute the test (the matrix's L1
/// section displays only L2 children + rolled-up status, not
/// direct L1 test markers, so the L1-EXIT-005 tag alone is
/// invisible in the rendered matrix).
#[test]
fn decode_emits_exit_class_summary_at_info_level() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("--log-level"),
        std::ffi::OsStr::new("info"),
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("decode exit class: complete"),
        "stderr must include the L1-EXIT-005 summary line on clean decode\n--- stderr ---\n{stderr}"
    );
}

// ── dump subcommand (L2-CLI-009) ─────────────────────────────────────

/// Requirements: L2-CLI-009
#[test]
fn dump_records_outputs_hex_to_stdout() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());

    let out = run([
        std::ffi::OsStr::new("dump"),
        input.as_os_str(),
        std::ffi::OsStr::new("--records"),
        std::ffi::OsStr::new("1"),
    ]);
    assert_eq!(exit_code(&out), 0, "dump --records 1 must exit 0");

    let stdout = String::from_utf8_lossy(&out.stdout);
    // dump output includes a Cmd Word and at least one hex word from
    // the fixture's payload. Check coarse invariants only — the exact
    // dump format is not part of the cross-impl contract.
    assert!(
        stdout.contains("797E") || stdout.contains("7e79") || stdout.contains("0x797E"),
        "dump output should include the fixture's Cmd Word 0x797E in some form\n--- stdout ---\n{stdout}"
    );
}

// ── Inline error output (L2-ERR-010, L2-ERR-011) ─────────────────────

/// Requirements: L2-ERR-010, L2-ERR-011, L3-RS-009
///
/// `--inline-errors` keeps errored records in the main CSV with the
/// ERROR and ERROR_CODE columns populated, instead of routing them
/// to a separate `_errors.csv` (the default `separate` error mode).
/// This test pins the inline behavior and confirms no split file is
/// produced.
#[test]
fn inline_errors_populates_error_code_column() {
    let tmp = TempDir::new();
    let mut bytes = one_valid_record();
    bytes.extend(errored_record());
    let input = tmp.write("rec.mie", &bytes);
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--inline-errors"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(exit_code(&out), 0);

    let csv = std::fs::read_to_string(&output).expect("output CSV not created");
    assert!(
        data_row_count(&csv) >= 2,
        "inline mode should keep both records in one file (got {} data rows)\n--- csv ---\n{csv}",
        data_row_count(&csv)
    );
    assert!(
        csv.contains("011E"),
        "inline-errors must populate ERROR_CODE with the DDC code (0x011E)\n--- csv ---\n{csv}"
    );

    // The separate `_errors.csv` file must NOT have been created
    // when inline mode is active (L2-ERR-011).
    let errors_csv = tmp.path().join("out_errors.csv");
    assert!(
        !errors_csv.exists(),
        "inline-errors must not produce a separate _errors.csv (found: {})",
        errors_csv.display()
    );
}

/// Requirements: L2-ERR-011, L3-RS-009
#[test]
fn stdout_output_forces_inline_error_mode() {
    let tmp = TempDir::new();
    let mut bytes = one_valid_record();
    bytes.extend(errored_record());
    let input = tmp.write("rec.mie", &bytes);

    let out = run([std::ffi::OsStr::new("decode"), input.as_os_str()]);
    assert_eq!(exit_code(&out), 0);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("011E"),
        "stdout output must inline errored records\n--- stdout ---\n{stdout}"
    );
}

/// Requirements: L2-SYN-013
#[test]
fn debug_sync_failure_includes_bounded_validation_context() {
    let tmp = TempDir::new();
    let mut bytes = one_valid_record();
    bytes.extend(one_valid_record());
    bytes.extend([0x03, 0x00].repeat(5));
    let input = tmp.write("corrupt.mie", &bytes);
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("--log-level"),
        std::ffi::OsStr::new("debug"),
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--strict"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_ne!(exit_code(&out), 0);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("look-ahead message type is unknown"),
        "strict failure should name the precise validation reason\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains("validation context") && stderr.contains("max 32"),
        "DEBUG failure should include a bounded context dump\n--- stderr ---\n{stderr}"
    );
}

/// Requirements: L2-SYN-004, L2-SYN-016
#[test]
fn strict_irig_failure_names_precise_validation_reason() {
    let tmp = TempDir::new();
    let mut bytes = one_valid_record();
    let mut invalid_day = one_valid_record();
    invalid_day[2..4].copy_from_slice(&0x000Fu16.to_le_bytes());
    bytes.extend(invalid_day);
    let input = tmp.write("bad-irig.mie", &bytes);
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--strict"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_ne!(exit_code(&out), 0);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("IRIG day-of-year is out of range"),
        "strict failure should name the precise IRIG field\n--- stderr ---\n{stderr}"
    );
}

// ── Timestamp-format auto-detect (L2-DEC-015) ────────────────────────

/// Requirements: L2-DEC-015
///
/// `--detect-records N` is accepted and the decode completes
/// normally. The probe at N=2 sees the single-record fixture as a
/// 1-record probe (the second record doesn't exist), scores it
/// decisively IRIG, and decodes. No strict-mode assertion here —
/// that path needs an ambiguous fixture, which is task #104's
/// territory.
#[test]
fn detect_records_flag_accepts_valid_size() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--detect-records"),
        std::ffi::OsStr::new("2"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(
        exit_code(&out),
        0,
        "decode with --detect-records 2 must exit 0 on a valid fixture"
    );
    assert!(output.exists(), "output CSV must be created");
}

/// Requirements: L2-SYN-026
///
/// `--lookahead-records N` is accepted in range and the decode
/// completes normally. Default N=2 (DEFAULT_LOOKAHEAD_RECORDS)
/// preserves historical behavior; any value in [1, 32] is valid.
#[test]
fn lookahead_records_flag_accepts_valid_size() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--lookahead-records"),
        std::ffi::OsStr::new("4"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_eq!(
        exit_code(&out),
        0,
        "decode with --lookahead-records 4 must exit 0 on a valid fixture"
    );
    assert!(output.exists(), "output CSV must be created");
}

/// Requirements: L2-SYN-026
#[test]
fn lookahead_records_flag_rejects_out_of_range() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--lookahead-records"),
        std::ffi::OsStr::new("999"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_ne!(
        exit_code(&out),
        0,
        "--lookahead-records 999 must fail (above the max of 32)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--lookahead-records") && stderr.contains("999"),
        "stderr should name the offending flag and value (got: {stderr:?})"
    );
}

/// Requirements: L2-DEC-015
///
/// Out-of-range `--detect-records` is rejected at parse time with a
/// non-zero exit and the valid range in the error message.
#[test]
fn detect_records_flag_rejects_out_of_range() {
    let tmp = TempDir::new();
    let input = tmp.write("rec.mie", &one_valid_record());
    let output = tmp.path().join("out.csv");

    // Above the max of 32.
    let out = run([
        std::ffi::OsStr::new("decode"),
        input.as_os_str(),
        std::ffi::OsStr::new("--detect-records"),
        std::ffi::OsStr::new("999"),
        std::ffi::OsStr::new("-o"),
        output.as_os_str(),
    ]);
    assert_ne!(
        exit_code(&out),
        0,
        "--detect-records 999 must fail (above the max of 32)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--detect-records") && stderr.contains("999"),
        "stderr should name the offending flag and value (got: {stderr:?})"
    );
}
