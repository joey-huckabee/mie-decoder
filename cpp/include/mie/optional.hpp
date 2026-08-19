// SPDX-License-Identifier: Apache-2.0
//
// A minimal stand-in for std::optional, which is C++17 and therefore
// unavailable at this tree's floor (ADR-0001).
//
// The Rust implementation's model types are full of Option<T> --
// `Option<CommandWord>`, `Option<u16>`, `Option<f64>` -- and the Python
// implementation's are full of `| None`. Reproducing that shape matters for
// more than style: "absent" and "zero" are genuinely different in this format.
// A missing Status Word is not a Status Word of 0x0000, and a DELTA that cannot
// be computed (SPURIOUS_DATA has no RT/MSG key; an uncalibrated Standard
// timestamp has no tick rate) is not a DELTA of 0.0. Collapsing the two would
// put a wrong value in the CSV rather than an empty cell.
//
// DELIBERATELY NOT a general-purpose optional. It requires T to be
// default-constructible and copyable, and it stores a T unconditionally rather
// than using aligned storage with placement new. That costs sizeof(T) bytes for
// an empty Optional and rules out non-default-constructible types -- both
// irrelevant here, since every T it holds is a small trivially-copyable value.
// What it buys is roughly forty lines instead of three hundred, with no
// lifetime subtleties to get wrong on a compiler from 2015.
//
// If a future need arises for Optional<T> over a type that is expensive to
// default-construct or has no default constructor, that is the point to replace
// this with a real implementation -- not before.

#ifndef MIE_OPTIONAL_HPP
#define MIE_OPTIONAL_HPP

namespace mie {

/// Tag type for constructing an empty Optional, so `Optional<T> x = none();`
/// reads as intent rather than as a default-construction accident.
struct NoneType {};

inline NoneType none() { return NoneType(); }

template <typename T>
class Optional {
  public:
    Optional() : value_(), has_value_(false) {}

    /// Both single-argument constructors are implicit ON PURPOSE, and both
    /// analysers object to it -- clang-tidy as google-explicit-constructor,
    /// cppcheck as noExplicitConstructor. Both objections are answered here,
    /// at the two lines that earn the exception, rather than by disabling
    /// either check tree-wide: an unintended implicit conversion anywhere else
    /// should still be reported.
    ///
    /// `return none();` and `m.delta = 0.0;` are the shapes this type exists to
    /// support. Requiring an explicit wrap at every assignment and every return
    /// would add noise at hundreds of call sites to prevent a conversion that is
    /// the intended reading at all of them.
    // cppcheck-suppress noExplicitConstructor
    Optional(NoneType) : value_(), has_value_(false) {}  // NOLINT(google-explicit-constructor)

    // cppcheck-suppress noExplicitConstructor
    Optional(const T& value)
        : value_(value), has_value_(true) {}  // NOLINT(google-explicit-constructor)

    bool has_value() const { return has_value_; }

    /// Unchecked. Callers must test has_value() first; there is no exception to
    /// throw here that would be more useful than the discipline of checking,
    /// and this type is used on the per-record hot path.
    const T& value() const { return value_; }
    T& value() { return value_; }

    /// The safe accessor, for the many sites that want a substitute rather than
    /// a branch -- CSV columns that render empty, counters that start at zero.
    T value_or(const T& fallback) const { return has_value_ ? value_ : fallback; }

    void reset() {
        value_ = T();
        has_value_ = false;
    }

    void set(const T& value) {
        value_ = value;
        has_value_ = true;
    }

    /// Equality treats two empties as equal and never compares the held value
    /// of an empty one -- which is why value_ is always a valid T rather than
    /// raw storage.
    bool operator==(const Optional& other) const {
        if (has_value_ != other.has_value_) {
            return false;
        }
        return !has_value_ || value_ == other.value_;
    }

    bool operator!=(const Optional& other) const { return !(*this == other); }

  private:
    T value_;
    bool has_value_;
};

}  // namespace mie

#endif  // MIE_OPTIONAL_HPP
