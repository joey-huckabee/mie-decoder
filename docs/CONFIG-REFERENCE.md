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
time_format    = "auto"          # auto | irig | standard
strict         = false           # true | false
error_mode     = "separate"      # separate | inline
allow_partial  = false           # true | false
# standard_tick_rate_hz = 1000000.0   # Standard counter Hz (unset = empty DELTA)

[output]
format     = "csv"               # csv (only value in v1)
no_clobber = false               # true | false

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
| `decode.strict` | bool | `false` | (no CLI flag in current versions) | L2-CFG-001, L1-MODE-001 |
| `decode.error_mode` | string | `"separate"` | `--inline-errors` (sets `inline`) | L2-CFG-001, L1-ERR-001 |
| `decode.allow_partial` | bool | `false` | `--allow-partial` | L2-CFG-001, L1-EXIT-004 |
| `decode.detect_records` | int | `8` | `--detect-records` | L2-CFG-001, L2-DEC-015 |
| `decode.lookahead_records` | int | `2` | `--lookahead-records` | L2-CFG-001, L2-SYN-026 |
| `decode.standard_tick_rate_hz` | float | unset | `--standard-tick-rate-hz` | L2-CFG-011, L2-DEC-017 |
| `output.format` | string | `"csv"` | (no CLI flag) | L2-CFG-001 |
| `output.no_clobber` | bool | `false` | `--no-clobber` | L2-CFG-001, L2-WRT-017 |
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
mie-decoder decode rec.mie --config my-config.toml --exclude-types BC_TO_RT
# Effective filter: exclude_types = ["SPURIOUS_DATA", "BC_TO_RT"]
```

This matches the operator expectation that CLI filters add to a base set defined in site config, rather than silently replacing them.

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

**Validation:** rejected at load time if not one of the three values. An explicit `irig` or `standard` is still sanity-checked against the L2-DEC-015 detection probe — pointing the decoder at an IRIG file with `--time-format standard` surfaces a distinct error class in strict mode when the probe is decisive for the other format (L2-DEC-013).

### `strict`

**Type:** bool · **Default:** `false` · **CLI:** (no flag — set via config file only)

Enables strict decoding mode (L1-MODE-001). In strict mode:

- Invalid records (bad Type Word, IRIG range failure, look-ahead failure) surface as exceptions instead of being skipped with a `WARN`.
- L2-SYN structural invariant violations (L2-SYN-020 through L2-SYN-023) abort decoding.
- Unknown DDC error codes (L2-ERR-004) raise instead of being passed through as `UNKNOWN`.
- Truncated records raise `MieRecordTruncatedError` / `MieError::RecordTruncated`.

Lenient mode (the default) preserves the maximum number of valid records by logging diagnostics and continuing. Use strict for triage and CI; use lenient for field analysis.

**Validation:** TOML boolean only — `true` or `false`. Strings like `"true"` are rejected at load time.

### `error_mode`

**Type:** string · **Default:** `"separate"` · **CLI:** `--inline-errors` (sets `inline`)

Controls how errored records (Type Word bit 14 set) and SPURIOUS_DATA records appear in CSV output (L1-ERR-001, L2-ERR-008, L2-ERR-011).

| Value | Behavior |
|-------|----------|
| `"separate"` | Errored and spurious messages are written to a separate file named `<output_stem>_errors<output_suffix>`. The main CSV contains only clean messages. Stem/suffix rules per L2-ERR-008: `out.csv` → `out_errors.csv`; `out` → `out_errors`; `data.bar.csv` → `data.bar_errors.csv`. The errors file is not created if there are no error rows. |
| `"inline"` | Errored, spurious, and normal messages all go to one CSV. The `ERROR` column contains `ERROR` or `SPURIOUS` (or empty for clean); `ERROR_CODE` contains the 4-character uppercase hex code. |

**Stdout output forces `inline` mode** in both implementations (you can't split stdout into two streams).

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

**Type:** string · **Default:** `"csv"` · **CLI:** (no flag)

Output file format. The only valid value in v1 is `csv`. Reserved for future Parquet support (v3.0 Rust roadmap).

**Validation:** rejected at load time if not `csv`.

### `no_clobber`

**Type:** bool · **Default:** `false` · **CLI:** `--no-clobber`

Controls whether the writer is allowed to overwrite an existing destination (L2-WRT-017).

| Value | Behavior |
|-------|----------|
| `false` | Default. Overwriting an existing destination succeeds. Matches operator expectations for batch reruns. |
| `true` | Refuses to overwrite. Surfaces `MieClobberRefusedError` / `MieError::ClobberRefused` and exits 1. Set this in pipelines where overwriting a possibly-newer result is unacceptable. |

When `error_mode = "separate"`, the no-clobber check applies to both the main output AND the errors file — either existing triggers refusal.

**Validation:** TOML boolean only.

---

## `[filter]`

Filtering happens after decoding and before CSV output. Filtered messages are silently dropped — they do not appear in the output CSV and are not counted in `count`.

All four filter lists use **OR logic** (L2-FLT-002): a message is excluded if it matches **any** of the configured exclusion criteria. There is no AND.

CLI filter values **merge** with config-file values (L2-CFG-004) — they don't replace them.

### `exclude_types`

**Type:** array of string · **Default:** `[]` · **CLI:** `--exclude-types <name1,name2,...>` (additive)

Exclude messages by Type Word message type. Accepts symbolic names (case-insensitive) or hexadecimal codes (`"0x02"` etc.) interchangeably per L2-CFG-007.

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

## Validation timing

Per L2-CFG-010, all schema validation (type checks, range checks, enum membership, unknown-key detection) happens at **configuration load time**, not at use time. By the time a `DecoderConfig` / `MieConfig` is constructed, the values have been validated.

This means a config file is either fully accepted (with optional WARN lines for unknown keys) or fully rejected at the very start of CLI invocation, before any file mmap or output write occurs. There is no class of "config error surfacing mid-decode."

When a load-time validation fails, the CLI exits `5` (the configuration-error class, L1-EXIT-008 / L2-CLI-011) with a stderr message naming the offending key and the rule it broke, and creates no output file.

---

## Per-implementation notes

### Rust-only TOML extensions

The Rust crate accepts include-filter CLI flags (L3-RS-010): `--include-types`, `--include-rts`, `--include-buses`, `--include-subaddresses`. These are **CLI-only on Rust** — they have no TOML representation. The Python implementation does not provide include filters; the shared L1-CLI-002 capability of "filter by type / RT / bus / subaddress" is met via the exclude filters in both impls.

### Python and `tomllib`

Python's TOML parser is the standard-library `tomllib` on Python 3.11+ and the `tomli` package on Python 3.10 (L3-PY-005). Either way, the schema validation is identical — the TOML library only parses; the decoder validates.

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
mie-decoder decode flight.mie --config /etc/mie-decoder/site.toml \
                              --exclude-rts 31 \
                              --log-level WARNING
# Effective: SPURIOUS_DATA filtered (config), RT 31 filtered (CLI merge),
# log level WARNING (CLI override of INFO).
```
