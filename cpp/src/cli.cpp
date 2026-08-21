// SPDX-License-Identifier: Apache-2.0

#define MIE_LOG_MODULE "mie_decoder::cli"

#include "mie/cli.hpp"

#include <cstdio>
#include <cstdlib>

#include "mie/config.hpp"
#include "mie/error.hpp"
#include "mie/filter.hpp"
#include "mie/log.hpp"
#include "mie/order.hpp"
#include "mie/platform.hpp"
#include "mie/reader.hpp"
#include "mie/text.hpp"
#include "mie/writer.hpp"

namespace mie {
namespace cli {

namespace {

const char* const kVersion = "2.12.0";

const char* const kHelp =
    "mie-decoder \xE2\x80\x94 DDC MIL-STD-1553 MIE binary decoder\n"
    "\n"
    "USAGE:\n"
    "    mie-decoder [GLOBAL] <COMMAND> [OPTIONS]\n"
    "\n"
    "COMMANDS:\n"
    "    decode <FILE>     Decode a recording to CSV\n"
    "    count <FILE>      Count decodable records\n"
    "\n"
    "GLOBAL OPTIONS:\n"
    "    --config <PATH>   TOML configuration file\n"
    "    --log-level <L>   DEBUG, INFO, WARNING, ERROR, CRITICAL, OFF\n"
    "    -h, --help        Print this help\n"
    "    -V, --version     Print the version\n"
    "\n"
    "DECODE OPTIONS:\n"
    "    -o, --output <PATH>          Destination CSV; '-' writes to stdout\n"
    "    --strict                     Reject on the first anomaly\n"
    "    --time-format <F>            auto, irig, standard\n"
    "    --separate-errors            Errored rows to <stem>_errors.csv\n"
    "    --allow-partial              Commit <dest>.partial on sync loss\n"
    "    --no-clobber                 Refuse to overwrite the destination\n"
    "    --format <F>                 Output format (csv)\n"
    "    --detect-records <N>         Timestamp-probe size, 1-32\n"
    "    --lookahead-records <N>      Validation look-ahead depth, 1-32\n"
    "    --standard-tick-rate-hz <H>  Calibrate Standard timestamps\n"
    "    --max-sort-group <N>         Canonical-order run cap, 1-1048576\n"
    "    --no-mux                     Leave the MUX column empty\n"
    "    --mux-delimiter <S>          MUX field delimiter\n"
    "    --mux-field <N>              MUX field index; negative counts back\n"
    "    --exclude-types <LIST>       Drop these message types\n"
    "    --exclude-rts <LIST>         Drop these RT addresses\n"
    "    --exclude-buses <LIST>       Drop these buses (A, B)\n"
    "    --exclude-subaddresses <L>   Drop these subaddresses\n"
    "    --include-types <LIST>       Keep only these message types\n"
    "    --include-rts <LIST>         Keep only these RT addresses\n"
    "    --include-buses <LIST>       Keep only these buses\n"
    "    --include-subaddresses <L>   Keep only these subaddresses\n"
    "\n"
    "EXIT CODES:\n"
    "    0 success   1 runtime   2 no records   3 sync loss\n"
    "    4 usage     5 config    6 merge-incompatible\n";

/// Flags this build parses but cannot serve yet. Named individually so the
/// message can say WHICH part is missing -- an operator copying a working Rust
/// invocation should learn that the merge is unported, not doubt the flag name.
const char* const kDeferredFlags[] = {
    "--manifest", "--glob", "--delta-scope", "--collapse-duplicates", "--collapse-window-us",
};

/// A failure carrying the exit code it maps to, so a config problem (5) is not
/// flattened into a generic runtime error (1).
struct CliError {
    int code;
    std::string message;

    CliError(int code_, const std::string& message_) : code(code_), message(message_) {}
};

CliError usage_error(const std::string& message) { return CliError(EXIT_USAGE, message); }
CliError config_error(const std::string& message) { return CliError(EXIT_CONFIG, message); }
CliError runtime_error_(const std::string& message) { return CliError(EXIT_RUNTIME, message); }

/// Map a decode-time MieError to its exit class.
///
/// Shared by `decode` and `count` so the two agree: a wrong-file rejection is
/// exit 2 on both. Rust records that `count` once flattened every reader error
/// to exit 1, which made a script's "is this an MIE file?" check depend on
/// which subcommand it happened to use.
int exit_code_for(const MieError& error) {
    switch (error.kind()) {
        case KIND_NO_VALID_RECORDS:
        case KIND_HOMOGENEOUS_PAYLOAD:
        case KIND_TIMESTAMP_FORMAT_MISMATCH: return EXIT_NO_RECORDS;
        default: return EXIT_RUNTIME;
    }
}

bool write_out(std::FILE* stream, const std::string& text) {
    if (stream == NULL) {
        return false;
    }
    const bool ok = std::fwrite(text.data(), 1, text.size(), stream) == text.size();
    // Flushed here rather than at exit: `count` writes its integer to `out` and
    // its sentence to `err`, and a caller reading both wants them complete the
    // moment run() returns, not whenever the CRT gets around to it.
    (void)std::fflush(stream);
    return ok;
}

// ---------------------------------------------------------------------------
// Argument cursor
// ---------------------------------------------------------------------------

/// A cursor over the argument list that understands `--flag value` and
/// `--flag=value` as one thing.
///
/// The Rust parser spells both forms out at every flag, which is twenty-odd
/// near-identical match arms and twenty-odd chances for one of them to drift.
/// Handling it once here is the same behaviour with one place to be wrong.
class ArgReader {
  public:
    explicit ArgReader(const std::vector<std::string>& args) : args_(args), at_(0) {}

    bool at_end() const { return at_ >= args_.size(); }
    const std::string& peek() const { return args_[at_]; }
    void advance() { at_ += 1; }

    /// Consume the current token if it is exactly `name`.
    bool take_flag(const char* name) {
        if (at_end() || args_[at_] != name) {
            return false;
        }
        at_ += 1;
        return true;
    }

    /// Consume `name value` or `name=value`, putting the value in `out`.
    ///
    /// Throws when the separated form runs off the end: a trailing
    /// `--output` with nothing after it is a usage error, not an empty path.
    bool take_value(const char* name, std::string& out) {
        if (at_end()) {
            return false;
        }
        const std::string& token = args_[at_];
        const std::string prefix = std::string(name) + "=";
        if (token.size() > prefix.size() && token.compare(0, prefix.size(), prefix) == 0) {
            out = token.substr(prefix.size());
            at_ += 1;
            return true;
        }
        if (token != name) {
            return false;
        }
        at_ += 1;
        if (at_end()) {
            throw usage_error(std::string(name) + " requires a value");
        }
        out = args_[at_];
        at_ += 1;
        return true;
    }

    /// `take_value` for a flag with a short alias, e.g. `-o` / `--output`.
    bool take_value(const char* short_name, const char* name, std::string& out) {
        if (!at_end() && args_[at_] == short_name) {
            at_ += 1;
            if (at_end()) {
                throw usage_error(std::string(name) + " requires a value");
            }
            out = args_[at_];
            at_ += 1;
            return true;
        }
        return take_value(name, out);
    }

  private:
    std::vector<std::string> args_;
    std::size_t at_;
};

// ---------------------------------------------------------------------------
// Value parsers
// ---------------------------------------------------------------------------

/// A bounded integer flag value. Rejects trailing junk, which `atoi` accepts.
int64_t parse_integer(const std::string& text, const char* flag) {
    if (text.empty()) {
        throw usage_error(std::string(flag) + " requires a number, got an empty value");
    }
    char* end = 0;
    const long long value = std::strtoll(text.c_str(), &end, 10);
    if (end == 0 || *end != '\0') {
        throw usage_error(std::string(flag) + " requires a number, got \"" + text + "\"");
    }
    return static_cast<int64_t>(value);
}

std::size_t parse_ranged(const std::string& text, const char* flag, std::size_t lo,
                         std::size_t hi) {
    const int64_t value = parse_integer(text, flag);
    if (value < static_cast<int64_t>(lo) || value > static_cast<int64_t>(hi)) {
        throw usage_error(
            std::string(flag) + " must be in [" + text::decimal(static_cast<uint64_t>(lo)) + ", " +
            text::decimal(static_cast<uint64_t>(hi)) + "], got " + text::decimal_signed(value));
    }
    return static_cast<std::size_t>(value);
}

double parse_tick_rate(const std::string& text) {
    if (text.empty()) {
        throw usage_error("--standard-tick-rate-hz requires a value");
    }
    char* end = 0;
    const double value = std::strtod(text.c_str(), &end);
    if (end == 0 || *end != '\0') {
        throw usage_error("--standard-tick-rate-hz requires a number, got \"" + text + "\"");
    }
    if (!(value > 0.0)) {
        // Also catches NaN, for which every comparison is false.
        throw usage_error("--standard-tick-rate-hz must be greater than 0, got \"" + text + "\"");
    }
    return value;
}

/// Split a comma-separated filter list, dropping empty pieces so a trailing
/// comma is tolerated rather than producing an unnamed element.
std::vector<std::string> split_csv(const std::string& text) {
    std::vector<std::string> out;
    std::string current;
    for (std::size_t i = 0; i < text.size(); ++i) {
        if (text[i] == ',') {
            const std::string piece = text::trim_ascii_blank(current);
            if (!piece.empty()) {
                out.push_back(piece);
            }
            current.clear();
        } else {
            current += text[i];
        }
    }
    const std::string piece = text::trim_ascii_blank(current);
    if (!piece.empty()) {
        out.push_back(piece);
    }
    return out;
}

/// Parse a filter-list element with a parser shared with the config loader,
/// reporting failure as a USAGE error that names the flag.
///
/// `parse_type_name` / `parse_bus_name` signal with ConfigError, which is exit
/// 5. Reached through a CLI flag the same bad value is exit 4 -- the operator
/// mistyped an argument, not their config file -- and the message should say
/// which flag rather than leaving them to guess. Without this the ConfigError
/// also escaped every handler in `run()` and aborted the process.
uint8_t parse_type_flag(const std::string& text, const char* flag) {
    try {
        return parse_type_name(text);
    } catch (const ConfigError& error) {
        throw usage_error(std::string(flag) + ": " + error.message());
    }
}

Bus parse_bus_flag(const std::string& text, const char* flag) {
    try {
        return parse_bus_name(text);
    } catch (const ConfigError& error) {
        throw usage_error(std::string(flag) + ": " + error.message());
    }
}

/// A filter list element that must be a 0-31 wire field.
uint8_t parse_small(const std::string& text, const char* flag) {
    const int64_t value = parse_integer(text, flag);
    if (value < 0 || value > 31) {
        throw usage_error(std::string(flag) + " values must be in [0, 31], got " +
                          text::decimal_signed(value));
    }
    return static_cast<uint8_t>(value);
}

// ---------------------------------------------------------------------------
// Parsed arguments
// ---------------------------------------------------------------------------

struct GlobalArgs {
    Optional<std::string> config;
    Optional<std::string> log_level;

    GlobalArgs() : config(), log_level() {}
};

struct DecodeArgs {
    std::vector<std::string> inputs;
    Optional<std::string> output;
    ConfigOverrides overrides;

    DecodeArgs() : inputs(), output(), overrides() {}
};

void reject_deferred(const std::string& token) {
    for (std::size_t i = 0; i < sizeof(kDeferredFlags) / sizeof(kDeferredFlags[0]); ++i) {
        const std::string flag = kDeferredFlags[i];
        if (token == flag || token.compare(0, flag.size() + 1, flag + std::string("=")) == 0) {
            throw usage_error(token +
                              " is not available in this build: the multi-file merge is not "
                              "ported yet. Use the Rust or Python implementation for it.");
        }
    }
}

/// Parse everything after `decode`.
DecodeArgs parse_decode(ArgReader& reader) {
    DecodeArgs args;
    std::string value;

    while (!reader.at_end()) {
        const std::string token = reader.peek();
        reject_deferred(token);

        if (reader.take_value("-o", "--output", value)) {
            args.output = value;
        } else if (reader.take_flag("--separate-errors")) {
            // Presence-only. Absence must NOT contribute `inline`, or it would
            // clobber a `separate` the operator set in their config file.
            args.overrides.error_mode = ERROR_MODE_SEPARATE;
        } else if (reader.take_flag("--no-clobber")) {
            args.overrides.no_clobber = true;
        } else if (reader.take_flag("--allow-partial")) {
            args.overrides.allow_partial = true;
        } else if (reader.take_flag("--strict")) {
            args.overrides.strict = true;
        } else if (reader.take_flag("--no-mux")) {
            args.overrides.mux_enabled = false;
        } else if (reader.take_value("--time-format", value)) {
            TimestampFormat format = TIMESTAMP_AUTO;
            if (!timestamp_format_from_name(value, format)) {
                throw usage_error("--time-format must be auto, irig or standard, got \"" + value +
                                  "\"");
            }
            args.overrides.time_format = format;
        } else if (reader.take_value("--format", value)) {
            args.overrides.output_format = value;
        } else if (reader.take_value("--detect-records", value)) {
            args.overrides.detect_records =
                parse_ranged(value, "--detect-records", DETECT_RECORDS_MIN, DETECT_RECORDS_MAX);
        } else if (reader.take_value("--lookahead-records", value)) {
            args.overrides.lookahead_records = parse_ranged(
                value, "--lookahead-records", LOOKAHEAD_RECORDS_MIN, LOOKAHEAD_RECORDS_MAX);
        } else if (reader.take_value("--standard-tick-rate-hz", value)) {
            args.overrides.standard_tick_rate_hz = parse_tick_rate(value);
        } else if (reader.take_value("--max-sort-group", value)) {
            args.overrides.max_sort_group =
                parse_ranged(value, "--max-sort-group", MAX_SORT_GROUP_MIN, MAX_SORT_GROUP_MAX);
        } else if (reader.take_value("--mux-delimiter", value)) {
            if (value.empty()) {
                throw usage_error("--mux-delimiter must not be empty");
            }
            args.overrides.mux_delimiter = value;
        } else if (reader.take_value("--mux-field", value)) {
            args.overrides.mux_field = parse_integer(value, "--mux-field");
        } else if (reader.take_value("--exclude-types", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.exclude_types.push_back(
                    parse_type_flag(items[i], "--exclude-types"));
            }
        } else if (reader.take_value("--include-types", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.include_types.push_back(
                    parse_type_flag(items[i], "--include-types"));
            }
        } else if (reader.take_value("--exclude-buses", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.exclude_buses.push_back(
                    parse_bus_flag(items[i], "--exclude-buses"));
            }
        } else if (reader.take_value("--include-buses", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.include_buses.push_back(
                    parse_bus_flag(items[i], "--include-buses"));
            }
        } else if (reader.take_value("--exclude-rts", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.exclude_rts.push_back(
                    parse_small(items[i], "--exclude-rts"));
            }
        } else if (reader.take_value("--include-rts", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.include_rts.push_back(
                    parse_small(items[i], "--include-rts"));
            }
        } else if (reader.take_value("--exclude-subaddresses", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.exclude_subaddresses.push_back(
                    parse_small(items[i], "--exclude-subaddresses"));
            }
        } else if (reader.take_value("--include-subaddresses", value)) {
            const std::vector<std::string> items = split_csv(value);
            for (std::size_t i = 0; i < items.size(); ++i) {
                args.overrides.filters.include_subaddresses.push_back(
                    parse_small(items[i], "--include-subaddresses"));
            }
        } else if (!token.empty() && token[0] == '-' && token != "-") {
            throw usage_error("unknown option " + token);
        } else {
            args.inputs.push_back(token);
            reader.advance();
        }
    }

    if (args.inputs.empty()) {
        throw usage_error("decode requires an input file");
    }
    if (args.inputs.size() > 1) {
        throw usage_error(
            "decoding more than one input requires the multi-file merge, which is not "
            "ported yet. Use the Rust or Python implementation for it.");
    }
    return args;
}

std::string parse_count(ArgReader& reader) {
    std::vector<std::string> inputs;
    while (!reader.at_end()) {
        const std::string token = reader.peek();
        reject_deferred(token);
        if (!token.empty() && token[0] == '-' && token != "-") {
            throw usage_error("unknown option " + token);
        }
        inputs.push_back(token);
        reader.advance();
    }
    if (inputs.empty()) {
        throw usage_error("count requires an input file");
    }
    if (inputs.size() > 1) {
        throw usage_error("count takes exactly one input file");
    }
    return inputs[0];
}

// ---------------------------------------------------------------------------
// Runners
// ---------------------------------------------------------------------------

void apply_log_level(const char* source, const std::string& value) {
    log::Level level = log::LEVEL_WARN;
    if (!log::level_from_name(value, level)) {
        throw usage_error(std::string("invalid ") + source + " \"" + value +
                          "\": valid levels are DEBUG, INFO, WARNING, WARN, ERROR, CRITICAL, "
                          "OFF");
    }
    log::set_level(level);
}

DecoderConfig resolve_config(const GlobalArgs& globals) {
    DecoderConfig config;
    try {
        config = load_config(globals.config);
    } catch (const ConfigError& error) {
        throw config_error(error.message());
    }
    // The config file's level was validated at load time, so this cannot fail
    // for a file-sourced value.
    apply_log_level("[logging].level", config.log_level);
    // The CLI wins, and is applied last for that reason.
    if (globals.log_level.has_value()) {
        apply_log_level("--log-level", globals.log_level.value());
    }
    return config;
}

ReaderOptions reader_options(const DecoderConfig& config) {
    ReaderOptions options;
    options.strict = config.strict;
    options.time_format = config.time_format;
    options.detect_records = config.detect_records;
    options.lookahead_records = config.lookahead_records;
    options.standard_tick_rate_hz = config.standard_tick_rate_hz;
    options.mux_enabled = config.mux_enabled;
    options.mux_delimiter = config.mux_delimiter;
    options.mux_field = config.mux_field;
    return options;
}

/// Adapts a RecordIter to the pipeline's MessageSource contract.
///
/// Lives here rather than in `reader.hpp` because it is the CLI that knows both
/// types: making the reader implement MessageSource would give it a vtable and
/// a dependency on the pipeline contract for the benefit of one caller.
class ReaderSource : public MessageSource {
  public:
    explicit ReaderSource(RecordIter& iter) : iter_(&iter) {}
    bool next(MieMessage& out) override { return iter_->next(out); }

  private:
    RecordIter* iter_;
};

/// The exit-class summary, and the code that goes with a successful write.
int classify_decode_exit(const WriteOutcome& outcome, uint64_t sync_losses, bool empty_recording) {
    // L1-EXIT-010: report `empty-recording` only when the decode really
    // produced nothing AND the reader saw the end-of-records terminator. A file
    // that yielded zero rows because a filter dropped them all is `complete`.
    const bool empty = empty_recording && !outcome.partial.has_value() &&
                       outcome.normal_count == 0 && outcome.error_count == 0;
    std::string classification;
    if (outcome.partial.has_value()) {
        classification = "partial-unrecoverable";
    } else if (empty) {
        classification = "empty-recording";
    } else if (sync_losses > 0) {
        classification = "partial-recovered";
    } else {
        classification = "complete";
    }
    MIE_LOG_INFO("decode exit class: " + classification +
                 " (sync_losses=" + text::decimal(sync_losses) + ")");
    return EXIT_OK;
}

/// Report a decode failure and choose its exit class.
int report_decode_failure(const Streams& streams, const MieError& error, uint64_t sync_losses) {
    if (error.is_broken_pipe()) {
        // L2-WRT-018: `mie-decoder decode x.mie | head` is a normal thing to
        // type, not a failure.
        MIE_LOG_INFO("decode exit class: complete (broken-pipe on stdout)");
        return EXIT_OK;
    }

    MIE_LOG_ERROR(error.message());
    (void)write_out(streams.err, "Error: " + error.message() + "\n");

    const int code = exit_code_for(error);
    if (code == EXIT_NO_RECORDS) {
        MIE_LOG_INFO("decode exit class: no-records");
        return code;
    }
    if (error.kind() == KIND_UNRECOVERABLE_SYNC_LOSS) {
        MIE_LOG_INFO(
            "decode exit class: partial-unrecoverable (sync_losses=" + text::decimal(sync_losses) +
            "); pass --allow-partial to preserve the rows decoded so far");
        return EXIT_SYNC_LOSS;
    }
    if (error.kind() == KIND_INCOMPATIBLE_MERGE_INPUTS) {
        MIE_LOG_INFO("decode exit class: merge-incompatible");
        return EXIT_MERGE_INCOMPATIBLE;
    }
    return EXIT_RUNTIME;
}

int run_decode(const Streams& streams, const GlobalArgs& globals, DecodeArgs& args) {
    const DecoderConfig base = resolve_config(globals);
    if (globals.log_level.has_value()) {
        args.overrides.log_level = globals.log_level.value();
    }
    const DecoderConfig config = with_overrides(base, args.overrides);

    if (config.output_format != "csv") {
        throw runtime_error_("output format \"" + config.output_format +
                             "\" is not supported (only csv)");
    }

    const std::string& input = args.inputs[0];

    MieFileReader file_reader;
    try {
        file_reader.open(input, reader_options(config));
    } catch (const MieError& error) {
        throw CliError(exit_code_for(error), error.message());
    }
    MIE_LOG_INFO("opened " + file_reader.path() + " (" + text::decimal(file_reader.file_size()) +
                 " bytes)");

    // `-` means stdout, which has no filesystem identity: no collision check,
    // no clobber guard, and `--allow-partial` cannot name a `.partial` beside
    // it.
    const bool to_stdout = args.output.has_value() && args.output.value() == "-";
    Optional<std::string> destination;
    if (args.output.has_value() && !to_stdout) {
        destination = args.output.value();
    }

    WriteOptions write_options;
    write_options.no_clobber = config.no_clobber;
    write_options.allow_partial = config.allow_partial;
    // So `-o -` lands on the same stream everything else reports on.
    write_options.stdout_stream = streams.out;
    if (destination.has_value()) {
        write_options.input_path = input;
    }

    // The pipeline, assembled. Each stage wraps the one before it, so the
    // writer never learns whether filtering or ordering happened.
    RecordIter iter = file_reader.iter();
    ReaderSource from_reader(iter);
    FilteredSource filtered(from_reader, config.filters);
    OrderedSource ordered(filtered, config.max_sort_group);

    if (!destination.has_value() && config.error_mode == ERROR_MODE_SEPARATE) {
        // L3-RS-009: separate mode needs a file path to derive the errors name
        // from, so stdout forces inline. Warned rather than done quietly --
        // an operator who asked for a split file and silently got one combined
        // stream would only find out by reading the output.
        MIE_LOG_WARN("stdout output forces inline error mode");
    }

    WriteOutcome outcome;
    try {
        if (!destination.has_value()) {
            outcome = write_csv(ordered, Optional<std::string>(), write_options);
        } else if (config.error_mode == ERROR_MODE_SEPARATE) {
            outcome = write_csv_split(ordered, destination.value(), write_options);
        } else {
            outcome = write_csv(ordered, destination, write_options);
        }
    } catch (const MieError& error) {
        return report_decode_failure(streams, error, file_reader.sync_losses());
    }

    return classify_decode_exit(outcome, file_reader.sync_losses(), file_reader.empty_recording());
}

int run_count(const Streams& streams, const GlobalArgs& globals, const std::string& input) {
    const DecoderConfig config = resolve_config(globals);

    MieFileReader file_reader;
    try {
        file_reader.open(input, reader_options(config));
    } catch (const MieError& error) {
        throw CliError(exit_code_for(error), error.message());
    }

    uint64_t count = 0;
    {
        // The config's filters apply, matching decode: an operator who wants a
        // raw count omits [filter] from their config rather than needing a
        // second subcommand.
        RecordIter iter = file_reader.iter();
        ReaderSource from_reader(iter);
        FilteredSource filtered(from_reader, config.filters);
        MieMessage message;
        try {
            while (filtered.next(message)) {
                count += 1;
            }
        } catch (const MieError& error) {
            throw CliError(exit_code_for(error), error.message());
        }
    }

    // The integer alone on stdout -- that is the machine-readable answer, and
    // a script doing `n=$(mie-decoder count x.mie)` must not have to strip
    // prose. The human sentence goes to stderr, ungated by --log-level so an
    // interactive operator sees it without opting into INFO.
    (void)write_out(streams.out, text::decimal(count) + "\n");
    if (file_reader.empty_recording()) {
        (void)write_out(streams.err,
                        "no records in " + file_reader.path() +
                            " (empty recording \xE2\x80\x94 opens on the end-of-records "
                            "terminator)\n");
    } else {
        (void)write_out(streams.err, "counted " + text::decimal(count) + " messages in " +
                                         file_reader.path() + "\n");
    }
    return EXIT_OK;
}

bool is_version_flag(const std::string& arg) {
    return arg == "-V" || arg == "-v" || text::equals_ignoring_ascii_case(arg, "--version");
}

bool is_help_flag(const std::string& arg) {
    return arg == "-h" || text::equals_ignoring_ascii_case(arg, "--help");
}

}  // namespace

const char* help_text() { return kHelp; }

std::string version_line() { return std::string("mie-decoder ") + kVersion; }

Streams::Streams() : out(stdout), err(stderr) {}

Streams::Streams(std::FILE* out_, std::FILE* err_) : out(out_), err(err_) {}

int run(const std::vector<std::string>& args) { return run(args, Streams()); }

int run(const std::vector<std::string>& args, const Streams& streams) {
    // Help and version are answered before any other validation, matching the
    // other two implementations: asking a broken invocation for help must
    // produce help, not a usage error about the invocation.
    for (std::size_t i = 0; i < args.size(); ++i) {
        if (is_version_flag(args[i])) {
            return write_out(streams.out, version_line() + "\n") ? EXIT_OK : EXIT_RUNTIME;
        }
        if (is_help_flag(args[i])) {
            return write_out(streams.out, kHelp) ? EXIT_OK : EXIT_RUNTIME;
        }
    }

    if (args.empty()) {
        // Help, but a non-zero exit: a script that invokes the tool with no
        // arguments has a bug, and exiting 0 would hide it.
        (void)write_out(streams.err, kHelp);
        return EXIT_USAGE;
    }

    try {
        ArgReader reader(args);
        GlobalArgs globals;
        std::string value;

        // Global flags may appear before the subcommand.
        while (!reader.at_end()) {
            if (reader.take_value("--config", value)) {
                globals.config = value;
            } else if (reader.take_value("--log-level", value)) {
                globals.log_level = value;
            } else {
                break;
            }
        }

        if (reader.at_end()) {
            throw usage_error("no command given; expected decode or count");
        }
        const std::string command = reader.peek();
        reader.advance();

        // The level is applied early so the version banner and every later
        // diagnostic respect it. An invalid value fails here rather than being
        // silently ignored.
        if (globals.log_level.has_value()) {
            apply_log_level("--log-level", globals.log_level.value());
        } else {
            log::set_level(log::LEVEL_WARN);
        }
        MIE_LOG_INFO(version_line());

        if (command == "decode") {
            DecodeArgs decode_args = parse_decode(reader);
            return run_decode(streams, globals, decode_args);
        }
        if (command == "count") {
            return run_count(streams, globals, parse_count(reader));
        }
        if (command == "dump") {
            throw usage_error(
                "dump is not available in this build: the hex-dump module is not ported yet. "
                "Use the Rust or Python implementation for it.");
        }
        throw usage_error("unknown command \"" + command + "\"; expected decode or count");
    } catch (const CliError& error) {
        MIE_LOG_ERROR(error.message);
        (void)write_out(streams.err, "Error: " + error.message + "\n");
        if (error.code == EXIT_USAGE) {
            (void)write_out(streams.err, std::string("\n") + kHelp);
        }
        return error.code;
    } catch (const MieError& error) {
        // Anything a runner did not already classify.
        MIE_LOG_ERROR(error.message());
        (void)write_out(streams.err, "Error: " + error.message() + "\n");
        return exit_code_for(error);
    } catch (const ConfigError& error) {
        MIE_LOG_ERROR(error.message());
        (void)write_out(streams.err, "Error: " + error.message() + "\n");
        return EXIT_CONFIG;
    } catch (const std::exception& error) {
        // The backstop. An exception escaping here would reach
        // std::terminate and abort, and a caller cannot tell an abort
        // apart from a signal -- every failure this tool has must arrive
        // as an exit code.
        (void)write_out(streams.err, std::string("Error: ") + error.what() + "\n");
        return EXIT_RUNTIME;
    }
}

}  // namespace cli
}  // namespace mie
