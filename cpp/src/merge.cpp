// SPDX-License-Identifier: Apache-2.0

#define MIE_LOG_MODULE "mie_decoder::merge"

#include "mie/merge.hpp"

#include <algorithm>
#include <cstdio>

#include "mie/error.hpp"
#include "mie/log.hpp"
#include "mie/text.hpp"

namespace mie {
namespace merge {

namespace {

/// Byte length of the UTF-8 sequence starting at `lead`.
///
/// Used so `?` consumes one character rather than one byte. A malformed or
/// unexpected continuation byte reports 1, which degrades to byte-wise matching
/// for that position rather than running off the end of the string -- a
/// filename is not required to be valid UTF-8 on either platform, and a matcher
/// is the wrong place to reject one.
std::size_t utf8_sequence_length(unsigned char lead) {
    if ((lead & 0x80) == 0x00) {
        return 1;
    }
    if ((lead & 0xE0) == 0xC0) {
        return 2;
    }
    if ((lead & 0xF0) == 0xE0) {
        return 3;
    }
    if ((lead & 0xF8) == 0xF0) {
        return 4;
    }
    return 1;
}

/// Advance `at` by one whole character, never past `size`.
std::size_t next_char(const std::string& text, std::size_t at) {
    const std::size_t width = utf8_sequence_length(static_cast<unsigned char>(text[at]));
    const std::size_t stepped = at + width;
    return stepped > text.size() ? text.size() : stepped;
}

/// The microsecond value a message merges on.
///
/// Every input is validated as calendar-locked IRIG before any of this runs, so
/// the "no value" branch is unreachable for a real merge; 0 is a defined answer
/// rather than an assertion so a future format cannot turn a merge into a
/// crash.
uint64_t micros_of(const MieMessage& message, const Optional<double>& tick) {
    uint64_t out = 0;
    if (!message.timestamp.to_microseconds(tick, out)) {
        return 0;
    }
    return out;
}

/// Reject an input whose leading record cannot anchor an absolute timeline.
void check_mergeable(const MieMessage& message, std::size_t file_index, const std::string& path) {
    if (message.timestamp.is_standard()) {
        throw MieError::incompatible_merge_inputs(file_index, path,
                                                  "resolves to the Standard timestamp format");
    }
    if (message.timestamp.is_irig() && message.timestamp.irig.freerun) {
        throw MieError::incompatible_merge_inputs(
            file_index, path, "leads with a freerun IRIG record (no calendar time)");
    }
}

/// Take the error out of `slot`, clearing it.
///
/// Clearing BEFORE the throw is the point: a caller that catches and calls
/// `next()` again must get a clean end of stream, not the same failure a second
/// time. Returning by value also means the throw expression is a temporary,
/// which is what keeps the thrown object independent of the slot it came from.
MieError take(PendingError& slot) {
    const MieError error = *slot;
    slot.reset();
    return error;
}

/// Absolute distance between two microsecond stamps.
///
/// Order-independent on purpose. A lenient non-monotonic input (L2-MRG-006) can
/// step backward, and a one-sided subtraction would wrap to an enormous
/// unsigned value -- which compares as "outside the window" only by luck.
uint64_t abs_diff(uint64_t a, uint64_t b) { return a > b ? a - b : b - a; }

}  // namespace

// ---------------------------------------------------------------------------
// Input resolution
// ---------------------------------------------------------------------------

bool read_manifest(const std::string& path, std::vector<std::string>& out, platform::OsError& err) {
    err.clear();
    out.clear();
    // Through the platform layer: a manifest may sit at a non-ASCII path, and
    // std::fopen would fail to find it on Windows.
    std::vector<uint8_t> raw;
    if (!platform::read_file(path, raw, err)) {
        return false;
    }
    const std::string body(raw.begin(), raw.end());

    std::string line;
    for (std::size_t i = 0; i <= body.size(); ++i) {
        if (i == body.size() || body[i] == '\n') {
            const std::string trimmed = text::trim_ascii_blank(line);
            // A comment or a blank line is not an empty path -- treating it as
            // one would put "" into the merge and fail on a file nobody named.
            if (!trimmed.empty() && trimmed[0] != '#') {
                out.push_back(trimmed);
            }
            line.clear();
        } else if (body[i] != '\r') {
            line += body[i];
        }
    }
    return true;
}

bool glob_match(const std::string& pattern, const std::string& name) {
    std::size_t p = 0;
    std::size_t t = 0;
    bool have_star = false;
    std::size_t star_at = 0;
    std::size_t mark = 0;

    while (t < name.size()) {
        if (p < pattern.size() && pattern[p] == '?') {
            p += 1;
            t = next_char(name, t);
        } else if (p < pattern.size() && pattern[p] != '*' && pattern[p] == name[t]) {
            p += 1;
            t += 1;
        } else if (p < pattern.size() && pattern[p] == '*') {
            have_star = true;
            star_at = p;
            mark = t;
            p += 1;
        } else if (have_star) {
            // Backtrack: let the last `*` swallow one more character and retry.
            // Stepping `mark` by a whole character keeps a multi-byte name from
            // being split mid-sequence, which would then compare against
            // pattern bytes that can never match.
            p = star_at + 1;
            mark = next_char(name, mark);
            t = mark;
        } else {
            return false;
        }
    }
    while (p < pattern.size() && pattern[p] == '*') {
        p += 1;
    }
    return p == pattern.size();
}

bool expand_glob(const std::string& pattern, std::vector<std::string>& out,
                 platform::OsError& err) {
    err.clear();
    out.clear();

    // Split on the LAST separator. Both are accepted on both platforms: a
    // config file or a script written on one host is routinely run on the
    // other, and a backslash is not a legal filename character on Windows
    // anyway.
    const std::size_t slash = pattern.find_last_of("/\\");
    std::string directory;
    std::string name_pattern;
    // Everything up to AND INCLUDING the separator, reused verbatim when
    // rebuilding each match. Rebuilding with a hardcoded '/' instead produced
    // `C:\dir/file.mie` for a Windows pattern -- which Windows happily OPENS,
    // so the decode still worked and only the paths the tool reported back were
    // mangled. A defect that survives every functional check and shows up in
    // log lines and error messages is worth more care, not less.
    std::string prefix;
    if (slash == std::string::npos) {
        directory = ".";
        name_pattern = pattern;
    } else {
        directory = pattern.substr(0, slash);
        name_pattern = pattern.substr(slash + 1);
        prefix = pattern.substr(0, slash + 1);
        if (directory.empty()) {
            // Rooted at the separator itself, e.g. "/recordings.mie".
            directory = pattern.substr(0, 1);
        }
    }

    std::vector<std::string> names;
    if (!platform::list_directory(directory, names, err)) {
        return false;
    }

    for (std::size_t i = 0; i < names.size(); ++i) {
        if (glob_match(name_pattern, names[i])) {
            out.push_back(prefix + names[i]);
        }
    }
    // Lexicographic, so two hosts enumerating the same directory merge in the
    // same order. Directory order is whatever the filesystem feels like, which
    // would make the output depend on the machine that produced it.
    std::sort(out.begin(), out.end());
    return true;
}

// ---------------------------------------------------------------------------
// Options and dedup
// ---------------------------------------------------------------------------

MergeOptions::MergeOptions()
    : allow_partial(false),
      strict(false),
      collapse_duplicates(false),
      collapse_window_us(0),
      delta_scope(DELTA_SCOPE_PER_FILE) {}

DedupKey::DedupKey() {}

DedupKey DedupKey::of(const MieMessage& message) {
    DedupKey key;
    key.type_word = message.type_word;
    key.command_word = message.command_word;
    key.command_word_2 = message.command_word_2;
    key.status_word = message.status_word;
    key.status_word_2 = message.status_word_2;
    key.error_word = message.error_word;
    key.data_words = message.data_words;
    return key;
}

bool DedupKey::operator==(const DedupKey& other) const {
    return type_word.raw == other.type_word.raw && command_word == other.command_word &&
           command_word_2 == other.command_word_2 && status_word == other.status_word &&
           status_word_2 == other.status_word_2 && error_word == other.error_word &&
           data_words == other.data_words;
}

DedupWindow::Survivor::Survivor(uint64_t us_, std::size_t file_index_, const DedupKey& key_)
    : us(us_), file_index(file_index_), key(key_) {}

DedupWindow::DedupWindow(uint64_t window_us) : window_us_(window_us) {}

bool DedupWindow::is_duplicate(uint64_t us, std::size_t file_index, const MieMessage& message) {
    // Evict survivors that can no longer fall within the window of this or any
    // later record. Guarded against a backward step so a non-monotonic input
    // cannot underflow into an enormous difference and evict everything.
    while (!survivors_.empty()) {
        const uint64_t buffered = survivors_.front().us;
        if (us > buffered && (us - buffered) > window_us_) {
            survivors_.pop_front();
        } else {
            break;
        }
    }

    const DedupKey key = DedupKey::of(message);
    const uint64_t window = window_us_;
    // A survivor matches only if it came from a DIFFERENT input and lies within
    // the window in ABSOLUTE time -- the merged stream may step backward, so
    // the distance has to be order-independent rather than a one-sided
    // subtraction.
    const bool matched =
        std::any_of(survivors_.begin(), survivors_.end(), [&](const Survivor& survivor) {
            return survivor.file_index != file_index && abs_diff(survivor.us, us) <= window &&
                   survivor.key == key;
        });
    if (matched) {
        return true;
    }
    survivors_.emplace_back(us, file_index, key);
    return false;
}

// ---------------------------------------------------------------------------
// MergedSource
// ---------------------------------------------------------------------------

MergedSource::Entry::Entry() : us(0), file_index(0), seq(0) {}

bool MergedSource::Entry::operator<(const Entry& other) const {
    // Reversed: std::priority_queue is a max-heap and the merge wants the
    // smallest key first.
    if (us != other.us) {
        return us > other.us;
    }
    if (file_index != other.file_index) {
        return file_index > other.file_index;
    }
    return seq > other.seq;
}

MergedSource::MergedSource(const std::vector<MieFileReader*>& readers, const MergeOptions& options)
    : readers_(readers),
      next_seq_(readers.size(), 0),
      prev_us_(readers.size(), 0),
      has_prev_(readers.size(), false),
      warned_backward_(readers.size(), false),
      options_(options),
      delta_tracker_(options.standard_tick_rate_hz),
      dedup_(options.collapse_window_us),
      collapsed_(0) {
    iters_.reserve(readers_.size());
    paths_.reserve(readers_.size());
    for (std::size_t i = 0; i < readers_.size(); ++i) {
        paths_.push_back(readers_[i]->path());
        iters_.push_back(readers_[i]->iter());
    }

    for (std::size_t i = 0; i < iters_.size(); ++i) {
        MieMessage first;
        bool got = false;
        try {
            got = iters_[i].next(first);
        } catch (const MieError& error) {
            if (!options_.allow_partial) {
                throw;
            }
            // L2-MRG-004: drop the input and keep going. The file contributed
            // nothing (it failed at offset 0), and arming the terminal is what
            // makes the writer commit a `.partial` rather than a clean CSV that
            // silently omits an entire recording.
            MIE_LOG_WARN("merge: input #" + text::decimal(i) + " (" + paths_[i] +
                         ") could not be read; truncating it from the merge "
                         "(--allow-partial): " +
                         error.message());
            pending_terminal_.reset(new MieError(MieError::unrecoverable_sync_loss(0, 0)));
            continue;
        }
        if (!got) {
            continue;  // valid but empty recording; contributes nothing
        }
        check_mergeable(first, i, paths_[i]);

        Entry entry;
        entry.us = micros_of(first, options_.standard_tick_rate_hz);
        entry.file_index = i;
        entry.seq = 0;
        entry.message = first;
        prev_us_[i] = entry.us;
        has_prev_[i] = true;
        next_seq_[i] = 1;
        heap_.push(entry);
    }
}

uint64_t MergedSource::merge_micros(const MieMessage& message) const {
    return micros_of(message, options_.standard_tick_rate_hz);
}

void MergedSource::apply_global_delta(MieMessage& message) {
    if (options_.delta_scope == DELTA_SCOPE_PER_FILE) {
        // The default, and a deliberate no-op: it keeps the value the record's
        // own reader computed, which is what makes a merged DELTA identical to
        // a single-file one.
        return;
    }
    const CommandWord* command =
        message.command_word.has_value() ? &message.command_word.value() : nullptr;
    // No WARN here. A backward step on the merged timeline means some input was
    // not internally sorted, which `advance` already reports once per file
    // (L2-MRG-006); repeating it per record would bury that one message.
    message.delta = delta_tracker_.observe(command, message.timestamp).value();
}

void MergedSource::advance(std::size_t file_index) {
    MieMessage message;
    bool got = false;
    try {
        got = iters_[file_index].next(message);
    } catch (const MieError& error) {
        if (options_.allow_partial && error.kind() == KIND_UNRECOVERABLE_SYNC_LOSS) {
            MIE_LOG_WARN("merge: input #" + text::decimal(file_index) +
                         " truncated at its failure point: " + error.message());
            // Deferred until the heap drains, so every good record from every
            // input is written before the failure surfaces.
            pending_terminal_.reset(new MieError(error));
        } else {
            // Surfaced on the NEXT call, after the record already popped.
            pending_error_.reset(new MieError(error));
        }
        return;
    }
    if (!got) {
        return;  // this input is exhausted
    }

    const uint64_t us = merge_micros(message);
    // L2-MRG-006: each input is assumed internally time-sorted, because capture
    // order is chronological. A backward step means the merged output may be
    // out of order for this input.
    if (has_prev_[file_index] && us < prev_us_[file_index]) {
        if (options_.strict) {
            pending_error_.reset(new MieError(MieError::non_monotonic_input(
                file_index, paths_[file_index], prev_us_[file_index], us)));
        } else if (!warned_backward_[file_index]) {
            warned_backward_[file_index] = true;
            MIE_LOG_WARN("merge: input #" + text::decimal(file_index) + " (" + paths_[file_index] +
                         ") is not internally time-sorted: timestamp stepped backward (prev_us=" +
                         text::decimal(prev_us_[file_index]) + " curr_us=" + text::decimal(us) +
                         ") — merged output may be out of order for this input (further "
                         "occurrences suppressed)");
        }
    }
    prev_us_[file_index] = us;
    has_prev_[file_index] = true;

    Entry entry;
    entry.us = us;
    entry.file_index = file_index;
    entry.seq = next_seq_[file_index];
    entry.message = message;
    next_seq_[file_index] = entry.seq + 1;
    heap_.push(entry);
}

bool MergedSource::next(MieMessage& out) {
    for (;;) {
        if (pending_error_) {
            throw take(pending_error_);
        }
        if (heap_.empty()) {
            if (pending_terminal_) {
                throw take(pending_terminal_);
            }
            return false;
        }

        const Entry entry = heap_.top();
        heap_.pop();
        const std::size_t file_index = entry.file_index;

        // Collapse BEFORE the global-DELTA stage (L2-MRG-007). A suppressed
        // duplicate must not advance the per-key DELTA cursor, or DELTA would
        // be measured across a timeline that includes rows nobody can see.
        if (options_.collapse_duplicates &&
            dedup_.is_duplicate(entry.us, file_index, entry.message)) {
            collapsed_ += 1;
            advance(file_index);
            continue;
        }

        out = entry.message;
        apply_global_delta(out);
        advance(file_index);
        return true;
    }
}

}  // namespace merge
}  // namespace mie
