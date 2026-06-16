# MIE-Decoder — Vendor CSV Alignment & Diffs

Documented column-by-column alignment between MIE-Decoder's CSV output and DDC's vendor-generated CSV. Read this when:

- You're validating that MIE-Decoder produces the same data your existing pipelines expect from the vendor tool.
- You ran `diff` against vendor output and found a mismatch.
- You're integrating MIE-Decoder into a system that previously consumed vendor CSV.

The short version: by spec (`L1-OUT-001`) MIE-Decoder produces CSV that is **column-name and column-order compatible** with the DDC vendor recorder's output. The cross-implementation conformance suite asserts byte-identical CSV between the Rust and Python implementations; that suite's oracles are derived from validated vendor output. In practice, except for the documented exceptions below, a single `diff` should produce zero lines of difference between MIE-Decoder output and a vendor CSV of the same recording.

---

## 1. Quick verdict

| Category | Status |
|----------|--------|
| Column names | **Match** (15 columns in the spec order) |
| Column order | **Match** |
| Cell formatting (hex width, casing, decimal precision) | **Match** |
| Line endings | **Match** (both produce LF; see §4) |
| Per-row data content for clean records | **Match** |
| Per-row data content for errored / SPURIOUS records | **Match** |
| Vendor-empty columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`) | **Both empty in v1** (see §3) |
| IRIG `TIME_STAMP` day-of-year field | **Firmware-dependent discrepancy** on some DDC card models (see §5) |

If you find a divergence outside the documented exceptions, **it is a bug** in MIE-Decoder. See §7.

---

## 2. The 15 CSV columns

In order, exactly as both tools emit them:

```
TIME_STAMP, RT, MSG, WD01, WD02, ..., WD32, STAT, CMD, MUX, TERM_NAME, BUS, DELTA, ERROR, ERROR_CODE, IM_GAP, RCV_GAP, XMT_GAP
```

That's 1 + 1 + 1 + 32 (data words) + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 = **46 columns** total. Reordering or renaming any column would break the L1-OUT-001 byte-compat contract.

### Cells that match exactly

The following columns produce byte-identical content between MIE-Decoder and the vendor tool for any clean (non-errored, non-spurious) record:

| Column | Format | Notes |
|--------|--------|-------|
| `TIME_STAMP` | `DAY:HH:MM:SS.uuuuuu` (IRIG) or `0xNNNNNNNN` (Standard) | Day-of-year on IRIG has a known firmware discrepancy — see §5. Otherwise identical. |
| `RT` | Integer 0–31, no padding | Empty for SPURIOUS_DATA. |
| `MSG` | `<subaddress><T\|R>` (e.g. `11R`, `22T`) | Empty for SPURIOUS_DATA. |
| `WD01` … `WD32` | 4-character uppercase hex, no `0x` prefix | Unused trailing columns are **empty cells** (not `0000`). |
| `STAT` | 4-character uppercase hex | Empty when not present (some Mode Code formats). |
| `CMD` | 4-character uppercase hex | Empty for SPURIOUS_DATA. |
| `BUS` | Single character `A` or `B` | |
| `DELTA` | `0.000000` (6 decimals) or empty | Empty for SPURIOUS_DATA, uncalibrated Standard-timestamp records (no tick rate configured — supply `standard_tick_rate_hz` to populate it, L2-DEC-017), and non-monotonic timestamps. See `docs/L2-REQ.md` L2-RDR-016 through L2-RDR-019 for the per-case rule. |
| `ERROR` | `ERROR`, `SPURIOUS`, or empty | Empty in clean rows. Only populated in inline error mode (`decode --error-mode inline` / `--inline-errors`). |
| `ERROR_CODE` | 4-character uppercase hex code | Empty in clean rows. See `docs/ERROR-CATALOG.md` §6–7 for the full code reference (`0x01xx` DDC, `0x20xx` decoder-assigned). |

---

## 3. Vendor-empty columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`)

Five columns sit between the meaningful payload columns and the trailing diagnostic columns. They are emitted as empty cells by MIE-Decoder in v1, preserved by spec (L2-WRT-013).

| Column | What the vendor uses it for | MIE-Decoder v1 |
|--------|---------------------------|----------------|
| `MUX` | Multiplexer channel identifier on multi-channel DDC cards. | Empty. |
| `TERM_NAME` | Operator-assigned symbolic name for the RT (loaded from a side-channel config file in the vendor tool). | Empty. |
| `IM_GAP` | Inter-message gap (microseconds since the previous transaction on either bus). | Empty. |
| `RCV_GAP` | Receive gap (between command and data on a receive transaction). | Empty. |
| `XMT_GAP` | Transmit gap (between command and status on a transmit transaction). | Empty. |

### Why we keep the columns

Removing them would break the L1-OUT-001 byte-compat contract: any downstream tool that consumes the vendor CSV by column index would point at the wrong field after we'd shifted the columns left. The columns are part of the layout per spec; the cell content is empty by default.

### When the vendor populates them and we don't

If your vendor CSV has values in these columns, MIE-Decoder's output will differ in those cells specifically. That's expected and documented — not a bug. The contract is "column layout matches"; populating the gap-timing and MUX columns is on the roadmap (per ROADMAP backlog, not yet scheduled) but unimplemented today.

To make the diff easier to read, filter the comparison to the meaningful columns:

```bash
# Compare only the columns we're known to populate
awk -F, '{print $1, $2, $3, $36, $37, $39, $40, $41, $42}' OFS=, vendor.csv > vendor-cmp.csv
awk -F, '{print $1, $2, $3, $36, $37, $39, $40, $41, $42}' OFS=, mie.csv    > mie-cmp.csv
diff vendor-cmp.csv mie-cmp.csv
```

(Columns 1–3 are `TIME_STAMP`, `RT`, `MSG`; 36–37 are `STAT`, `CMD`; 39–42 are `BUS`, `DELTA`, `ERROR`, `ERROR_CODE`. Data word columns 4–35 are also typically worth including — adjust as fits your validation needs.)

---

## 4. Line endings

Both implementations emit LF (`\n`) line endings on every platform, including Windows. The vendor tool's output may use CRLF on Windows builds. If your `diff` flags every line as different, normalize line endings first:

```bash
# On Linux / WSL:
dos2unix vendor.csv

# Or in pure POSIX:
tr -d '\r' < vendor.csv > vendor-lf.csv
diff vendor-lf.csv mie.csv
```

A `git diff --ignore-cr-at-eol vendor.csv mie.csv` also handles the trailing-CR case cleanly.

The MIE-Decoder LF-only choice is pinned by L2-WRT-012 and is intentional — keeps CSV byte-exact across host operating systems so the same recording produces the same hash from any decode host.

---

## 5. IRIG day-of-year field — firmware-dependent discrepancy

`docs/MIE-FORMAT.md` §5.1 documents this in the IRIG Upper Word section. Summary:

> Empirical testing has shown a discrepancy between the binary-decoded value and vendor CSV output for the day-of-year field on some DDC card models. The bit extraction is correct per the DDC specification, but the card firmware may use a different encoding (possibly BCD or a different field width).

To make this limitation visible at decode time, the decoder emits a **one-time WARN** per decode the first time it decodes a calendar-locked (non-freerun) IRIG record, pointing back to this section. It is advisory — not a decode failure — and can be silenced with `--log-level ERROR`. Freerun recordings (where day-of-year carries no calendar meaning) do not trigger it.

This is the only known column-content discrepancy. If you see day-of-year mismatch between MIE-Decoder output and vendor CSV for the same recording:

1. **Confirm both tools are looking at the same source file** (no transfer corruption).
2. **Note the card model and firmware version** that produced the recording.
3. **Open an issue with sample bytes** (a 72-byte canonical record from the file plus the corresponding vendor CSV row).

Hour, minute, second, microsecond, and freerun fields are not affected — they decode correctly across all observed card models. The investigation to reverse-engineer the per-firmware day-of-year encoding is tracked in `docs/ROADMAP.md` ("IRIG day-field decoding across DDC card models", Decode correctness section).

---

## 6. Validating a decode matches vendor output

The end-to-end workflow when you want a hard validation that MIE-Decoder reproduces vendor output:

1. **Decode the same recording with both tools.** Use the vendor tool's default settings; for MIE-Decoder use:

   ```bash
   mie-decoder decode flight.mie --inline-errors -o mie.csv
   ```

   Inline error mode matches the vendor tool's behavior of mixing errored and SPURIOUS records into the main CSV. (Separate-mode comparisons would need you to merge MIE-Decoder's two files first.)

2. **Normalize line endings** if your platforms differ (see §4).

3. **Diff with the vendor-empty columns masked out** if your vendor CSV populates `MUX` / `TERM_NAME` / `IM_GAP` / `RCV_GAP` / `XMT_GAP` and MIE-Decoder doesn't (see §3).

4. **Expect zero differences** outside the documented exceptions. If you see a divergence:

   - Day-of-year column → known firmware discrepancy (§5).
   - Empty `MUX` / `TERM_NAME` / gap columns on our side → expected (§3).
   - Anything else → bug. See §7.

5. **For automated comparison** in a regression pipeline, MIE-Decoder ships a cross-implementation conformance suite under `tests/conformance/` that asserts byte-identical CSV between the Rust and Python implementations against checked-in oracles. The oracle generation method (manual validation against vendor output, then committed) is documented in [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) §6.

---

## 7. If you find a divergence outside the documented exceptions

Any column-content mismatch that isn't:

- A `MUX` / `TERM_NAME` / `IM_GAP` / `RCV_GAP` / `XMT_GAP` cell that the vendor populated and we left empty (§3), or
- A line-ending CR/LF difference (§4), or
- A day-of-year discrepancy on the IRIG `TIME_STAMP` (§5)

…is a violation of the L1-OUT-001 byte-compat contract and a bug in MIE-Decoder. To report it:

1. **Capture the divergent row from both CSVs** (one line each, with the column header for context).
2. **Capture the source binary record.** Run `mie-decoder dump <file>.mie --records 1 --offset <byte>` to get the record-aware hex annotation. (For an arbitrary offset rather than the first record, `--raw --offset N --length 256` works.)
3. **Note the card model and firmware version** if known.
4. **Open an issue** with all three. The MIE-Decoder maintainers will reproduce against the conformance suite, add a fixture if missing, and land a fix.

The conformance suite (`tests/conformance/`) is the regression net — every reported divergence that turns out to be a real bug becomes a permanent fixture so it can't silently regress.

---

## 8. Why this contract matters

The L1-OUT-001 byte-compat commitment is load-bearing for adoption:

- **Existing pipelines** that consume vendor CSV can drop in MIE-Decoder without changing any downstream parser.
- **Validation campaigns** can diff MIE-Decoder output against vendor CSV as a sanity check on every new recording.
- **Audits** can show that an alternative decoder produces byte-identical output to a vendor reference.

The contract is enforced at three levels:

1. **L2-WRT-001** pins the column order.
2. **L2-WRT-002 / L2-WRT-003 / L2-WRT-004** pin the per-cell formatting (empty cells for unused fields, 4-char uppercase hex for words, 6-decimal DELTA).
3. **L2-WRT-013** explicitly preserves the vendor-empty columns even though MIE-Decoder doesn't populate them.

Plus the cross-implementation conformance suite, which asserts the Rust and Python implementations agree on every byte for every fixture. If both implementations drift, the suite fails CI; if only one drifts, the suite fails CI louder.

---

## 9. See also

- [`L1-REQ.md`](L1-REQ.md) — `L1-OUT-001` byte-compat contract; `L1-CONF-001` cross-impl conformance suite.
- [`L2-REQ.md`](L2-REQ.md) — `L2-WRT-001` through `L2-WRT-013` writer contract; `L2-CONF-001` through `L2-CONF-005` conformance suite specifics.
- [`MIE-FORMAT.md`](MIE-FORMAT.md) — Comprehensive binary format + CSV column reference.
- [`ERROR-CATALOG.md`](ERROR-CATALOG.md) — `ERROR_CODE` column values (DDC `0x01xx` and decoder `0x20xx` families).
- [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) §6 — Adding a new conformance fixture.
- [`USER-GUIDE.md`](USER-GUIDE.md) §7 — Reading the CSV from an operator perspective.
- [`tests/conformance/`](../tests/conformance/) — The cross-impl regression suite.
