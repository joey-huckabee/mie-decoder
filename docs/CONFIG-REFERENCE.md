# MIE-Decoder — Configuration Reference

Complete reference for every TOML key the decoder accepts. Use this when:

- You're writing a site-wide or campaign-wide `mie-decoder.toml`.
- The CLI rejected your config file and you need to know why.
- You're hunting for the CLI flag that overrides a particular TOML key.

The fully-commented starter file is [`config/default.toml`](../config/default.toml). This doc covers every key in normative form — what's accepted, what's rejected, what each value does, what the CLI override is.

For the underlying requirement IDs (`L2-CFG-*`), see [`docs/L2-REQ.md`](L2-REQ.md). For exit-code behavior driven by config (`allow_partial`, `no_clobber`), see [`docs/ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## Quick reference

```toml
[logging]
level = "WARNING"                # DEBUG | INFO | WARNING | WARN | ERROR | CRITICAL | OFF

[decode]
time_format       = "auto"       # auto | irig | standard
strict            = false        # true | false
error_mode        = "inline"     # inline | separate
allow_partial     = false        # true | false
detect_records    = 8            # timestamp-format probe size, [1, 32]
lookahead_records = 2            # sync look-ahead depth, [1, 32]
# standard_tick_rate_hz = 1000000.0   # Standard counter Hz (unset = empty DELTA)

[output]
format         = "csv"           # csv (the only value currently supported)
no_clobber     = false           # true | false
max_sort_group = 4096            # 1..=1048576 (1 disables row reordering)

[mux]
enabled   = true                 # populate MUX from the file name (--no-mux disables)
delimiter = "."                  # field separator (non-empty)
field     = 4                    # 0-based field index (negative = from end)

[merge]
delta_scope         = "per-file" # per-file | global (multi-file DELTA scope)
collapse_duplicates = false      # collapse cross-recorder duplicate rows
collapse_window_us  = 0          # timestamp tolerance for collapsing (µs)
max_collapse_survivors = 4096    # cap on the collapse survivor set

[filter]
exclude_types        = []        # array of names or hex codes
exclude_rts          = []        # array of integers in [0, 31]
exclude_buses        = []        # array of "A" / "B"
exclude_subaddresses = []        # array of integers in [0, 31]
```

| Key | Type | Default | CLI override | Pinned by |
|-----|------|---------|--------------|-----------|
| `logging.level` | string | `"WARNING"` | `--log-level` | L2-CFG-001, L1-LOG-001 |
| `decode.time_format` | string | `"auto"` | `--time-format` | L2-CFG-001, L2-DEC-013 |
| `decode.strict` | bool | `false` | `--strict` | L2-CFG-001, L1-MODE-001 |
| `decode.error_mode` | string | `"inline"` | `--separate-errors` (sets `separate`) | L2-CFG-001, L1-ERR-001 |
| `decode.allow_partial` | bool | `false` | `--allow-partial` | L2-CFG-001, L1-EXIT-004 |
| `decode.detect_records` | int | `8` | `--detect-records` | L2-CFG-001, L2-DEC-015 |
| `decode.lookahead_records` | int | `2` | `--lookahead-records` | L2-CFG-001, L2-SYN-026 |
| `decode.standard_tick_rate_hz` | float | unset | `--standard-tick-rate-hz` | L2-CFG-011, L2-DEC-017 |
| `output.format` | string | `"csv"` | `--format` | L2-CFG-001 |
| `output.no_clobber` | bool | `false` | `--no-clobber` | L2-CFG-001, L2-WRT-017 |
| `output.max_sort_group` | int | `4096` | `--max-sort-group` | L2-WRT-022, L1-OUT-003 |
| `mux.enabled` | bool | `true` | `--no-mux` (sets `false`) | L2-WRT-020 |
| `mux.delimiter` | string | `"."` | `--mux-delimiter` | L2-WRT-020 |
| `mux.field` | int | `4` | `--mux-field` | L2-WRT-020 |
| `merge.delta_scope` | string | `"per-file"` | `--delta-scope` | L2-MRG-005, L3-WRT-004 |
| `merge.collapse_duplicates` | bool | `false` | `--collapse-duplicates` | L2-MRG-007 |
| `merge.collapse_window_us` | int | `0` | `--collapse-window-us` | L2-MRG-007 |
| `merge.max_collapse_survivors` | int | `4096` | `--max-collapse-survivors` | L2-MRG-008 |
| `filter.exclude_types` | array | `[]` | `--exclude-types` (additive) | L2-CFG-006, L2-CFG-007 |
| `filter.exclude_rts` | array | `[]` | `--exclude-rts` (additive) | L2-CFG-006 |
| `filter.exclude_buses` | array | `[]` | `--exclude-buses` (additive) | L2-CFG-006 |
| `filter.exclude_subaddresses` | array | `[]` | `--exclude-subaddresses` (additive) | L2-CFG-006 |

---

## Precedence

CLI argument values override configuration file values, which override built-in defaults (L2-CFG-003). The precedence is per-key — a `--log-level INFO` flag overrides only the logging level; other keys still come from the config file (or their built-in defaults if absent).

For filter arrays specifically, CLI values **merge** with config-file values rather than replace them (L2-CFG-004). Example:

```toml
# my-config.toml
[filter]
exclude_types = ["SPURIOUS_DATA"]
```

```bash
mie-decoder --config my-config.toml decode rec.mie --exclude-types BC_TO_RT
# Effective filter: exclude_types = ["SPURIOUS_DATA", "BC_TO_RT"]
```

This matches the operator expectation that CLI filters add to a base set defined in site config, rather than silently replacing them.

---

## Trust boundary

`--config` names a file the decoder **reads**, with the permissions of whoever ran the command. What that means in practice:

| Property | Behavior |
|---|---|
| **Privileges** | The decoder runs as the invoking user and never elevates. It can read exactly what that user could already read with `cat`. |
| **File type** | The path must resolve to a **regular file**. A directory, FIFO, or character device is rejected before any read — so `--config <dir>` reports a clear error instead of an `IsADirectoryError` traceback, and `--config /dev/zero` cannot hang the process on an endless read. |
| **Missing file** | Reported as a configuration error, exit `5`. Never silently ignored, and never falls back to defaults. |
| **Interpretation** | The contents are parsed as TOML **data** and nothing else. There is no `include` directive, no shell interpolation, no code execution, and no network access. An unparseable file is a configuration error (exit `5`), not a partial load. |
| **Location** | **Deliberately unrestricted.** Any readable path is accepted — `/etc/mie-decoder/site.toml`, a mounted share, a path relative to the working directory. See [Site-wide config + per-invocation tweak](#site-wide-config--per-invocation-tweak). |

Both implementations enforce this identically, with the same message text (`Config path is not a regular file: …`), pinned across the two CLIs by `tests/conformance/config_path_parity.py`.

**Why location is not restricted.** Static analysis flags the `--config` path as a possible path-injection vector (SonarCloud `pythonsecurity:S8707`, "Agentic workflows should not be vulnerable to path injection"). That rule assumes a program confined to some root that an attacker-supplied path could escape. MIE-Decoder is an operator-run CLI with no such confinement: the config path *is* the interface, and a caller who can pass `--config` can already read the same file directly. Constraining configs to an allowlist of roots would therefore add no protection while breaking the site-config deployments the tool is built for.

That finding is suppressed for `python/src/mie_decoder/config.py` — scoped to that one rule in that one file, so any other finding there, and that rule anywhere else, still fails the build. The rationale is recorded next to the exclusion in `.github/workflows/sonarcloud.yml`; keep the two in step if either changes.

**The same reasoning covers `--manifest`, for a different path.** SonarCloud also reports `pythonsecurity:S2083` and `pythonsecurity:S8707` against the input-file `open()` in `reader.py`. That flow does **not** start at a command-line argument: it starts at the *contents* of a `--manifest` file, whose lines become input paths (`merge.py` → `cli.py` → `reader.py`). The claim there is narrower than the one above — not "the path is the interface", but that a manifest's contents are exactly as trusted as the operator who chose that manifest. Reading the files it lists is what `--manifest` is for (`L2-MRG-001`), and anyone able to write the manifest can already invoke the decoder with any argument they like, so the flow confers no capability. Both rules are suppressed for `reader.py` on the same scoped basis.

**What this does not cover.** If you run the decoder somewhere the invoking user is *not* the trust boundary — a setuid wrapper, a shared service account, a job runner that accepts a config path from an untrusted submitter — then `--config` is as privileged as that context, and restricting the path is the caller's responsibility, not the decoder's.

---

## `[logging]`

### `level`

**Type:** string · **Default:** `"WARNING"` · **CLI:** `--log-level <name>`

Diagnostic logging verbosity. Accepted values (case-insensitive):

| Value | What it emits |
|-------|---------------|
| `DEBUG` | Per-record decode details, CLI parsed arguments, truncation events. Verbose. |
| `INFO` | File open/close, decode start/complete with counts, auto-detected timestamp format, exit-class summary (L1-EXIT-005), header detection size (L2-SYN-012), sync-recovery successes (L2-SYN-013). |
| `WARNING` / `WARN` | Invalid records (lenient skip), freerun timestamps, unknown DDC error codes (lenient), non-monotonic timestamps (L2-RDR-017), sync loss (L2-SYN-013), structural-invariant violations (lenient), L2-SYN anomalies (L2-SYN-024/025). The two spellings are equivalent. |
| `ERROR` | File not found, empty file, write failures, NoValidRecords, HomogeneousPayload, UnrecoverableSyncLoss. |
| `CRITICAL` | Nothing — the decoder emits no CRITICAL-level messages, so selecting `CRITICAL` suppresses all output (it does **not** behave like `ERROR`). |
| `OFF` | Nothing — explicit "silence all output". Equivalent to `CRITICAL` for this decoder; both map to the Rust logger's `Level::Off`. |

**Validation:** rejected at load time if not one of the above. Case is normalized internally to the canonical uppercase form.

---

## `[decode]`

### `time_format`

**Type:** string · **Default:** `"auto"` · **CLI:** `--time-format <auto|irig|standard>`

Selects the timestamp format used by the binary file. DDC recording cards support two formats, configured at recording time. All records in a single file use the same format (L2-DEC-011).

| Value | Behavior |
|-------|----------|
| `"auto"` | Auto-detect by probing up to the first `decode.detect_records` records (default 8; L2-DEC-015). The decoder probes the Command Word at both candidate offsets and scores which produces a valid MIL-STD-1553 command, aggregating the scores across the probe set. Recommended for most workflows. |
| `"irig"` | Force the 48-bit IRIG-B format (3 × 16-bit words = day, hour, minute, second, microsecond, freerun flag). Provides absolute wall-clock time. |
| `"standard"` | Force the 32-bit free-running counter format (2 × 16-bit words). Provides relative timing only; tick rate is card-dependent and not encoded in the file. `DELTA` is empty for Standard records unless you supply [`standard_tick_rate_hz`](#standard_tick_rate_hz) (L2-RDR-019). |

**Tie-break (L2-DEC-012):** When `"auto"` and both formats score equally, IRIG is selected. Flight-test recordings overwhelmingly use IRIG; this tie-break preserves the most common path.

**Validation:** rejected at load time if not one of the three values. The name is matched case-insensitively (`IRIG`, `Auto`, and `standard` are all accepted), identically on the CLI and in the config file. An explicit `irig` or `standard` is still sanity-checked against the L2-DEC-015 detection probe — pointing the decoder at an IRIG file with `--time-format standard` surfaces a distinct error class in strict mode when the probe is decisive for the other format (L2-DEC-013).

### `strict`

**Type:** bool · **Default:** `false` · **CLI:** `--strict`

Enables strict decoding mode (L1-MODE-001). In strict mode:

- Invalid records (bad Type Word, IRIG range failure, look-ahead failure) surface as exceptions instead of being skipped with a `WARN`.
- L2-SYN structural invariant violations (L2-SYN-020 through L2-SYN-023) abort decoding.
- Unknown DDC error codes (L2-ERR-004) raise instead of being passed through as `UNKNOWN`.
- Truncated records raise `MieRecordTruncatedError` / `MieError::RecordTruncated`.

Lenient mode (the default) preserves the maximum number of valid records by logging diagnostics and continuing. Use strict for triage and CI; use lenient for field analysis.

**Validation:** TOML boolean only — `true` or `false`. Strings like `"true"` are rejected at load time.

### `error_mode`

**Type:** string · **Default:** `"inline"` · **CLI:** `--separate-errors` (sets `separate`)

Controls how errored records (Type Word bit 14 set) and SPURIOUS_DATA records appear in CSV output (L1-ERR-001, L2-ERR-008, L2-ERR-011).

| Value | Behavior |
|-------|----------|
| `"separate"` | Errored and spurious messages are written to a separate file named `<output_stem>_errors<output_suffix>`. The main CSV contains only clean messages. Stem/suffix rules per L2-ERR-008: `out.csv` → `out_errors.csv`; `out` → `out_errors`; `data.bar.csv` → `data.bar_errors.csv`. The errors file is not created if there are no error rows. |
| `"inline"` | Errored, spurious, and normal messages all go to one CSV. The `ERROR` column contains `ERROR` or `SPURIOUS` (or empty for clean); `ERROR_CODE` contains the 4-character uppercase hex code. |

**Stdout output forces `inline` mode** in both implementations (you can't split stdout into two streams), so `--separate-errors` is ignored there with a WARN.

> **Changed in v2.8.0:** the default flipped from `"separate"` to `"inline"`, and the CLI override changed from `--inline-errors` to `--separate-errors`. The old flag was removed, so passing it is a usage error (exit 4). A config file that sets `error_mode` explicitly is unaffected.

**Validation:** rejected at load time if not one of the two values.

### `allow_partial`

**Type:** bool · **Default:** `false` · **CLI:** `--allow-partial`

Controls the behavior on unrecoverable mid-file sync loss (L1-EXIT-004 / L2-WRT-016).

| Value | Behavior |
|-------|----------|
| `false` | Default. `MieError::UnrecoverableSyncLoss` / `MieUnrecoverableSyncLossError` exits 3. The temporary CSV file is unlinked; the destination is not touched. |
| `true` | The decoded rows up to the sync-loss point are renamed from the temp file to `<destination>.partial`. The original `<destination>` is not touched. The CLI exits 0 with a WARN summary. |

Use `true` when investigating a recording that's known to be corrupt and you want to inspect what was decodable. Use `false` (the default) in pipelines where any unrecoverable corruption should abort with a distinct exit code.

**Validation:** TOML boolean only.

### `standard_tick_rate_hz`

**Type:** float · **Default:** unset · **CLI:** `--standard-tick-rate-hz <HZ>`

Calibrates the Standard (free-running counter) timestamp format so its records can carry a `DELTA` (L2-DEC-017, L2-RDR-019). The Standard counter's tick rate is card-dependent and is *not* stored in the recording, so by default the decoder cannot express a Standard timestamp as elapsed time and leaves `DELTA` empty.

When you set this key to your card's counter frequency in Hz, the decoder converts each raw counter value to microseconds —

```
microseconds = round(raw_ticks × 1_000_000 / standard_tick_rate_hz)
```

— and Standard records then participate in per-RT/MSG `DELTA` tracking on exactly the same terms as IRIG records (first occurrence `0.000000`, subsequent gaps in seconds, empty on a non-monotonic step). Rounding is half-away-from-zero and is identical across the Rust and Python implementations.

This setting has no effect on IRIG recordings (IRIG already carries absolute time) and no effect when `time_format` resolves to anything other than `standard`.

**Example.** Two consecutive records of the same RT/MSG 16 ticks apart, decoded with a 1 MHz rate, yield a `DELTA` of `0.000016`:

```bash
mie-decoder decode rec.mie -o out.csv --time-format standard --standard-tick-rate-hz 1000000
```

**Validation:** must be a finite number strictly greater than `0`. A non-positive or non-finite value is rejected — at load time for the TOML key (L2-CFG-011) and at parse time for the CLI flag (L2-CLI-012) — so a bad rate can never silently produce meaningless timing.

---

## `[output]`

### `format`

**Type:** string · **Default:** `"csv"` · **CLI:** `--format <name>`

Output file format. `csv` is currently the only valid value. Reserved for future Parquet support (see [`ROADMAP.md`](ROADMAP.md)).

A config-file value is validated at load time (exit `5`); a `--format` value on the CLI is validated at **parse** time and an unsupported value is a usage error (exit `4`), like any other invalid flag value.

Through v2.14.0 the CLI form surfaced as a runtime error (exit `1`) because the check ran after the config layer had merged the override. That put one mistake on three different exit codes depending on where it was written — `4` for a bad `--time-format`, `1` for a bad `--format`, `5` for the same value in a config file — and contradicted L1-EXIT-007. Only the middle one was wrong; the config-file spelling is unchanged.

**Validation:** rejected at load time if not `csv` (config file, exit `5`) or at parse time (CLI flag, exit `4`).

### `no_clobber`

**Type:** bool · **Default:** `false` · **CLI:** `--no-clobber`

Controls whether the writer is allowed to overwrite an existing destination (L2-WRT-017).

| Value | Behavior |
|-------|----------|
| `false` | Default. Overwriting an existing destination succeeds. Matches operator expectations for batch reruns. |
| `true` | Refuses to overwrite. Surfaces `MieClobberRefusedError` / `MieError::ClobberRefused` and exits 1. Set this in pipelines where overwriting a possibly-newer result is unacceptable. |

When `error_mode = "separate"`, the no-clobber check applies to both the main output AND the errors file — either existing triggers refusal.

**Validation:** TOML boolean only.

### `max_sort_group`

**Type:** int · **Default:** `4096` · **Range:** `[1, 1048576]` · **CLI:** `--max-sort-group`

Caps how many **consecutive records sharing one `TIME_STAMP`** the canonical row-order stage buffers at once (L2-WRT-022).

Rows are always written in canonical order (L1-OUT-003): ascending `TIME_STAMP`, then ascending `RT`, then `MSG` (subaddress ascending, `R` before `T`). Producing that order requires holding one run of equal-timestamp records long enough to sort it — the only buffer in the pipeline whose size depends on the data rather than on the input count. This key bounds it.

| Value | Behavior |
|-------|----------|
| `1` | **Disables reordering.** Every record is its own run, so output is raw DDC capture order. This is the supported way to get byte-for-byte row parity with vendor CSV — see [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) §3a. |
| `4096` | Default. Far above any real tie: a 1553 bus carries one transaction at a time, so genuine ties come only from the two concurrent buses or from overlapping recorders in a merge — single digits in practice. |
| up to `1048576` | Raise it only if you have a legitimate reason to expect enormous equal-timestamp runs. Worst-case buffering scales with this value. |

**On overflow** the stage writes the buffered run in **arrival order**, emits exactly one WARN naming the timestamp and the cap, and continues. No record is dropped and the decode does not fail — the ordering guarantee is simply suspended for that run. The motivating case is a corrupt or misconfigured recording whose timestamps all decode to the same value, which would otherwise buffer the entire file.

**Validation:** TOML integer only (a bool or string is rejected); out-of-range values are rejected at load time (exit `5`). An out-of-range `--max-sort-group` is a usage error (exit `4`), matching `--detect-records`.

---

## `[mux]`

Populate the `MUX` CSV column from a field of each input's **file name** (L2-WRT-020). Useful when recordings are named so a field encodes the source / recorder, e.g. `full_loadout.draw.data.1553.aa.unused.mie_irig` → `MUX = aa`. In a multi-file merge each row carries the MUX of the file it was decoded from.

> **Vendor compatibility:** this is **on by default**, so default output is *not* byte-for-byte identical to the DDC vendor CSV when the input name has the field. Set `enabled = false` (or pass `--no-mux`) for a vendor-exact diff. See [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md).

### `enabled`

**Type:** bool · **Default:** `true` · **CLI:** `--no-mux` (sets `false`)

When `true`, the MUX column is derived from the file name; when `false`, MUX is left empty. **Validation:** TOML boolean only.

### `delimiter`

**Type:** string · **Default:** `"."` · **CLI:** `--mux-delimiter`

Separator the file's basename is split on. **Validation:** must be a non-empty string (rejected at load time otherwise).

### `field`

**Type:** int · **Default:** `4` · **CLI:** `--mux-field`

0-based index of the field used as the MUX value; a **negative** index counts from the end (e.g. `-1` is the last field). An out-of-range index, an empty selected field, or a `false` `enabled` leaves MUX empty. **Validation:** TOML integer only (any value accepted; out-of-range simply yields empty MUX).

---

## `[merge]`

Multi-file merge behavior: DELTA scope (L2-MRG-005) and cross-recorder duplicate collapsing (L2-MRG-007). Every key here applies only to a multi-file merge; a single-file decode is unaffected by all of them.

### `delta_scope`

**Type:** string · **Default:** `"per-file"` · **Values:** `per-file`, `global` (case-insensitive) · **CLI:** `--delta-scope`

Selects the scope over which the `DELTA` column is measured when more than one input is decoded.

| Value | Meaning |
|-------|---------|
| `"per-file"` | **Default.** A record's gap is to the previous same-RT/MSG record **from its own file**. The value is identical to what that record gets when its file is decoded alone — and to what the DDC vendor tool reports, since the vendor has no merge feature. |
| `"global"` | A record's gap is to the previous same-RT/MSG record from **any** input, measured across the merged timeline. Answers "how long since *any* recorder last saw this key". |

The two differ **only for RT/MSG keys that appear in more than one input file.** A key present in just one file gets the same DELTA under either scope, because only that file's records can advance its tracker.

`global` compresses inter-arrival gaps whenever a key is shared: two recorders each seeing a 0.2 s cadence produce an apparent 0.1 s under `global`. For fully overlapping recorders it degenerates further, to alternating `0.000000` values — one per duplicate pair — unless `collapse_duplicates` is also on.

> **Changed in v2.11.0:** the default was `global` through v2.10.0. It is now `per-file`, so merged DELTA no longer depends on how the input set was assembled. Pass `--delta-scope global` (or set this key) to restore the previous behavior.

**Interaction with `collapse_duplicates`:** under `per-file`, collapsing cannot change any surviving record's DELTA — that value is a property of the record's own file, not of the merged stream. Under `global` it can, since suppressed rows no longer advance the shared tracker.

**Validation:** an unrecognized name is a config error at load time (exit `5`); an unrecognized `--delta-scope` is a usage error (exit `4`).

### `collapse_duplicates`

**Type:** bool · **Default:** `false` · **CLI:** `--collapse-duplicates`

Enable cross-recorder duplicate collapsing.

### `collapse_window_us`

**Type:** int · **Default:** `0` · **CLI:** `--collapse-window-us`

Timestamp tolerance in microseconds: two recorders' copies of the same event collapse when their timestamps differ by at most this much. `0` requires an exact-microsecond match (the safest setting — never over-collapses); widen it for recorders whose IRIG clocks are not perfectly synced. **Validation:** non-negative integer (a negative value is rejected at load time).

### `max_collapse_survivors`

**Type:** int · **Default:** `4096` · **CLI:** `--max-collapse-survivors`

Cap on how many survivors the collapse window retains at once (L2-MRG-008).

`collapse_window_us` bounds retention in **time**. It cannot bound it in **count**: a corrupt recording whose timestamps all decode to one value, or a wide window on a dense bus, puts arbitrarily many records inside a single window. This is the second bound, and it is what makes the merge's constant-memory guarantee unconditional — the same role `output.max_sort_group` plays for the canonical-order stage, with the same default for the same reason.

On reaching the cap the decoder drops the oldest survivor to make room, emits **one** warning for the whole run, and carries on: collapsing becomes best-effort rather than exact. No record is ever dropped from the *output*. Raise this if you widen `collapse_window_us` on a busy bus and see that warning.

**Validation:** integer within `[1, 1048576]`, checked at load time.

---

## `[filter]`

Filtering happens after decoding and before CSV output. Filtered messages are silently dropped — they do not appear in the output CSV and are not counted in `count`.

All four filter lists use **OR logic** (L2-FLT-002): a message is excluded if it matches **any** of the configured exclusion criteria. There is no AND.

CLI filter values **merge** with config-file values (L2-CFG-004) — they don't replace them.

### `exclude_types`

**Type:** array of string or integer · **Default:** `[]` · **CLI:** `--exclude-types <name1,name2,...>` (additive)

Exclude messages by Type Word message type. In a TOML config each array element may be a symbolic name (case-insensitive), a hexadecimal-code string (`"0x02"`), or a bare integer code (`2`); the three are interchangeable per L2-CFG-007. (On the CLI the value is always a string.) Codes are bounded to a `u8` (`0..=255`) in both implementations — an out-of-range code (e.g. `"0x100"`) is rejected (a CLI usage error → exit 4, or a config error → exit 5), never silently ignored.

| Symbolic | Hex | Description |
|----------|-----|-------------|
| `MODE_COMMAND` | `0x01` | Mode code messages |
| `BC_TO_RT` | `0x02` | Bus Controller to Remote Terminal |
| `RT_TO_BC` | `0x04` | Remote Terminal to Bus Controller |
| `RT_TO_RT` | `0x08` | Terminal-to-Terminal transfers |
| `BROADCAST_BC_TO_RT` | `0x10` | Broadcast BC→RT |
| `BROADCAST_RT_TO_RT` | `0x18` | Broadcast RT→RT |
| `SPURIOUS_DATA` | `0x20` | Spurious bus noise (records without a Command Word) |

```toml
exclude_types = ["SPURIOUS_DATA", "0x01"]   # mixed forms OK
```

**Validation:** unknown symbolic names or invalid hex are rejected at load time with a clear error naming the offending entry.

### `exclude_rts`

**Type:** array of int · **Default:** `[]` · **CLI:** `--exclude-rts <n1,n2,...>` (additive)

Exclude messages by Remote Terminal address. Each value must be an integer in `[0, 31]`. Address 31 is the MIL-STD-1553 broadcast address.

```toml
exclude_rts = [0, 31]   # exclude broadcast + RT 0
```

SPURIOUS_DATA records have no RT and are unaffected by this filter.

**Validation:** out-of-range values rejected at load time (per L2-CFG schema reference).

### `exclude_buses`

**Type:** array of string · **Default:** `[]` · **CLI:** `--exclude-buses <A|B,...>` (additive)

Exclude messages by bus. Each value must be `"A"` or `"B"` (case-insensitive).

```toml
exclude_buses = ["B"]   # decode only Bus A
```

**Validation:** any value other than A/B (case-insensitive) rejected at load time.

### `exclude_subaddresses`

**Type:** array of int · **Default:** `[]` · **CLI:** `--exclude-subaddresses <n1,n2,...>` (additive)

Exclude messages by subaddress. Each value must be an integer in `[0, 31]`. Subaddresses 0 and 31 are mode-code subaddresses per MIL-STD-1553B.

```toml
exclude_subaddresses = [0, 31]   # exclude mode-code subaddresses
```

SPURIOUS_DATA records have no subaddress and are unaffected.

**Validation:** out-of-range values rejected at load time.

---

## Unknown keys

Per L2-CFG-009, unknown top-level TOML keys produce a `WARN` at load time naming the offending `[section] key` but **do not fail the load**. This is forward-compatible: an older binary opening a newer config logs the unknown keys it doesn't understand and continues with the keys it does.

Examples that produce a WARN but still load:

```toml
[output]
format = "csv"
unknown_thing = true   # WARN: unknown key [output] unknown_thing

[filter]
exclude_subdresses = [0]   # WARN: typo of exclude_subaddresses
```

Use the `WARN`-level log to catch typos in your config without an explicit schema validator.

---

## CLI-only options (no TOML key)

Some `decode` capabilities are intentionally **CLI-only** and have **no
configuration-file key**:

- **Include filters** — `--include-types` / `--include-rts` / `--include-buses`
  / `--include-subaddresses` (per-invocation overrides; see the `[filter]`
  section for the exclude-side keys).
- **Multi-file merge inputs** — the multiple positionals, `--manifest`, and
  `--glob` that select a set of recordings to merge (L2-MRG-001). Whether to
  merge, and which files, is a per-invocation decision, so those input-selection
  options are CLI-only. The merge honors the existing `[decode]` / `[filter]` /
  `[output]` keys (e.g. `strict`, `allow_partial`, `standard_tick_rate_hz`,
  filters) applied uniformly across all inputs, plus the `[merge]` section above
  for cross-recorder duplicate collapsing (L2-MRG-007).

---

## Validation timing

Per L2-CFG-010, all schema validation (type checks, range checks, enum membership, unknown-key detection) happens at **configuration load time**, not at use time. By the time a `DecoderConfig` / `MieConfig` is constructed, the values have been validated.

This means a config file is either fully accepted (with optional WARN lines for unknown keys) or fully rejected at the very start of CLI invocation, before any file mmap or output write occurs. There is no class of "config error surfacing mid-decode."

When a load-time validation fails, the CLI exits `5` (the configuration-error class, L1-EXIT-008 / L2-CLI-011) with a stderr message naming the offending key and the rule it broke, and creates no output file.

---

## Per-implementation notes

### Python and `tomllib`

Python's TOML parser is the standard-library `tomllib` on Python 3.11+ and the `tomli` package on Python 3.10 (L3-PY-005). Either way, the schema validation is identical — the TOML library only parses; the decoder validates.

### Rust hand-rolled TOML parser (accepted subset)

To preserve the crate's single-dependency design (only `memmap2`), Rust parses TOML with a hand-rolled reader that accepts a deliberately small subset — everything the decoder's schema needs, and nothing more:

- `[section]` headers (a simple identifier — letters, digits, underscore) and `key = value` pairs, one per line, where the key is a simple identifier;
- values:
  - **numbers** matching `[+-]? (0 | [1-9][0-9]*) (.[0-9]+)? ([eE][+-]?[0-9]+)?` — plain decimal integers and floats, **no** leading zeros (`08`), **no** bare trailing dot (`1.`), and **no** `0x` / `0o` / `0b` prefixes or `_` separators;
  - **strings** — double-quoted, supporting exactly the escapes `\"` `\\` `\n` `\t` (not `\r`, `\uXXXX`, or others);
  - **booleans** (`true` / `false`), and **single-line** arrays of the above (`[1, 2, 3]` or `["A", "B"]`);
- `#` line comments and trailing comments (a `#` inside a quoted string is preserved).

**Anything outside this flat subset is a load-time config error (exit `5`) on _both_ implementations.** Rather than accept different subsets and reconcile them one form at a time, Python validates every config line against the same grammar the Rust parser accepts (a whitelist run before `tomllib`), and both refuse the rest. The full-TOML forms that are therefore rejected include:

- multi-line / spanning arrays;
- underscore digit separators in numbers (`1_000_000` — write `1000000`), `0x` / `0o` / `0b` integer prefixes, leading zeros (`08`, `01`), and a bare trailing dot (`1.`);
- string escapes beyond `\"` `\\` `\n` `\t` (e.g. `\r`, `\uXXXX`);
- inline tables (`{ ... }`) and date-time values;
- quoted keys (`"strict" = ...`);
- dotted keys (`decode.strict = true`), dotted section headers (`[output.no_clobber]`), and array-of-tables headers (`[[decode]]`).

`tomllib` and Rust's native number/string parsing each accept forms the other does not (`tomllib` honors dotted keys and normalizes `1_000`; Rust's `i64`/`f64` accept `08` / `1.`), so the two are pinned to this single explicit grammar instead of reconciled form by form. Two guards run both CLIs against each other in CI: a curated parity corpus (`tests/conformance/config_parity.py`) and a differential **fuzzer** (`config_fuzz.py`) that generates config documents and asserts identical accept/reject — so a new divergence is caught by CI rather than in the field.

**Duplicate keys and re-declared sections are rejected by both implementations.** A repeated `(section, key)`, or a `[section]` header declared more than once (even with different keys inside), is a load-time config error (exit `5`) on Rust as well as Python — the hand-rolled parser previously kept the *first* value / silently merged the re-opened section; it now matches `tomllib`, which raises per the TOML spec.

The bundled `config/default.toml` stays within this subset, so a config derived from it is portable across both implementations.

### Implementations may add namespaced keys

Per L2-CFG-008, implementations MAY add additional keys under namespaces that don't collide with the shared schema (e.g., a Rust-only `[rust]` section), and implementations that don't recognize a key warn rather than reject. Today neither impl exercises this; reserved for future per-impl features.

---

## Examples

### Minimal — disable error file for cleaner output

```toml
[decode]
error_mode = "inline"
```

### Strict pipeline mode — fail fast on any anomaly

```toml
[decode]
strict = true

[output]
no_clobber = true
```

### Field-deployed analysis — extract maximum data even from corrupt recordings

```toml
[decode]
strict        = false   # default; explicit for clarity
allow_partial = true    # don't lose what was decoded

[logging]
level = "INFO"          # see recovery summary
```

### Vendor-exact output — for a byte-for-byte diff against DDC CSV

```toml
[mux]
enabled = false          # leave MUX empty like the other placeholder columns

[output]
max_sort_group = 1       # raw capture order, no equal-timestamp reordering
```

Equivalent on the command line: `--no-mux --max-sort-group 1`. See
[`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) §3 and §3a.

### Focused investigation — only Bus A, only Receive transactions, no spurious

```toml
[filter]
exclude_buses = ["B"]
exclude_types = ["RT_TO_BC", "SPURIOUS_DATA", "MODE_COMMAND"]
```

### Site-wide config + per-invocation tweak

```toml
# /etc/mie-decoder/site.toml
[filter]
exclude_types = ["SPURIOUS_DATA"]

[logging]
level = "INFO"
```

```bash
mie-decoder --config /etc/mie-decoder/site.toml --log-level WARNING \
            decode flight.mie \
            --exclude-rts 31
# Effective: SPURIOUS_DATA filtered (config), RT 31 filtered (CLI merge),
# log level WARNING (CLI override of INFO).
```
