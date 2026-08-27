// SPDX-License-Identifier: Apache-2.0
//
// The TOML-subset parser.
//
// This suite is written to be liftable with the component it tests: it uses no
// MIE type and asserts nothing about decoder semantics. What it pins is the
// GRAMMAR — what the parser accepts, what it refuses, and why refusing is the
// correct behaviour rather than a limitation.
//
// The number grammar gets an exhaustive sweep rather than samples, because it is
// the single most divergence-prone rule in the subset: every language's native
// number parser is MORE permissive than TOML. `strtod` takes `08`, `1.`, `0x10`
// and leading whitespace; Python's `tomllib` takes none of them. A parser that
// delegates the decision to its language drifts from a strict one on inputs
// nobody thought to write a case for, which is exactly what an exhaustive sweep
// finds and a sample does not.

#include "mie/toml.hpp"

#include <catch2/catch.hpp>

#include <cstddef>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

// Not `tm`: that is `struct tm` from <ctime>, which arrives transitively and
// makes every use ambiguous. g++ resolved it to the namespace; clang refused,
// and refusing is correct.
namespace tml = mie::toml;

/// Parse, requiring success, and return the document.
tml::Document must_parse(const std::string& text) {
    tml::Document doc;
    tml::ParseError error;
    INFO("input: " << text);
    INFO("error: " << error.format());
    REQUIRE(tml::parse(text, doc, error));
    return doc;
}

/// Parse, requiring failure, and return the error.
tml::ParseError must_reject(const std::string& text) {
    tml::Document doc;
    tml::ParseError error;
    INFO("input: " << text);
    REQUIRE_FALSE(tml::parse(text, doc, error));
    return error;
}

tml::Value value_of(const tml::Document& doc, const std::string& section, const std::string& key) {
    tml::Value out;
    INFO("looking for [" << section << "] " << key);
    REQUIRE(doc.get(section, key, out));
    return out;
}

}  // namespace

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

TEST_CASE("an empty document parses to nothing", "[toml][L2-CFG-001]") {
    CHECK(must_parse("").size() == 0);
    CHECK(must_parse("\n\n\n").size() == 0);
    CHECK(must_parse("# just a comment\n").size() == 0);
    CHECK(must_parse("   \t  \n").size() == 0);
}

TEST_CASE("sections and keys bind together", "[toml][L2-CFG-001]") {
    const tml::Document doc = must_parse("[decode]\nstrict = true\n[output]\nno_clobber = false\n");
    REQUIRE(doc.size() == 2);
    CHECK(value_of(doc, "decode", "strict").as_boolean());
    CHECK_FALSE(value_of(doc, "output", "no_clobber").as_boolean());
    // Same key name in different sections is two bindings, not a duplicate.
    CHECK_FALSE(doc.contains("decode", "no_clobber"));
}

TEST_CASE("a key before any section header binds to the empty section", "[toml]") {
    const tml::Document doc = must_parse("orphan = 1\n");
    REQUIRE(doc.size() == 1);
    CHECK(doc.at(0).section.empty());
    CHECK(doc.at(0).key == "orphan");
}

TEST_CASE("entries keep their source order and line numbers", "[toml]") {
    // Ascending line numbers are what let a schema layer point at the offending
    // line for a problem the parser cannot see -- an out-of-range value, an
    // unknown key -- which is most of what a schema rejects.
    const tml::Document doc = must_parse("[a]\nx = 1\n\n# comment\ny = 2\n");
    REQUIRE(doc.size() == 2);
    CHECK(doc.at(0).key == "x");
    CHECK(doc.at(0).line == 2);
    CHECK(doc.at(1).key == "y");
    CHECK(doc.at(1).line == 5);
}

TEST_CASE("a missing lookup leaves the out-parameter alone", "[toml]") {
    const tml::Document doc = must_parse("[a]\nx = 1\n");
    tml::Value out = tml::Value::of_string("untouched");
    CHECK_FALSE(doc.get("a", "absent", out));
    CHECK(out.as_string() == "untouched");
}

TEST_CASE("CRLF line endings parse the same as LF", "[toml]") {
    // Config files get edited on Windows and read on Linux. A trailing CR is
    // whitespace, not content -- without this, every value on every line would
    // carry an invisible character.
    const tml::Document doc = must_parse("[decode]\r\nstrict = true\r\n");
    CHECK(value_of(doc, "decode", "strict").as_boolean());
}

TEST_CASE("a final line without a trailing newline still parses", "[toml]") {
    const tml::Document doc = must_parse("[decode]\nstrict = true");
    CHECK(value_of(doc, "decode", "strict").as_boolean());
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

TEST_CASE("every value kind round-trips", "[toml][L2-CFG-001]") {
    const tml::Document doc = must_parse(
        "[v]\n"
        "s = \"text\"\n"
        "i = 42\n"
        "f = 1.5\n"
        "b = true\n"
        "a = [1, 2]\n");

    CHECK(value_of(doc, "v", "s").kind() == tml::VALUE_STRING);
    CHECK(value_of(doc, "v", "s").as_string() == "text");
    CHECK(value_of(doc, "v", "i").kind() == tml::VALUE_INTEGER);
    CHECK(value_of(doc, "v", "i").as_integer() == 42);
    CHECK(value_of(doc, "v", "f").kind() == tml::VALUE_FLOAT);
    CHECK(value_of(doc, "v", "f").as_float() == Approx(1.5));
    CHECK(value_of(doc, "v", "b").kind() == tml::VALUE_BOOLEAN);
    CHECK(value_of(doc, "v", "a").kind() == tml::VALUE_ARRAY);
    REQUIRE(value_of(doc, "v", "a").as_array().size() == 2);
}

TEST_CASE("kind_name reports what was found", "[toml]") {
    // So a schema type error can say what it got, not only what it wanted.
    CHECK(std::string(tml::Value::of_string("").kind_name()) == "string");
    CHECK(std::string(tml::Value::of_integer(0).kind_name()) == "integer");
    CHECK(std::string(tml::Value::of_float(0.0).kind_name()) == "float");
    CHECK(std::string(tml::Value::of_boolean(false).kind_name()) == "boolean");
    CHECK(std::string(tml::Value::of_array(std::vector<tml::Value>()).kind_name()) == "array");
}

TEST_CASE("a negative integer parses", "[toml]") {
    // The MUX field is 0-based with negative counting from the end, so this is
    // a real schema value rather than a grammar curiosity.
    CHECK(value_of(must_parse("[mux]\nfield = -1\n"), "mux", "field").as_integer() == -1);
}

TEST_CASE("booleans are exactly true and false", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = true\n"), "a", "x").as_boolean());
    CHECK_FALSE(value_of(must_parse("[a]\nx = false\n"), "a", "x").as_boolean());
    // Not TOML booleans, and not silently strings either.
    must_reject("[a]\nx = True\n");
    must_reject("[a]\nx = TRUE\n");
    must_reject("[a]\nx = yes\n");
    must_reject("[a]\nx = tru\n");
}

TEST_CASE("an empty value is rejected", "[toml]") {
    CHECK(must_reject("[a]\nx =\n").message == "empty value");
    must_reject("[a]\nx = \n");
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

TEST_CASE("the four supported escapes decode", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = \"q\\\"q\"\n"), "a", "x").as_string() == "q\"q");
    CHECK(value_of(must_parse("[a]\nx = \"b\\\\b\"\n"), "a", "x").as_string() == "b\\b");
    CHECK(value_of(must_parse("[a]\nx = \"n\\nn\"\n"), "a", "x").as_string() == "n\nn");
    CHECK(value_of(must_parse("[a]\nx = \"t\\tt\"\n"), "a", "x").as_string() == "t\tt");
}

TEST_CASE("an unsupported escape is refused, not passed through", "[toml]") {
    // Refusing means adding \\uXXXX later is a WIDENING rather than a change in
    // what an existing config means. Passing it through would make the escape
    // silently become a literal backslash-u today and a code point tomorrow.
    CHECK(must_reject("[a]\nx = \"\\u0041\"\n").message == "bad escape \\u");
    must_reject("[a]\nx = \"\\r\"\n");
    must_reject("[a]\nx = \"\\0\"\n");
}

TEST_CASE("a trailing backslash inside a string is refused", "[toml]") {
    CHECK(must_reject("[a]\nx = \"oops\\\"\n").message == "trailing backslash");
}

TEST_CASE("an empty string is a valid value", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = \"\"\n"), "a", "x").as_string().empty());
}

TEST_CASE("an unterminated string is refused", "[toml]") { must_reject("[a]\nx = \"open\n"); }

TEST_CASE("a single-quoted string is not part of the subset", "[toml]") {
    // TOML's literal strings. The schema has no value that needs one, and
    // accepting them would mean a second escape story.
    must_reject("[a]\nx = 'literal'\n");
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

TEST_CASE("a comment runs to end of line", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = 1  # trailing\n"), "a", "x").as_integer() == 1);
    CHECK(must_parse("[a]\n# whole line\nx = 1\n").size() == 1);
}

TEST_CASE("a hash inside a string is not a comment", "[toml]") {
    // The case a naive `line.split('#')` gets wrong, and it is reachable: a MUX
    // delimiter could legitimately be "#".
    CHECK(value_of(must_parse("[a]\nx = \"not # a comment\"\n"), "a", "x").as_string() ==
          "not # a comment");
    CHECK(value_of(must_parse("[a]\nx = \"#\"\n"), "a", "x").as_string() == "#");
}

TEST_CASE("a hash after a string closes is a comment", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = \"v\" # gone\n"), "a", "x").as_string() == "v");
}

TEST_CASE("an escaped quote does not end the string for comment purposes", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = \"a\\\"# b\"\n"), "a", "x").as_string() == "a\"# b");
}

// ---------------------------------------------------------------------------
// Section headers
// ---------------------------------------------------------------------------

TEST_CASE("a section header must be a plain identifier", "[toml][L2-CFG-010]") {
    CHECK(must_parse("[decode]\n").size() == 0);
    CHECK(must_parse("[with_underscore]\n").size() == 0);
    // A dash is NOT in the alphabet. Python's _IDENT_RE is `^\w+$` under
    // re.ASCII and Rust's check is `is_ascii_alphanumeric() || '_'`, so
    // accepting one here would make a shared config file mean different
    // things on different implementations.
    must_reject("[with-dash]\n");
    CHECK(must_parse("[a1]\n").size() == 0);

    // Each of these is valid TOML that the flat schema cannot model.
    must_reject("[a.b]\n");
    must_reject("[\"quoted\"]\n");
    must_reject("[has space]\n");
    must_reject("[]\n");
    must_reject("[unterminated\n");
    must_reject("[[array]]\n");
}

TEST_CASE("a repeated section is refused", "[toml][L2-CFG-010]") {
    // The TOML spec forbids defining a table twice. Without the check the
    // second block silently merged into the first.
    CHECK(must_reject("[decode]\nx = 1\n[decode]\ny = 2\n").message ==
          "section [decode] declared more than once");
}

TEST_CASE("whitespace inside the brackets is trimmed", "[toml]") {
    CHECK(must_parse("[  decode  ]\nx = 1\n").at(0).section == "decode");
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

TEST_CASE("a key must be a plain identifier", "[toml][L2-CFG-010]") {
    CHECK(must_parse("[a]\nplain = 1\n").size() == 1);
    CHECK(must_parse("[a]\nwith_underscore = 1\n").size() == 1);
    must_reject("[a]\nwith-dash = 1\n");

    // Dotted keys are NESTED by tomllib, honouring the value. Dropping one here
    // would silently ignore a safety option such as output.no_clobber.
    must_reject("[a]\nb.c = 1\n");
    // A quoted key is honoured by tomllib with the quotes stripped, but would
    // be stored here with them attached -- the same key under two names.
    must_reject("[a]\n\"quoted\" = 1\n");
    must_reject("[a]\nhas space = 1\n");
    must_reject("[a]\n= 1\n");
    must_reject("[a]\nnoequals\n");
}

TEST_CASE("a repeated key is refused, and the message names it", "[toml][L2-CFG-010]") {
    // tomllib raises per the spec. Without this the FIRST value silently won,
    // so a repeated key decoded differently on each implementation.
    const tml::ParseError in_section = must_reject("[a]\nx = 1\nx = 2\n");
    CHECK(in_section.message == "duplicate key 'x' in section '[a]'");
    CHECK(in_section.line == 3);

    const tml::ParseError at_root = must_reject("x = 1\nx = 2\n");
    CHECK(at_root.message == "duplicate key 'x'");
}

TEST_CASE("whitespace around the equals sign is trimmed", "[toml]") {
    CHECK(value_of(must_parse("[a]\n   x   =   1   \n"), "a", "x").as_integer() == 1);
}

// ---------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------

TEST_CASE("arrays hold the values they list", "[toml]") {
    const tml::Value ints = value_of(must_parse("[a]\nx = [1, 2, 3]\n"), "a", "x");
    REQUIRE(ints.as_array().size() == 3);
    CHECK(ints.as_array()[0].as_integer() == 1);
    CHECK(ints.as_array()[2].as_integer() == 3);

    const tml::Value strings = value_of(must_parse("[a]\nx = [\"A\", \"B\"]\n"), "a", "x");
    REQUIRE(strings.as_array().size() == 2);
    CHECK(strings.as_array()[1].as_string() == "B");
}

TEST_CASE("an empty array is valid", "[toml]") {
    CHECK(value_of(must_parse("[a]\nx = []\n"), "a", "x").as_array().empty());
    CHECK(value_of(must_parse("[a]\nx = [  ]\n"), "a", "x").as_array().empty());
}

TEST_CASE("a comma inside a quoted array item does not split it", "[toml]") {
    const tml::Value v = value_of(must_parse("[a]\nx = [\"one,two\", \"three\"]\n"), "a", "x");
    REQUIRE(v.as_array().size() == 2);
    CHECK(v.as_array()[0].as_string() == "one,two");
}

TEST_CASE("an escaped quote inside an array item does not end it", "[toml]") {
    const tml::Value v = value_of(must_parse("[a]\nx = [\"a\\\", b\"]\n"), "a", "x");
    REQUIRE(v.as_array().size() == 1);
    CHECK(v.as_array()[0].as_string() == "a\", b");
}

TEST_CASE("a trailing comma is tolerated", "[toml]") {
    const tml::Value v = value_of(must_parse("[a]\nx = [1, 2, ]\n"), "a", "x");
    CHECK(v.as_array().size() == 2);
}

TEST_CASE("nested arrays are refused", "[toml]") {
    // The flat schema has no key whose value is an array of arrays, so storing
    // one would create a shape nothing reads.
    CHECK(must_reject("[a]\nx = [[1]]\n").message == "nested arrays not supported");
}

TEST_CASE("a multi-line array is refused", "[toml][L2-CFG-010]") {
    // Valid TOML. This parser is line-oriented, and quietly accepting the first
    // line would store a DIFFERENT array than the file spells.
    must_reject("[a]\nx = [\n  1,\n]\n");
}

TEST_CASE("an inline table is refused", "[toml][L2-CFG-010]") {
    must_reject("[a]\nx = { b = 1 }\n");
}

// ---------------------------------------------------------------------------
// The number grammar, exhaustively
// ---------------------------------------------------------------------------

TEST_CASE("every literal the grammar generates is accepted", "[toml][number]") {
    // Built FROM the productions rather than listed by hand, so the corpus
    // cannot quietly omit a shape the grammar allows.
    const char* const signs[] = {"", "+", "-"};
    const char* const integers[] = {"0", "1", "9", "10", "907", "1234567890"};
    const char* const fractions[] = {"", ".0", ".5", ".000001", ".1234567890"};
    const char* const exponents[] = {"", "e1", "E1", "e+1", "e-1", "E+10", "e-10", "e0"};

    std::size_t checked = 0;
    for (std::size_t s = 0; s < 3; ++s) {
        for (std::size_t i = 0; i < 6; ++i) {
            for (std::size_t f = 0; f < 5; ++f) {
                for (std::size_t e = 0; e < 8; ++e) {
                    const std::string literal =
                        std::string(signs[s]) + integers[i] + fractions[f] + exponents[e];
                    INFO("literal: " << literal);
                    CHECK(tml::is_number_literal(literal));
                    // A necessary property of a well-formed literal: the C
                    // library consumes ALL of it. This is a cross-check against
                    // an independent parser, not a restatement of the grammar.
                    const char* c_str = literal.c_str();
                    char* end = 0;
                    // The value is not the question; how much of the string was
                    // consumed is, and that comes back through `end`.
                    (void)std::strtod(c_str, &end);
                    CHECK(end == c_str + literal.size());
                    checked += 1;
                }
            }
        }
    }
    CHECK(checked == static_cast<std::size_t>(3) * 6 * 5 * 8);
}

TEST_CASE("the forms a native parser would accept are refused", "[toml][number]") {
    // Every entry here is taken by strtod or strtoll and refused by TOML. This
    // is the divergence the grammar check exists to prevent.
    const char* const rejected[] = {
        "08",    "01",    "0123",               // leading zeros
        "1.",    "1.e5",  ".5",   "-.5",        // a bare or missing fractional part
        "1e",    "1e+",   "1E-",                // an exponent marker with no digits
        "0x10",  "0o17",  "0b1",                // radix prefixes
        "1_000", "1__0",                        // digit separators
        " 1",    "1 ",    "1 2",                // surrounding or embedded space
        "+",     "-",     "",     ".",   "e5",  // no integer part at all
        "1.2.3", "1e5e5",                       // repeated productions
        "inf",   "nan",   "+inf",               // TOML spells these differently
        "--1",   "++1",   "1-",   "1+",         // stray signs
    };
    for (std::size_t i = 0; i < sizeof(rejected) / sizeof(rejected[0]); ++i) {
        INFO("literal: " << rejected[i]);
        CHECK_FALSE(tml::is_number_literal(rejected[i]));
    }
}

TEST_CASE("the number grammar is swept exhaustively over its own alphabet", "[toml][number]") {
    // Every string up to four characters over the alphabet the grammar uses.
    // 7 + 49 + 343 + 2401 = 2800 candidates, which is small enough to be
    // exhaustive and wide enough to contain every shape the curated tables
    // above name plus every shape nobody thought of.
    //
    // The assertion is a PROPERTY, not a second copy of the grammar: an
    // accepted literal must be one the C library also consumes completely.
    // Restating the productions here would just be the same code twice, and
    // two copies of one mistake agree with each other.
    const char alphabet[] = {'0', '1', '+', '-', '.', 'e', '_'};
    const std::size_t n = sizeof(alphabet) / sizeof(alphabet[0]);

    std::size_t accepted = 0;
    std::size_t rejected = 0;
    for (std::size_t length = 1; length <= 4; ++length) {
        std::vector<std::size_t> index(length, 0);
        for (;;) {
            std::string candidate;
            for (std::size_t i = 0; i < length; ++i) {
                candidate += alphabet[index[i]];
            }

            if (tml::is_number_literal(candidate)) {
                accepted += 1;
                const char* c_str = candidate.c_str();
                char* end = 0;
                (void)std::strtod(c_str, &end);
                INFO("accepted but not fully consumed by strtod: " << candidate);
                CHECK(end == c_str + candidate.size());
                // An underscore never appears in a literal this grammar takes.
                INFO("accepted with a digit separator: " << candidate);
                CHECK(candidate.find('_') == std::string::npos);
            } else {
                rejected += 1;
            }

            std::size_t position = length;
            while (position > 0) {
                position -= 1;
                index[position] += 1;
                if (index[position] < n) {
                    break;
                }
                index[position] = 0;
                if (position == 0) {
                    position = static_cast<std::size_t>(-1);
                    break;
                }
            }
            if (position == static_cast<std::size_t>(-1)) {
                break;
            }
        }
    }
    CHECK(accepted + rejected == static_cast<std::size_t>(7) + 49 + 343 + 2401);
    // A sweep that accepted nothing, or everything, would pass a
    // property-only assertion while testing nothing at all.
    CHECK(accepted > 0);
    CHECK(rejected > accepted);
}

TEST_CASE("integers and floats are told apart by their spelling", "[toml][number]") {
    // A decimal point or an exponent makes it a float; nothing else does. The
    // schema cares: detect_records is an integer key and rejects 8.0.
    CHECK(value_of(must_parse("[a]\nx = 8\n"), "a", "x").kind() == tml::VALUE_INTEGER);
    CHECK(value_of(must_parse("[a]\nx = 8.0\n"), "a", "x").kind() == tml::VALUE_FLOAT);
    CHECK(value_of(must_parse("[a]\nx = 8e0\n"), "a", "x").kind() == tml::VALUE_FLOAT);
    CHECK(value_of(must_parse("[a]\nx = 8E0\n"), "a", "x").kind() == tml::VALUE_FLOAT);
    CHECK(value_of(must_parse("[a]\nx = -8\n"), "a", "x").kind() == tml::VALUE_INTEGER);
}

TEST_CASE("a rejected number names the literal", "[toml][number]") {
    const tml::ParseError error = must_reject("[a]\nx = 08\n");
    CHECK(error.line == 2);
    CHECK(error.message == "invalid number literal \"08\"");
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

TEST_CASE("is_plain_identifier accepts exactly the identifier alphabet", "[toml]") {
    CHECK(tml::is_plain_identifier("a"));
    CHECK(tml::is_plain_identifier("A0_z"));
    CHECK_FALSE(tml::is_plain_identifier("a-b"));
    CHECK_FALSE(tml::is_plain_identifier(""));
    CHECK_FALSE(tml::is_plain_identifier("a.b"));
    CHECK_FALSE(tml::is_plain_identifier("a b"));
    CHECK_FALSE(tml::is_plain_identifier("\"a\""));
    CHECK_FALSE(tml::is_plain_identifier("a\t"));
}

TEST_CASE("identifier classification is swept over every byte", "[toml]") {
    // Exhaustive over all 256 byte values, and the reason is locale: the
    // classification MUST use explicit ASCII ranges rather than <cctype>,
    // because under tr_TR the case mapping of 'I' changes and a locale-driven
    // classifier would accept a different set of key names on a Turkish host.
    for (int byte = 0; byte < 256; ++byte) {
        const std::string one(1, static_cast<char>(byte));
        const bool expected = (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') ||
                              (byte >= 'a' && byte <= 'z') || byte == '_';
        INFO("byte: " << byte);
        CHECK(tml::is_plain_identifier(one) == expected);
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

TEST_CASE("an error reports the line it happened on", "[toml]") {
    CHECK(must_reject("[a]\nx = 1\ny = @\n").line == 3);
    CHECK(must_reject("\n\n[a.b]\n").line == 3);
    // Comment and blank lines still count, so the number matches the editor.
    CHECK(must_reject("# one\n\n# three\nbad line\n").line == 4);
}

TEST_CASE("format composes the line prefix, and the message omits it", "[toml]") {
    // The message is prefix-free so a caller embedding it in its own sentence
    // is not forced to strip "line N:" back out.
    const tml::ParseError error = must_reject("[a]\nx = 08\n");
    CHECK(error.message.find("line") == std::string::npos);
    CHECK(error.format() == "line 2: invalid number literal \"08\"");
}

TEST_CASE("a line-independent error formats without a prefix", "[toml]") {
    tml::ParseError error;
    error.message = "something general";
    CHECK(error.line == 0);
    CHECK(error.format() == "something general");
}
