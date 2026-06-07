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
/// `record_rt15_sa11_rcv` fixture in `tests/integration.rs` —
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

/// Requirements: L2-CLI-005, L1-EXIT-002
#[test]
fn no_args_invocation_exits_non_zero() {
    let out = run(Vec::<&str>::new());
    assert_ne!(
        exit_code(&out),
        0,
        "invoking the binary with no arguments must fail with a non-zero exit"
    );
}
