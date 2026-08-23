// SPDX-License-Identifier: Apache-2.0

#include "mie/source.hpp"

namespace mie {

// Out of line so this translation unit is the vtable's key function, rather
// than every consumer emitting a copy of it.
MessageSource::MessageSource() = default;
MessageSource::~MessageSource() = default;

}  // namespace mie
