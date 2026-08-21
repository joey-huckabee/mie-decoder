// SPDX-License-Identifier: Apache-2.0
//
// The pipeline's shared contract: a pull source of decoded records.
//
// Every stage between the reader and the writer both CONSUMES one of these and
// IS one, which is what lets the pipeline be assembled by nesting rather than
// by a fixed sequence:
//
//     RecordIter  ->  FilteredSource  ->  OrderedSource  ->  write_csv
//
// That is the same shape as the Rust implementation's iterator-adapter chain
// (`reader.iter().filter_messages(f).order_rows(n)`), expressed with the tool
// C++ has for it. A stage that wraps its input and presents the same interface
// is a Decorator; the value here is not the pattern name but the consequence --
// stages can be added, reordered or omitted without any of them knowing what
// the others are, and the writer needs no knowledge of whether filtering
// happened.
//
// This interface lived in `writer.hpp` when the writer was its only consumer.
// It moved when the second one arrived: `filter` depending on `writer` to
// borrow a type it needs would have been a dependency edge that describes
// nothing real.

#ifndef MIE_SOURCE_HPP
#define MIE_SOURCE_HPP

#include "mie/models.hpp"

namespace mie {

/// A pull source of decoded records.
class MessageSource {
  public:
    virtual ~MessageSource();

    /// Fill `out` with the next record; false at end of stream.
    ///
    /// May throw MieError. `--allow-partial` turns an UnrecoverableSyncLoss
    /// thrown from here into a committed `.partial` rather than a failure, so
    /// this is a load-bearing part of the contract, not just an error path.
    ///
    /// A stage that BUFFERS -- `OrderedSource` does -- must therefore flush
    /// what it holds before letting a throw through, or the rows it was
    /// holding never reach the `.partial` the operator is given.
    virtual bool next(MieMessage& out) = 0;

  protected:
    MessageSource();

  private:
    MessageSource(const MessageSource&);
    MessageSource& operator=(const MessageSource&);
};

}  // namespace mie

#endif  // MIE_SOURCE_HPP
