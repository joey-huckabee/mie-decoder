// SPDX-License-Identifier: Apache-2.0
//
// Catch2's main(). Alone in its own translation unit on purpose: defining
// CATCH_CONFIG_MAIN expands the whole test runner, which is slow to compile, and
// putting it beside real tests means every edit to those tests pays that cost.

#define CATCH_CONFIG_MAIN
#include <catch2/catch.hpp>
