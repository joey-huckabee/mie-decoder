# MIE-Decoder — Vendor CSV Alignment & Diffs

Documented column-by-column alignment between MIE-Decoder's CSV output and DDC's vendor-generated CSV. Read this when:

- You're validating that MIE-Decoder produces the same data your existing pipelines expect from the vendor tool.
- You ran `diff` against vendor output and found a mismatch.
- You're integrating MIE-Decoder into a system that previously consumed vendor CSV.

The short version: by spec (`L1-OUT-001`) MIE-Decoder's first **44 columns are the DDC vendor layout**, name-for-name and index-for-index, with two decoder-added columns (`ERROR`, `ERROR_CODE`) appended after them. The cross-implementation conformance suite asserts byte-identical CSV between the Rust and Python implementations; that suite's oracles are derived from validated vendor output. In practice, except for the documented exceptions below, a single `diff` should produce zero lines of difference between MIE-Decoder output and a vendor CSV of the same recording.

---

## 1. Quick verdict

| Category | Status |
|----------|--------|
| Column names | **Match** for all 44 vendor columns; we add 2 more (see §2) |
| Column order | **Match** — vendor columns occupy indices 1–44 exactly; `ERROR` / `ERROR_CODE` are appended at 45–46 |
| Cell formatting (hex width, casing, decimal precision) | **Match** |
| Line endings | **Match** (both produce LF; see §4) |
| Per-row data content for clean records | **Match** |
| Per-row data content for errored / SPURIOUS records | **Match** |
| Placeholder columns `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` | **Empty** (see §3) |
| `MUX` column | **Populated from the file name by default** (L2-WRT-020); empty with `--no-mux` for a vendor-exact diff (see §3) |
| IRIG `TIME_STAMP` day-of-year field | **Firmware-dependent discrepancy** on some DDC card models (see §5) |
| `DELTA` on a **multi-file** decode | **Match** by default (`--delta-scope per-file`, since v2.11.0); `--delta-scope global` diverges deliberately (see §3b) |
| **Row order** for records sharing one `TIME_STAMP` | **May differ** — we sort ties by `RT` then `MSG` (L1-OUT-003); the vendor writes capture order. Restore with `--max-sort-group 1` (see §3a) |

If you find a divergence outside the documented exceptions, **it is a bug** in MIE-Decoder. See §7.

---

## 2. The CSV columns

A decoded CSV is **two blocks**: the DDC vendor layout, then the columns
MIE-Decoder adds on top of it.

**Block 1 — the vendor layout, columns 1–44.** These are the columns the DDC tool
itself emits, in its order:

```
TIME_STAMP, RT, MSG, WD01, WD02, ..., WD32, STAT, CMD, MUX, TERM_NAME, BUS, DELTA, IM_GAP, RCV_GAP, XMT_GAP
```

That's 1 + 1 + 1 + 32 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 + 1 = **44 columns**.
Reordering or renaming any of them breaks the L1-OUT-001 compatibility contract.

**Block 2 — decoder additions, columns 45–46.** Appended after the vendor block:

```
ERROR, ERROR_CODE
```

`ERROR` and `ERROR_CODE` have **no vendor counterpart** — the DDC tool does not
report bus errors as CSV fields at all. Surfacing them is a MIE-Decoder feature
(L2-ERR-002), so they live at the tail where they cannot disturb vendor column
indices. Any column added in a future release goes here too, never inside block 1.

> **Changed in v2.10.0.** Through v2.9.0 these two columns sat *inside* the vendor
> block, between `DELTA` and `IM_GAP`. That silently pushed `IM_GAP` / `RCV_GAP` /
> `XMT_GAP` two positions to the right of their vendor indices, so any positional
> comparison (`awk '{print $42}'`, a column slice, a fixed-width import) past
> `DELTA` was comparing the wrong fields against vendor output — while every
> column *name* still matched, which is why it went unnoticed. If you have
> scripts written against the pre-v2.10.0 column numbers, see §8.

### Cells that match exactly

The following vendor-block columns produce byte-identical content between
MIE-Decoder and the vendor tool for any clean (non-errored, non-spurious) record:

| Column | Format | Notes |
|--------|--------|-------|
| `TIME_STAMP` | `DAY:HH:MM:SS.uuuuuu` (IRIG) or `0xNNNNNNNN` (Standard) | Day-of-year on IRIG has a known firmware discrepancy — see §5. Otherwise identical. |
| `RT` | Integer 0–31, no padding | Empty for SPURIOUS_DATA. |
| `MSG` | `<subaddress><T\|R>` (e.g. `11R`, `22T`) | Empty for SPURIOUS_DATA. |
| `WD01` … `WD32` | 4-character uppercase hex, no `0x` prefix | Unused trailing columns are **empty cells** (not `0000`). |
| `STAT` | 4-character uppercase hex | Empty when not present (some Mode Code formats). |
| `CMD` | 4-character uppercase hex | Empty for SPURIOUS_DATA. |
| `BUS` | Single character `A` or `B` | |
| `DELTA` | `0.000000` (6 decimals) or empty | Empty for SPURIOUS_DATA, uncalibrated Standard-timestamp records (no tick rate configured — supply `standard_tick_rate_hz` to populate it, L2-DEC-017), and non-monotonic timestamps. See `docs/L2-REQ.md` L2-RDR-016 through L2-RDR-019 for the per-case rule. On a **multi-file** decode see §3b — the default scope matches vendor, but `--delta-scope global` deliberately does not. |

### Decoder-added columns (no vendor equivalent)

These have nothing to compare against — a vendor CSV has no such columns, so they
are **expected extra fields**, not divergences:

| Column | Format | Notes |
|--------|--------|-------|
| `ERROR` | `ERROR`, `SPURIOUS`, or empty | Empty in clean rows. Populated in the default inline mode; with `--separate-errors` the errored rows live in the sibling `_errors.csv` instead. |
| `ERROR_CODE` | 4-character uppercase hex code | Empty in clean rows. See `docs/ERROR-CATALOG.md` §6–7 for the full code reference (`0x01xx` DDC, `0x20xx` decoder-assigned). |

---

## 3. Placeholder columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`)

Five columns sit between the meaningful payload columns and the trailing diagnostic columns. Four are emitted as empty cells, preserved by spec (L2-WRT-013). **`MUX` is the exception: by default MIE-Decoder populates it from the input file name (L2-WRT-020)** — so default output is *not* byte-for-byte identical to the vendor CSV when the input name carries that field.

| Column | What the vendor uses it for | MIE-Decoder |
|--------|---------------------------|----------------|
| `MUX` | Multiplexer channel identifier on multi-channel DDC cards. | **Populated from a field of the input file name by default** (L2-WRT-020); empty with `--no-mux` / `[mux] enabled = false`. |
| `TERM_NAME` | Operator-assigned symbolic name for the RT (loaded from a side-channel config file in the vendor tool). | Empty. |
| `IM_GAP` | Inter-message gap (microseconds since the previous transaction on either bus). | Empty. |
| `RCV_GAP` | Receive gap (between command and data on a receive transaction). | Empty. |
| `XMT_GAP` | Transmit gap (between command and status on a transmit transaction). | Empty. |

### Why we keep the columns

Removing them would break the L1-OUT-001 byte-compat contract: any downstream tool that consumes the vendor CSV by column index would point at the wrong field after we'd shifted the columns left. The columns are part of the layout per spec; `MUX` now carries a filename-derived value (its position is unchanged) and the other four stay empty.

### Getting vendor-exact output (`--no-mux`)

For a byte-for-byte diff against a vendor CSV, disable MUX population: pass **`--no-mux`** (or set `[mux] enabled = false` in your config). MIE-Decoder then leaves `MUX` empty like the other four placeholders, and the only remaining differences are the genuinely vendor-populated cells (gap timing, `TERM_NAME`, and any `MUX` values the vendor itself wrote). Those remaining differences are expected and documented — not a bug; the contract is "column layout matches."

To make the diff easier to read, filter the comparison to the meaningful columns.
Since v2.10.0 the vendor columns share indices with the vendor CSV, so the **same**
field list works on both files:

```bash
# Compare only the vendor columns we populate. Same indices on both sides.
FIELDS='$1, $2, $3, $36, $37, $40, $41'
awk -F, "{print $FIELDS}" OFS=, vendor.csv > vendor-cmp.csv
awk -F, "{print $FIELDS}" OFS=, mie.csv    > mie-cmp.csv
diff vendor-cmp.csv mie-cmp.csv
```

(Columns 1–3 are `TIME_STAMP`, `RT`, `MSG`; 36–37 are `STAT`, `CMD`; 40–41 are
`BUS`, `DELTA`. Data word columns 4–35 are also typically worth including — adjust
as fits your validation needs. 38 is `MUX` and 39 is `TERM_NAME`; 42–44 are the
`IM_GAP` / `RCV_GAP` / `XMT_GAP` placeholders. The full 1-based index map is in
[`EXAMPLES.md`](EXAMPLES.md) §12.)

Because `ERROR` / `ERROR_CODE` are now at 45–46, a simpler approach also works —
truncate our output to the vendor block and compare whole rows:

```bash
cut -d, -f1-44 mie.csv > mie-vendor-block.csv
diff vendor.csv mie-vendor-block.csv
```

That was not possible before v2.10.0, when the two decoder columns were embedded
mid-row.

---

## 3a. Row order (`--max-sort-group 1`)

Since v2.9.0, MIE-Decoder writes rows in a **canonical order** (L1-OUT-003):
ascending `TIME_STAMP`, then ascending `RT`, then `MSG` (subaddress ascending,
`R` before `T`). The vendor tool writes rows in **capture order** — the order the
DDC card wrote them. For records with distinct timestamps the two agree, because
capture order *is* time order. They can differ only for records that share one
`TIME_STAMP` to the microsecond.

**When that actually happens.** A 1553 bus carries one transaction at a time, so
on a single-bus recording it essentially never does. The realistic case is a
**dual-bus recording**: bus A and bus B transactions are genuinely concurrent and
can land on the same microsecond with different RTs. There, our row order may
differ from the vendor's while every cell still matches.

**For a byte-for-byte diff, disable reordering:**

```bash
mie-decoder decode recording.mie -o mie.csv --no-mux --max-sort-group 1
```

`--max-sort-group 1` makes every record its own sort group, so nothing is
reordered and the output is raw capture order — exactly what the vendor writes.
Combine it with `--no-mux` (§3) for a full vendor-exact decode. The equivalent
config keys are `[output] max_sort_group = 1` and `[mux] enabled = false`.

**A row-order-only difference is not a bug.** If a diff shows the same set of
rows in a different order within one timestamp, and re-running with
`--max-sort-group 1` makes the diff clean, that is this documented exception —
not a divergence to report under §7. What *would* be a bug: a row-order
difference that persists with `--max-sort-group 1`, a row-order difference
between records with *different* timestamps, or any cell-content difference.

---

## 3b. `DELTA` on a multi-file decode (`--delta-scope`)

The DDC vendor tool has **no merge feature** — it decodes one recording at a
time, so its `DELTA` is always measured within a single file.

Since v2.11.0 that is also MIE-Decoder's default (`--delta-scope per-file`), so a
merged decode's `DELTA` matches vendor output for every record: each gap is to
the previous same-RT/MSG record from that record's *own* file, which is by
construction the value that file produces decoded alone.

`--delta-scope global` measures across the merged timeline instead. That is a
deliberate divergence, not a bug — it answers "how long since *any* recorder last
saw this key" rather than "how long since *this* recorder did". If you are diffing
against vendor CSV, leave the scope at its default.

> **Before v2.11.0** merged `DELTA` was always global, so any multi-file decode
> diffed against a per-file vendor CSV showed differences on every RT/MSG key
> present in more than one input. Keys unique to one file were unaffected.

A single-input decode is identical under either scope, so this section does not
apply to the ordinary one-file vendor comparison.

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

1. **Decode the same recording with both tools.** Use the vendor tool's default settings; for MIE-Decoder use the two vendor-exact flags:

   ```bash
   mie-decoder decode flight.mie -o mie.csv --no-mux --max-sort-group 1
   ```

   `--no-mux` leaves the `MUX` column empty like the vendor's other placeholders (§3), and `--max-sort-group 1` disables canonical row ordering so rows stay in capture order (§3a). Without those two flags a clean decode will still show expected differences, and you would be chasing documented behavior.

   Inline error mode matches the vendor tool's behavior of mixing errored and SPURIOUS records into the main CSV. (Separate-mode comparisons would need you to merge MIE-Decoder's two files first.)

2. **Normalize line endings** if your platforms differ (see §4).

3. **Diff with the vendor-empty columns masked out** if your vendor CSV populates `MUX` / `TERM_NAME` / `IM_GAP` / `RCV_GAP` / `XMT_GAP` and MIE-Decoder doesn't (see §3).

4. **Expect zero differences** outside the documented exceptions. If you see a divergence:

   - Day-of-year column → known firmware discrepancy (§5).
   - `MUX` column → populated from the file name by default; pass `--no-mux` for a vendor-exact diff (§3).
   - Empty `TERM_NAME` / gap columns on our side → expected (§3).
   - Same rows, different order within one `TIME_STAMP` → canonical row order; pass `--max-sort-group 1` (§3a).
   - Two extra columns at the end of our rows (`ERROR`, `ERROR_CODE`) → expected; they have no vendor counterpart (§2). Compare `cut -d, -f1-44` of our output against the vendor file.
   - `DELTA` differs on a **multi-file** decode → check `--delta-scope`; the default (`per-file`) matches vendor, `global` does not (§3b).
   - Anything else → bug. See §7.

5. **For automated comparison** in a regression pipeline, MIE-Decoder ships a cross-implementation conformance suite under `tests/conformance/` that asserts byte-identical CSV between the Rust and Python implementations against checked-in oracles. The oracle generation method (manual validation against vendor output, then committed) is documented in [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) §6.

---

## 7. If you find a divergence outside the documented exceptions

Any column-content mismatch that isn't:

- A `MUX` / `TERM_NAME` / `IM_GAP` / `RCV_GAP` / `XMT_GAP` cell that the vendor populated and we left empty (§3), or
- The presence of the `ERROR` / `ERROR_CODE` columns at indices 45–46, which the vendor CSV does not have at all (§2), or
- A row-order difference *within a single `TIME_STAMP`* that goes away under `--max-sort-group 1` (§3a), or
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

1. **L2-WRT-001** pins the column order, including the rule that decoder-added columns are appended *after* the 44-column vendor block and never interleaved within it.
2. **L2-WRT-002 / L2-WRT-003 / L2-WRT-004** pin the per-cell formatting (empty cells for unused fields, 4-char uppercase hex for words, 6-decimal DELTA).
3. **L2-WRT-013** explicitly preserves the vendor-empty columns even though MIE-Decoder doesn't populate them.

Plus the cross-implementation conformance suite, which asserts the Rust and Python implementations agree on every byte for every fixture. If both implementations drift, the suite fails CI; if only one drifts, the suite fails CI louder.

### Migrating scripts written against pre-v2.10.0 column numbers

Through v2.9.0, `ERROR` and `ERROR_CODE` sat at indices 42–43, pushing the three
gap columns to 44–46. From v2.10.0 the gaps are at 42–44 (their vendor positions)
and the two decoder columns are at 45–46.

| Column | Index ≤ v2.9.0 | Index ≥ v2.10.0 |
|---|---|---|
| `DELTA` | 41 | 41 *(unchanged)* |
| `ERROR` | 42 | **45** |
| `ERROR_CODE` | 43 | **46** |
| `IM_GAP` | 44 | **42** |
| `RCV_GAP` | 45 | **43** |
| `XMT_GAP` | 46 | **44** |

Everything at index ≤ 41 is unaffected, which covers `TIME_STAMP`, `RT`, `MSG`,
all 32 data words, `STAT`, `CMD`, `MUX`, `TERM_NAME`, `BUS`, and `DELTA` — so most
scripts need no change at all. Only code that referenced the six columns above by
number is affected.

The durable fix is to resolve columns **by header name** rather than by index:

```bash
# Resolve ERROR_CODE by name, whatever position it occupies
awk -F, 'NR==1 {for (i=1; i<=NF; i++) if ($i=="ERROR_CODE") c=i; next} $c!="" {print}' mie.csv
```

```python
import csv
with open("mie.csv", newline="") as fh:
    for row in csv.DictReader(fh):      # DictReader is index-independent
        if row["ERROR_CODE"]:
            ...
```

Name-based access is what the decoder's own tests now use, precisely because the
positional variants are what let this bug hide for so long.

---

## 9. See also

- [`L1-REQ.md`](L1-REQ.md) — `L1-OUT-001` byte-compat contract; `L1-CONF-001` cross-impl conformance suite.
- [`L2-REQ.md`](L2-REQ.md) — `L2-WRT-001` through `L2-WRT-013` writer contract; `L2-CONF-001` through `L2-CONF-005` conformance suite specifics.
- [`MIE-FORMAT.md`](MIE-FORMAT.md) — Comprehensive binary format + CSV column reference.
- [`ERROR-CATALOG.md`](ERROR-CATALOG.md) — `ERROR_CODE` column values (DDC `0x01xx` and decoder `0x20xx` families).
- [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) §6 — Adding a new conformance fixture.
- [`USER-GUIDE.md`](USER-GUIDE.md) §7 — Reading the CSV from an operator perspective.
- [`tests/conformance/`](../tests/conformance/) — The cross-impl regression suite.
