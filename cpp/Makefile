# MIE-Decoder -- C++ build (ADR-0002)
# Target toolchain: GCC 4.8.5 on SLES 12 SP5. Keep everything C++11-clean.
#
# This Makefile is AUTHORITATIVE on Linux. CMakeLists.txt exists alongside it
# for the MSVC/Visual Studio build and is authoritative there. The reason for
# two build descriptions rather than one is not preference: the gcc:4.8 fidelity
# container is Debian 7 and ships CMake 2.8, which cannot read a modern
# CMakeLists.txt at all -- so the tier that proves SLES 12 conformance has to be
# driven by make. Both read cpp/sources.txt so the file list cannot drift.

CXX      ?= g++
CPPFLAGS += -Iinclude -isystem third_party -D_FILE_OFFSET_BITS=64

# --- Memory-safety instrumentation (modern toolchains; CI jobs) -------------
# Usage: make check SANITIZE=1     (AddressSanitizer + UBSan + LeakSanitizer)
#        make check-valgrind       (Valgrind memcheck, error on any finding)
#
# GCC 4.8 has only partial ASan and no LSan, so the sanitizer tiers run on the
# modern compiler. The code under test is identical because the source is
# strictly C++11-conformant and compiled from one body -- which is the argument
# ADR-0001 rests on, and the reason the 4.8 tier runs the FULL suite rather than
# a compile check.
#
# This block must precede BUILD_DIR, which keys on TIER: make expands target
# patterns as it parses, so a later assignment would not reach the rules below.
SANITIZE ?= 0
ifeq ($(SANITIZE),1)
CXXFLAGS ?= -std=c++11 -Wall -Wextra -Werror -O1 -g \
            -fsanitize=address,undefined -fno-omit-frame-pointer \
            -fno-sanitize-recover=all
TIER := asan
else
CXXFLAGS ?= -std=c++11 -Wall -Wextra -Werror -O2 -g
TIER := default
endif

# Objects are keyed on the toolchain that produced them so the tiers cannot
# clobber one another. Running the WSL build and then the gcc:4.8 container
# build over a shared directory leaves a glibc-2.34 binary in place for the
# container to execute, which fails with a wall of "GLIBC_2.xx not found" and
# reads as a code fault rather than the stale artifact it is.
#
# Override per invocation when needed: make BUILD_DIR=/somewhere/else
TOOLCHAIN ?= $(shell $(CXX) -dumpmachine 2>/dev/null)-$(shell $(CXX) -dumpversion 2>/dev/null)-$(TIER)
BUILD_DIR ?= build/$(TOOLCHAIN)

# --- Sources ----------------------------------------------------------------
# The portable library sources come from sources.txt, shared with CMake. Comment
# and blank lines are stripped here rather than in the file so the file stays
# readable. $(file <) would be tidier but needs GNU make 4.2; SLES 12 ships 4.0.
#
# The comment character is written as \x23 rather than literally. make strips
# comments before expanding a line, INCLUDING inside $(shell ...), so a literal
# one here eats the closing paren and the file dies with "unterminated call to
# function shell". GNU make 4.3 happens to tolerate it; make 3.81 -- which is
# what the gcc:4.8 fidelity container ships -- does not, so this failed only in
# the tier that matters. GNU sed has understood \xHH since 4.0.
LIB_SRC  := $(shell sed -e 's/\x23.*//' -e '/^[[:space:]]*$$/d' sources.txt)

# Exactly one platform backend, chosen by the host. Deliberately not in
# sources.txt: everything named there is compiled unconditionally, and these two
# are mutually exclusive.
LIB_SRC  += src/platform_posix.cpp

LIB_OBJ  := $(patsubst src/%.cpp,$(BUILD_DIR)/src/%.o,$(LIB_SRC))

# Tests are globbed rather than listed. This is the opposite of the sources.txt
# decision on purpose: a source file omitted from the build fails loudly at
# link time, whereas a TEST file omitted from the build simply does not run --
# it fails silently, and a silently-skipped test is worse than a wildcard.
TEST_SRC := $(sort $(wildcard tests/*.cpp))
TEST_OBJ := $(patsubst tests/%.cpp,$(BUILD_DIR)/tests/%.o,$(TEST_SRC))

TEST_BIN    := $(BUILD_DIR)/mie_decoder_tests
DECODER_BIN := $(BUILD_DIR)/mie-decoder
DECODER_OBJ := $(BUILD_DIR)/src/main.o

# Header dependency tracking: without it, editing a header rebuilds nothing and
# the next test run silently exercises stale objects.
DEPFLAGS := -MMD -MP

.PHONY: all check clean clean-all toolchain versions check-valgrind \
        check-gcc48 verify-ci print-test-bin print-decoder-bin \
        print-lib-sources decoder tidy format format-check

all: $(TEST_BIN) $(DECODER_BIN)

decoder: $(DECODER_BIN)

$(BUILD_DIR)/src/%.o: src/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CPPFLAGS) $(CXXFLAGS) $(DEPFLAGS) -c $< -o $@

$(BUILD_DIR)/tests/%.o: tests/%.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CPPFLAGS) $(CXXFLAGS) $(DEPFLAGS) -c $< -o $@

$(TEST_BIN): $(LIB_OBJ) $(TEST_OBJ)
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $^ -o $@

$(DECODER_BIN): $(LIB_OBJ) $(DECODER_OBJ)
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) $^ -o $@

-include $(LIB_OBJ:.o=.d) $(TEST_OBJ:.o=.d) $(DECODER_OBJ:.o=.d)

# --use-colour no keeps CI logs readable; Catch2 otherwise emits ANSI escapes
# that a log viewer renders as noise around every assertion.
check: $(TEST_BIN)
	$(TEST_BIN) --use-colour no

clean:
	rm -rf $(BUILD_DIR)

clean-all:
	rm -rf build

toolchain:
	@echo $(BUILD_DIR)

# Printed at the head of every CI job so a toolchain change shows up as a
# visible diff in the log rather than as an unexplained warning somewhere else.
versions:
	@echo "CXX       : $(CXX)"
	@$(CXX) --version | head -1
	@echo "TOOLCHAIN : $(TOOLCHAIN)"
	@echo "BUILD_DIR : $(BUILD_DIR)"
	@echo "SOURCES   : $(words $(LIB_SRC)) library, $(words $(TEST_SRC)) test"

print-test-bin:
	@echo $(TEST_BIN)

# The Makefile's own view of the library source set, for
# scripts/assert-sources-agree.sh. That gate asks the build rather than
# re-implementing the sources.txt parsing, so it compares against the build
# itself instead of against a copy that could drift alongside it.
print-lib-sources:
	@echo $(LIB_SRC)

# Used by tests/conformance/run.py so the harness never re-derives BUILD_DIR --
# one place computes the toolchain-keyed path, and every caller asks it.
print-decoder-bin:
	@echo $(DECODER_BIN)

check-valgrind: $(TEST_BIN)
	valgrind --tool=memcheck --leak-check=full --show-leak-kinds=all \
	  --errors-for-leak-kinds=all --error-exitcode=1 \
	  $(TEST_BIN) --use-colour no

# --- Deployment-fidelity tier (GCC 4.8.5) -----------------------------------
# A MIRROR of docker.io/library/gcc:4.8, not that image directly. The upstream
# tag was published in 2016 with a Docker manifest v2 *schema 1*, which modern
# Docker disables by default -- the job dies at the pull with exit 125 before
# compiling anything, while the same tier passes locally because podman still
# accepts schema 1. The mirror is the identical image republished with a v2s2
# manifest. Shared with background-file-mover, which published it.
GCC48_IMAGE ?= ghcr.io/joey-huckabee/gcc-4.8:4.8.5
CI_ENGINE   ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)
CI_JOBS     ?= $(shell nproc 2>/dev/null || echo 4)

# Runs the fidelity tier the way CI runs it: one target, one image, both
# callers. Three separately maintained copies of this invocation -- in a
# workflow, in CONTRIBUTING.md, and in a developer's shell history -- is exactly
# how CI and local came to pull different images without either noticing.
#
# NOTE FOR WSL2: this must run from a Linux-native path. Rootless podman cannot
# bind-mount a /mnt/c DrvFs path; the container starts, the mount is silently
# empty, and g++ reports "no input files" as though the source had vanished.
check-gcc48:
	$(CI_ENGINE) run --rm -v "$(CURDIR)":/cpp -w /cpp $(GCC48_IMAGE) \
	    sh -c "make clean && make -j$(CI_JOBS) check"

# --- Static analysis and formatting -----------------------------------------
TIDY   ?= clang-tidy
FORMAT ?= clang-format

# Only project sources: vendored headers arrive via -isystem precisely so they
# do not trip the gates, and pointing an analyzer at them would undo that.
#
# clang-tidy sees only what this host compiles. It cannot analyse
# platform_win32.cpp on Linux -- there is no <windows.h> to parse -- so that
# file is covered by the MSVC tier's /W4 /WX instead.
TIDY_SRC := $(LIB_SRC) src/main.cpp $(TEST_SRC)

# clang-format has no such excuse: it never compiles anything, so it can and
# must cover BOTH backends. Leaving the inactive one out means the Windows
# backend is never format-checked on the Linux tiers and never format-checked on
# the Windows tier either (which runs no clang-format), so it drifts silently
# until someone opens it.
FORMAT_SRC := $(sort $(TIDY_SRC) src/platform_posix.cpp src/platform_win32.cpp) \
              $(wildcard include/mie/*.hpp)

# --warnings-as-errors='*' lives HERE, not in .clang-tidy's WarningsAsErrors.
#
# clang-tidy exits 0 when it emits warnings unless told otherwise, so a gate
# without this flag prints findings and reports success -- which is worse than
# no gate, because the log looks like it was checked. The enabled/disabled check
# list stays in .clang-tidy where the reasoning lives; the severity is a property
# of running it as a gate.
tidy:
	$(TIDY) -p $(BUILD_DIR) --warnings-as-errors='*' $(TIDY_SRC)

format-check:
	$(FORMAT) --dry-run --Werror $(FORMAT_SRC)

format:
	$(FORMAT) -i $(FORMAT_SRC)

# --- CI parity ---------------------------------------------------------------
# The pre-push check: the gates whose LOCAL tool differs from CI's, run with
# CI's tools in CI's image.
#
# Every analysis tool a developer runs here is older than the one CI runs. On
# the WSL2 development host (Ubuntu 22.04) against the CI runner (Ubuntu 24.04):
#
#     clang-format   14  vs  18
#     clang-tidy     14  vs  18
#     cppcheck      2.7  vs  2.13
#
# That matters because a passing local run can mean "my tool does not know about
# this check" rather than "this code is fine" -- and in the other direction,
# formatting with clang-format 14 and having CI check with 18 produces a red
# build with no code change behind it. Both have already happened here.
#
# Not a substitute for the sanitizer, Valgrind or GCC 4.8.5 tiers, which run
# against the real compilers. This is what to run before pushing a formatting or
# lint-affecting change.
CI_BASE_IMAGE ?= docker.io/library/ubuntu:24.04

verify-ci:
	@echo "Running the divergence-prone gates in $(CI_BASE_IMAGE) via $(CI_ENGINE)..."
	$(CI_ENGINE) run --rm -v "$(CURDIR)":/cpp -w /cpp $(CI_BASE_IMAGE) bash -c '\
	  apt-get update -qq >/dev/null && \
	  apt-get install -y -qq --no-install-recommends \
	      g++-14 make clang-format-18 clang-tidy-18 cppcheck bear >/dev/null && \
	  echo "--- versions ---" && \
	  g++-14 --version | head -1 && \
	  clang-format-18 --version && \
	  clang-tidy-18 --version | grep -i version | head -1 && \
	  cppcheck --version && \
	  echo "--- format ---" && \
	  make format-check FORMAT=clang-format-18 && \
	  echo "--- build (CI compiler) ---" && \
	  make clean CXX=g++-14 && bear -- make -j$(CI_JOBS) all CXX=g++-14 && \
	  echo "--- clang-tidy ---" && \
	  make tidy TIDY=clang-tidy-18 CXX=g++-14 && \
	  echo "--- cppcheck ---" && \
	  cppcheck --error-exitcode=1 --enable=warning,style,performance,portability \
	           --inline-suppr --std=c++11 --language=c++ \
	           --suppress=missingIncludeSystem -I include src tests'
	@echo ""
	@echo "verify-ci OK. This leaves the build tree built with g++-14;"
	@echo "your next local 'make check' will rebuild against the host compiler."
