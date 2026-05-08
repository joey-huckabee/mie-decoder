# MIE-Decoder Requirements

**Document ID:** MIE-REQ-001
**Version:** 1.0.0

---

## L1 — System-Level Requirements

L1 requirements define the overall capabilities and constraints of the
MIE-Decoder system.

| ID | Requirement |
|----|------------|
| L1-001 | The system SHALL decode DDC MIE binary recording files containing MIL-STD-1553 bus monitor captures. |
| L1-002 | The system SHALL produce CSV output matching the column layout of DDC's vendor recording software. |
| L1-003 | The system SHALL decode IRIG-format timestamps with microsecond resolution. |
| L1-004 | The system SHALL correctly extract message payloads for both Receive (BC→RT) and Transmit (RT→BC) transfer directions per MIL-STD-1553B wire order. |
| L1-005 | The system SHALL compute the per-RT/MSG inter-arrival time (DELTA) for each decoded message. |
| L1-006 | The system SHALL support both Bus A and Bus B recordings. |
| L1-007 | The system SHALL provide a command-line interface with file input/output, message counting, and configurable logging. |
| L1-008 | The system SHALL handle truncated final records without crashing. |
| L1-009 | The system SHALL use memory-mapped I/O for efficient processing of large files. |
| L1-010 | The system SHALL use pandas for CSV output generation. |
| L1-011 | The system SHALL provide configurable logging at DEBUG, INFO, WARNING, ERROR, and CRITICAL levels. |
| L1-012 | The system SHALL define a custom exception hierarchy for all error conditions. |
| L1-013 | The system SHALL support a strict mode that raises exceptions on invalid records. |
| L1-014 | The system SHALL be implemented as a Python 3.10+ Poetry project with type hints, Google-style docstrings, and pytest test coverage. |

---

## L2 — Module-Level Requirements

L2 requirements allocate L1 capabilities to specific modules.

### L2-DEC: decode.py — Binary Decoding

| ID | Parent | Requirement |
|----|--------|------------|
| L2-DEC-001 | L1-001 | The decode module SHALL decode a 16-bit Type Word into message_type (bits 0–6), bus (bit 7), word_count (bits 8–13), and error flag (bit 14). |
| L2-DEC-002 | L1-003 | The decode module SHALL decode a 3-word IRIG timestamp into day (bits 13–5 of upper word), hour (bits 4–0 of upper word), minute (bits 15–10 of middle word), second (bits 9–4 of middle word), and microsecond (bits 3–0 of middle word concatenated with all 16 bits of lower word). |
| L2-DEC-003 | L1-003 | The decode module SHALL decode the freerun flag from bit 15 of the IRIG upper word. |
| L2-DEC-004 | L1-004 | The decode module SHALL decode a 16-bit Command Word into RT address (bits 15–11), T/R direction (bit 10), subaddress (bits 9–5), and data word count (bits 4–0, where 0 means 32). |
| L2-DEC-005 | L1-001 | The decode module SHALL read little-endian unsigned 16-bit integers and arrays from byte buffers using struct. |
| L2-DEC-006 | L1-001 | The decode module SHALL define MIN_RECORD_BYTES (10) and MIN_RECORD_WORDS (5) constants. |

### L2-MDL: models.py — Data Structures

| ID | Parent | Requirement |
|----|--------|------------|
| L2-MDL-001 | L1-001 | All model dataclasses SHALL use frozen=True and slots=True for immutability and memory efficiency. |
| L2-MDL-002 | L1-006 | The Bus enum SHALL define A=0 and B=1. |
| L2-MDL-003 | L1-004 | The Direction enum SHALL define RECEIVE=0 and TRANSMIT=1. |
| L2-MDL-004 | L1-003 | IrigTimestamp SHALL provide to_total_microseconds() for absolute time comparison. |
| L2-MDL-005 | L1-002 | IrigTimestamp SHALL provide format() producing "DAY:HH:MM:SS.uuuuuu" strings. |
| L2-MDL-006 | L1-005 | MieMessage SHALL provide a delta_key property returning "<RT>:<SA><T\|R>" for DELTA grouping. |
| L2-MDL-007 | L1-002 | MieMessage SHALL provide a msg_label property returning "<SA><T\|R>" for CSV output. |
| L2-MDL-008 | L1-001 | MieMessage SHALL store the file_offset of the source record for diagnostic traceability. |
| L2-MDL-009 | L1-001 | TypeWord SHALL preserve the raw 16-bit value for round-trip fidelity. |
| L2-MDL-010 | L1-001 | CommandWord SHALL preserve the raw 16-bit value for round-trip fidelity. |

### L2-RDR: reader.py — File Reader

| ID | Parent | Requirement |
|----|--------|------------|
| L2-RDR-001 | L1-009 | MieFileReader SHALL use mmap with ACCESS_READ for memory-mapped file access. |
| L2-RDR-002 | L1-008 | MieFileReader SHALL silently skip truncated final records when strict=False. |
| L2-RDR-003 | L1-013 | MieFileReader SHALL raise MieRecordTruncatedError on truncated records when strict=True. |
| L2-RDR-004 | L1-013 | MieFileReader SHALL raise MieInvalidTypeWordError on invalid word counts when strict=True. |
| L2-RDR-005 | L1-012 | MieFileReader SHALL raise MieFileNotFoundError when the file does not exist. |
| L2-RDR-006 | L1-012 | MieFileReader SHALL raise MieFileEmptyError when the file is zero bytes. |
| L2-RDR-007 | L1-004 | MieFileReader SHALL extract Data Words before Status Word for Receive messages. |
| L2-RDR-008 | L1-004 | MieFileReader SHALL extract Status Word before Data Words for Transmit messages. |
| L2-RDR-009 | L1-005 | MieFileReader SHALL compute DELTA as the elapsed time in seconds between the current message and the most recent prior message with the same RT address and MSG identifier. |
| L2-RDR-010 | L1-005 | MieFileReader SHALL set DELTA to 0.0 for the first occurrence of each RT/MSG combination. |
| L2-RDR-011 | L1-011 | MieFileReader SHALL log per-record details at DEBUG level. |
| L2-RDR-012 | L1-011 | MieFileReader SHALL log decode start/complete with message counts at INFO level. |
| L2-RDR-013 | L1-011 | MieFileReader SHALL log progress every 100,000 messages at INFO level. |
| L2-RDR-014 | L1-011 | MieFileReader SHALL log freerun timestamps at WARNING level. |

### L2-WRT: writer.py — CSV Output

| ID | Parent | Requirement |
|----|--------|------------|
| L2-WRT-001 | L1-002 | The writer module SHALL define CSV_HEADER matching DDC vendor column layout: TIME_STAMP, RT, MSG, WD01–WD32, STAT, CMD, MUX, TERM_NAME, BUS, DELTA, IM_GAP, RCV_GAP, XMT_GAP. |
| L2-WRT-002 | L1-002 | The writer module SHALL pad data word columns to 32 with empty strings for messages with fewer than 32 data words. |
| L2-WRT-003 | L1-002 | The writer module SHALL format data words, STAT, and CMD as 4-character uppercase hexadecimal. |
| L2-WRT-004 | L1-002 | The writer module SHALL format DELTA with six decimal places. |
| L2-WRT-005 | L1-010 | The writer module SHALL construct a pandas DataFrame via messages_to_dataframe(). |
| L2-WRT-006 | L1-010 | The writer module SHALL write CSV via a separate dataframe_to_csv() function. |
| L2-WRT-007 | L1-002 | write_csv() SHALL accept output as a file path, text stream, or None (stdout). |
| L2-WRT-008 | L1-002 | write_csv() SHALL return the number of messages written. |
| L2-WRT-009 | L1-012 | dataframe_to_csv() SHALL raise MieWriterError on I/O failure. |
| L2-WRT-010 | L1-002 | CSV_COLUMNS SHALL define both column names and human-readable descriptions. |

### L2-CLI: cli.py — Command-Line Interface

| ID | Parent | Requirement |
|----|--------|------------|
| L2-CLI-001 | L1-007 | The CLI SHALL accept a positional input file path argument. |
| L2-CLI-002 | L1-007 | The CLI SHALL accept an optional -o/--output file path argument. |
| L2-CLI-003 | L1-007 | The CLI SHALL accept a --count flag to print message count to stderr. |
| L2-CLI-004 | L1-011 | The CLI SHALL accept a --log-level argument with choices DEBUG, INFO, WARNING, ERROR, CRITICAL. |
| L2-CLI-005 | L1-007 | The CLI SHALL return exit code 0 on success and 1 on error. |
| L2-CLI-006 | L1-007 | The CLI SHALL print human-readable error messages to stderr on failure. |
| L2-CLI-007 | L1-011 | The CLI SHALL delegate logging configuration to logger.configure_logging(). |

### L2-LOG: logger.py — Logging Configuration

| ID | Parent | Requirement |
|----|--------|------------|
| L2-LOG-001 | L1-011 | configure_logging() SHALL configure the "mie_decoder" logger namespace. |
| L2-LOG-002 | L1-011 | configure_logging() SHALL output to stderr by default. |
| L2-LOG-003 | L1-011 | configure_logging() SHALL use the format "%(asctime)s [%(levelname)-5s] %(name)s: %(message)s". |
| L2-LOG-004 | L1-011 | configure_logging() SHALL raise ValueError for unrecognized log level names. |
| L2-LOG-005 | L1-011 | configure_logging() SHALL remove existing handlers to prevent duplicate output on repeated calls. |

### L2-EXC: exceptions.py — Exception Hierarchy

| ID | Parent | Requirement |
|----|--------|------------|
| L2-EXC-001 | L1-012 | All custom exceptions SHALL inherit from MieDecoderError. |
| L2-EXC-002 | L1-012 | File-level exceptions SHALL inherit from MieFileError. |
| L2-EXC-003 | L1-012 | Record-level exceptions SHALL inherit from MieRecordError and include byte offset. |
| L2-EXC-004 | L1-012 | MieWriterError SHALL wrap the underlying cause exception. |

---

## L3 — Implementation-Level Requirements

L3 requirements specify implementation details and constraints.

| ID | Parent | Requirement |
|----|--------|------------|
| L3-001 | L1-014 | The project SHALL use Poetry for dependency management and packaging. |
| L3-002 | L1-014 | The project SHALL target Python 3.10+. |
| L3-003 | L1-014 | All public functions and classes SHALL have Google-style docstrings. |
| L3-004 | L1-014 | All public functions SHALL have complete type annotations. |
| L3-005 | L1-014 | The project SHALL include unit tests for all decode functions. |
| L3-006 | L1-014 | The project SHALL include end-to-end tests validating CSV output against known-good vendor data. |
| L3-007 | L1-014 | The project SHALL include tests for all custom exceptions. |
| L3-008 | L1-014 | Test fixtures SHALL use binary data derived from empirically validated recordings. |
| L3-009 | L1-010 | pandas SHALL be the only external runtime dependency. |
| L3-010 | L2-DEC-005 | All binary reads SHALL use struct.unpack_from with little-endian format. |
| L3-011 | L2-MDL-001 | All model dataclasses SHALL use __slots__ to minimize per-instance memory. |
| L3-012 | L2-RDR-001 | The reader SHALL not load the entire file into memory; mmap provides virtual memory paging. |
| L3-013 | L1-007 | The __main__.py module SHALL contain only a delegation call to cli.main(). |
| L3-014 | L1-011 | The logger module SHALL define LOGGER_NAME, LOG_FORMAT, and LOG_DATE_FORMAT as module-level constants. |
| L3-015 | L2-WRT-001 | CSV_COLUMNS SHALL be the single source of truth for column ordering and descriptions. |
| L3-016 | L2-EXC-003 | MieInvalidTypeWordError SHALL include the raw Type Word value and decoded word count. |
| L3-017 | L2-EXC-003 | MieRecordTruncatedError SHALL include expected and available byte counts. |
| L3-018 | L2-EXC-004 | MieWriterError SHALL include the output destination name. |

### L2-SYN: sync.py — Record Synchronization

| ID | Parent | Requirement |
|----|--------|------------|
| L2-SYN-001 | L1-008 | validate_record() SHALL check Type Word message type against VALID_MESSAGE_TYPES. |
| L2-SYN-002 | L1-008 | validate_record() SHALL check word count is between minimum and 63. |
| L2-SYN-003 | L1-008 | validate_record() SHALL verify the record does not extend past EOF. |
| L2-SYN-004 | L1-008 | validate_record() SHALL check IRIG timestamp field ranges when format is known. |
| L2-SYN-005 | L1-008 | validate_record() SHALL confirm the next record's Type Word is also valid (look-ahead). |
| L2-SYN-006 | L1-008 | find_first_record() SHALL scan from offset 0 in 2-byte steps to find the first valid record. |
| L2-SYN-007 | L1-008 | find_first_record() SHALL cap the scan at MAX_SCAN_BYTES (64 KB). |
| L2-SYN-008 | L1-008 | find_first_record() SHALL return None if no valid record is found. |
| L2-SYN-009 | L1-008 | recover_sync() SHALL scan forward from the invalid offset in 2-byte steps. |
| L2-SYN-010 | L1-008 | recover_sync() SHALL cap the scan at MAX_SCAN_BYTES from the current offset. |
| L2-SYN-011 | L1-008 | recover_sync() SHALL return None if no valid record is found within the scan window. |
| L2-SYN-012 | L1-011 | find_first_record() SHALL log the header size at INFO level when a header is detected. |
| L2-SYN-013 | L1-011 | recover_sync() SHALL log sync loss at WARNING and recovery at INFO. |

### L2-ERR: Error Record Handling

| ID | Parent | Requirement |
|----|--------|------------|
| L2-ERR-001 | L1-001 | The reader SHALL detect error records by testing Type Word bit 14. |
| L2-ERR-002 | L1-001 | The reader SHALL extract the Error Word as the last word of errored records. |
| L2-ERR-003 | L1-001 | The reader SHALL validate Error Word codes against KNOWN_DDC_ERROR_CODES. |
| L2-ERR-004 | L1-012 | The reader SHALL raise MieUnknownErrorCodeError for unrecognized error codes in strict mode. |
| L2-ERR-005 | L1-001 | The reader SHALL classify SPURIOUS_DATA (0x20) following an error record as continuation (0x2000). |
| L2-ERR-006 | L1-001 | The reader SHALL classify standalone SPURIOUS_DATA as standalone (0x2001). |
| L2-ERR-007 | L1-002 | The writer SHALL include ERROR and ERROR_CODE columns in CSV output. |
| L2-ERR-008 | L1-002 | write_csv_split() SHALL write normal messages to the main file and errored/spurious to a separate file. |
| L2-ERR-009 | L1-007 | The CLI SHALL accept --error-mode with values "separate" (default) and "inline". |
| L2-ERR-010 | L1-002 | The ERROR column SHALL contain "ERROR" for errored records, "SPURIOUS" for spurious data, and empty for normal. |

### L2-CFG: config.py — Configuration Management

| ID | Parent | Requirement |
|----|--------|------------|
| L2-CFG-001 | L1-007 | DecoderConfig SHALL support log_level, time_format, strict, error_mode, filters, and output_format fields. |
| L2-CFG-002 | L1-007 | load_config() SHALL parse TOML files using tomllib (3.11+) or tomli (3.10). |
| L2-CFG-003 | L1-007 | with_overrides() SHALL merge CLI arguments on top of config file values. |
| L2-CFG-004 | L1-007 | Filter CLI arguments SHALL merge with (not replace) config file filters. |
| L2-CFG-005 | L1-007 | The CLI SHALL accept --config to specify a TOML configuration file. |
| L2-CFG-006 | L1-007 | The CLI SHALL accept --exclude-types, --exclude-rts, --exclude-buses, --exclude-subaddresses. |
| L2-CFG-007 | L1-007 | _parse_type_names() SHALL accept both enum names and hex codes (0x02). |

### L2-FLT: filters.py — Message Filtering

| ID | Parent | Requirement |
|----|--------|------------|
| L2-FLT-001 | L1-007 | apply_filters() SHALL yield only messages not matching any exclusion criterion. |
| L2-FLT-002 | L1-007 | FilterConfig.should_exclude() SHALL use OR logic across all filter lists. |
| L2-FLT-003 | L1-007 | apply_filters() SHALL pass through all messages with zero overhead when no filters are active. |
| L2-FLT-004 | L1-011 | apply_filters() SHALL log filter results (passed/excluded counts) at INFO level. |
