# Foundation — Errors + Quoting + Command-Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, IO-free foundation of the `subprocess` crate — the error taxonomy, the argv quoting/splitting engines (POSIX shlex + Windows `CommandLineToArgvW`-compatible), and the command-input data model — all unit-testable on every OS.

**Architecture:** This is Layer 1 (pure core) from the design spec. No spawning, no syscalls, no `cfg`-gated runtime behavior. Two quoting engines operate on raw code units (POSIX over `&[u8]`, Windows over `&[u16]`) so both are testable on any host. The `Command` type here holds only the *input* model (executable/args/commandline) and its setters; wiring the quoters into a real spawn (resolution-to-native + PATH) is deferred to the later "Stdio + first sync spawn" plan.

**Tech Stack:** Rust (MSRV 1.87, edition 2021), `thiserror` for errors. Quoting ported from JetBrains qodana-cli `internal/foundation/shlex` (Apache-2.0, user-authored) and the Rust-std/MSVCRT Windows quoting rules.

## Global Constraints

- MSRV: **1.87** (`rust-version = "1.87"`). Unlocks `std::io::pipe` for later plans. (`std::iter::repeat_n`, used in Task 5, stabilized in 1.82 — available.)
- Platforms are equals: Windows + Linux + macOS. No first/second tier. (No platform-specific behavior in *this* plan — it is all pure.)
- Quoting is a **security boundary** (BatBadBut / CVE-2024-24576): correctness is non-negotiable; port the test corpus as the oracle and never claim OS-fidelity the code does not have.
- POSIX quoting MUST stay **byte-oriented** (`&[u8]` / `Vec<u8>`), never `&str` — preserves NUL-safety and non-UTF-8 argv.
- Tests **never skip on missing deps**; fail loudly. No time-based synchronization. No reliance on data races. (No concurrency in this plan.)
- Single crate; `foo.rs` + `foo/` module style, **no `mod.rs`**; unit tests in sibling `*_tests.rs` files via `#[path]`.
- **Incremental module wiring:** every task that creates a module declares it in `src/lib.rs` *in that same task*, so each task's red/green test runs really run. There is no "stub then restore" — `lib.rs` only ever grows.
- Commit messages: single line (metadata after `\n` only).
- Published crate name is TBD; working name is `subprocess`.

---

### Task 1: Crate scaffold + repository

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `.gitignore`
- (Already present) `TODO.md`, `.tmp/claude/superpowers/specs/2026-06-20-subprocess-design.md`, this plan.

**Interfaces:**
- Consumes: nothing.
- Produces: a compiling, empty library crate `subprocess`; `thiserror` available as a dependency.

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "subprocess"
version = "0.0.0"
edition = "2021"
rust-version = "1.87"
description = "Unified cross-platform subprocess management: spawning, stdio, process trees, stable identity (and, later, elevation)."
license = "MIT OR Apache-2.0"
publish = false           # working name; not yet published

[dependencies]
thiserror = "2"

[lints.rust]
unsafe_op_in_unsafe_fn = "warn"

[lints.clippy]
all = "warn"
```

- [ ] **Step 2: Create `.gitignore`**

Note: git only treats `#` as a comment when it is the first character of a line — no trailing inline comments. The agent scratch tree `.tmp/` is ignored wholesale; the design docs we want tracked are force-added in Step 5.

```gitignore
/target
**/*.rs.bk

# library crate: do not commit the lockfile
Cargo.lock

# agent scratch (specs/plans are force-added in git)
.tmp/
```

- [ ] **Step 3: Create `src/lib.rs` (empty crate — modules are wired in by later tasks)**

```rust
//! `subprocess`: unified cross-platform subprocess management.
//!
//! Under construction. The first landed layer is the pure core: the error
//! taxonomy, argv quoting, and the command input model. Modules are added by
//! the foundation plan task-by-task.
```

- [ ] **Step 4: Verify the empty crate builds**

Run: `cargo build`
Expected: `Finished` with no errors (an empty library crate).

- [ ] **Step 5: Initialize git and commit the scaffold + design docs**

```bash
cd /c/Users/bindreams/src/subprocess
git init
git add Cargo.toml .gitignore src/lib.rs TODO.md
git add -f .tmp/claude/superpowers/specs/2026-06-20-subprocess-design.md
git add -f .tmp/claude/superpowers/plans/2026-06-20-foundation-errors-quoting-command-input.md
git commit -m "chore: scaffold subprocess crate with design spec and TODO"
```

Verify the lockfile is actually ignored (catches the inline-comment trap):

Run: `git check-ignore Cargo.lock`
Expected: prints `Cargo.lock` (i.e. it IS ignored). If it prints nothing, the `.gitignore` is wrong.

---

### Task 2: Error taxonomy

**Files:**
- Modify: `src/lib.rs` (add `pub mod error;`)
- Create: `src/error.rs`
- Create: `src/error_tests.rs`

**Interfaces:**
- Consumes: `thiserror`.
- Produces:
  - `pub struct QuoteError { pub pos: usize, pub kind: QuoteErrorKind }` with `pub(crate) fn new(pos: usize, kind: QuoteErrorKind) -> QuoteError`.
  - `pub enum QuoteErrorKind { UnterminatedSingleQuote, UnterminatedDoubleQuote, TrailingBackslash }` (derives `Clone, Copy, Debug, PartialEq, Eq`).
  - `pub enum Error { Quote(QuoteError), Io(std::io::Error) }` with `#[from]` conversions; implements `std::error::Error`.

- [ ] **Step 1: Write the failing test and wire the module skeleton**

Create `src/error_tests.rs`:

```rust
use crate::error::{Error, QuoteError, QuoteErrorKind};

#[test]
fn quote_error_displays_kind_and_offset() {
    let e = QuoteError::new(7, QuoteErrorKind::UnterminatedSingleQuote);
    assert_eq!(e.to_string(), "unterminated single quote at offset 7");
}

#[test]
fn quote_error_kinds_have_distinct_messages() {
    assert_eq!(
        QuoteErrorKind::UnterminatedDoubleQuote.to_string(),
        "unterminated double quote"
    );
    assert_eq!(
        QuoteErrorKind::TrailingBackslash.to_string(),
        "trailing backslash"
    );
}

#[test]
fn error_wraps_quote_error_via_from() {
    let e: Error = QuoteError::new(0, QuoteErrorKind::TrailingBackslash).into();
    assert!(matches!(e, Error::Quote(_)));
    assert!(e.to_string().contains("trailing backslash"));
}
```

Create `src/error.rs` as a skeleton that wires the test module but defines nothing yet (so the test compiles-fails):

```rust
//! Crate error taxonomy. Extended by later plans (spawn, containment, identity).

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
```

Add to `src/lib.rs` (after the doc comment):

```rust
pub mod error;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib error`
Expected: FAIL — compile error `E0432: unresolved import` / `cannot find type QuoteError` (the types aren't defined yet).

- [ ] **Step 3: Write minimal implementation**

Replace `src/error.rs` with:

```rust
//! Crate error taxonomy. Extended by later plans (spawn, containment, identity).

/// Why splitting a command line failed. `pos` is a 0-based byte offset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind} at offset {pos}")]
pub struct QuoteError {
    pub pos: usize,
    pub kind: QuoteErrorKind,
}

impl QuoteError {
    pub(crate) fn new(pos: usize, kind: QuoteErrorKind) -> Self {
        QuoteError { pos, kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuoteErrorKind {
    #[error("unterminated single quote")]
    UnterminatedSingleQuote,
    #[error("unterminated double quote")]
    UnterminatedDoubleQuote,
    #[error("trailing backslash")]
    TrailingBackslash,
}

/// The crate's top-level error type. Grows as later plans add fallible operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("argument parsing failed: {0}")]
    Quote(#[from] QuoteError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib error`
Expected: PASS — `running 3 tests ... test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/error.rs src/error_tests.rs
git commit -m "feat: add error taxonomy (QuoteError, Error)"
```

---

### Task 3: POSIX shlex `split`

Port of qodana-cli `internal/foundation/shlex/split.go`. Byte-oriented, strict IEEE 1003.1-2024 §2.2 with the three documented deviations (CR-as-whitespace, `#` not a comment, no expansion).

**Files:**
- Modify: `src/lib.rs` (add `pub mod quote;`)
- Create: `src/quote.rs`
- Create: `src/quote/posix.rs`
- Create: `src/quote/posix_tests.rs`
- Create: `src/quote/windows.rs` (doc-only placeholder; filled in Task 5)

**Interfaces:**
- Consumes: `crate::error::{QuoteError, QuoteErrorKind}`.
- Produces: `pub fn crate::quote::posix::split(s: &[u8]) -> Result<Vec<Vec<u8>>, QuoteError>`.

- [ ] **Step 1: Write the failing test and wire the module skeleton**

Create `src/quote/posix_tests.rs`:

```rust
use crate::error::QuoteErrorKind;
use crate::quote::posix::split;

fn words(s: &str) -> Vec<Vec<u8>> {
    split(s.as_bytes()).expect("should parse")
}
fn strs(s: &str) -> Vec<String> {
    words(s).into_iter().map(|w| String::from_utf8(w).unwrap()).collect()
}

#[test]
fn empty_and_whitespace_yield_no_words() {
    assert!(split(b"").unwrap().is_empty());
    assert!(split(b"   \t \n ").unwrap().is_empty());
}

#[test]
fn plain_words_split_on_runs_of_whitespace() {
    assert_eq!(strs("a b c"), ["a", "b", "c"]);
    assert_eq!(strs("a   b\t\tc"), ["a", "b", "c"]);
}

#[test]
fn single_quotes_are_fully_literal() {
    assert_eq!(strs(r#"'a b' c"#), ["a b", "c"]);
    assert_eq!(strs(r#"'a\$b'"#), [r"a\$b"]); // backslash literal inside single quotes
}

#[test]
fn empty_single_quotes_emit_empty_token() {
    assert_eq!(strs("''"), [""]);
}

#[test]
fn double_quotes_consume_backslash_only_before_special() {
    assert_eq!(strs(r#""a\"b""#), [r#"a"b"#]);   // \" -> "
    assert_eq!(strs(r#""a\$b""#), ["a$b"]);       // \$ -> $   (POSIX, not Python)
    assert_eq!(strs(r#""a\`b""#), ["a`b"]);       // \` -> `
    assert_eq!(strs(r#""a\xb""#), [r"a\xb"]);    // \x -> \x  (backslash literal)
}

#[test]
fn unquoted_backslash_escapes_next_byte() {
    assert_eq!(strs(r"a\ b"), ["a b"]);
}

#[test]
fn hash_is_never_a_comment() {
    // Documented deviation #2: '#' is a literal word byte, not a comment introducer.
    assert_eq!(strs("a#b c"), ["a#b", "c"]);
}

#[test]
fn adjacent_quoting_concatenates_into_one_word() {
    assert_eq!(strs(r#"foo"bar"'baz'"#), ["foobarbaz"]);
}

#[test]
fn line_continuation_is_removed() {
    assert_eq!(strs("a\\\nb"), ["ab"]);       // \<LF>
    assert_eq!(strs("a\\\r\nb"), ["ab"]);     // \<CR><LF>
}

#[test]
fn bare_cr_is_whitespace_not_continuation() {
    // \<CR> with no following <LF> falls through to the ordinary \X rule: CR becomes literal.
    assert_eq!(words("a\\\rb"), [b"a\rb".to_vec()]);
}

#[test]
fn non_ascii_bytes_pass_through() {
    let input = [b'a', 0xC3, 0xA9, b'b']; // "a", 0xC3 0xA9 (é utf8), "b"
    assert_eq!(split(&input).unwrap(), vec![input.to_vec()]);
}

#[test]
fn nul_byte_is_a_valid_word_byte() {
    assert_eq!(split(b"a\0b").unwrap(), vec![b"a\0b".to_vec()]);
}

#[test]
fn errors_report_kind_and_offset() {
    let e = split(b"a\\").unwrap_err();
    assert_eq!(e.kind, QuoteErrorKind::TrailingBackslash);
    assert_eq!(e.pos, 1);

    let e = split(b"  'oops").unwrap_err();
    assert_eq!(e.kind, QuoteErrorKind::UnterminatedSingleQuote);
    assert_eq!(e.pos, 2); // offset of the opening quote

    let e = split(br#""oops"#).unwrap_err();
    assert_eq!(e.kind, QuoteErrorKind::UnterminatedDoubleQuote);
    assert_eq!(e.pos, 0);
}
```

Create `src/quote.rs`:

```rust
//! Argv quoting/splitting. POSIX operates on bytes; Windows on UTF-16 code units.
//! Both are pure and unit-testable on any host.

pub mod posix;
pub mod windows;
```

Create `src/quote/windows.rs` (placeholder so `pub mod windows;` compiles; filled in Task 5):

```rust
//! Windows `CommandLineToArgvW`-compatible quoting. Filled in by the
//! "Windows command-line join" task.
```

Create `src/quote/posix.rs` as a skeleton that wires the test module but defines nothing yet:

```rust
//! Strict-POSIX (IEEE 1003.1-2024 §2.2) word splitting, byte-oriented.
//! Ported from JetBrains qodana-cli `internal/foundation/shlex` (Apache-2.0).

#[cfg(test)]
#[path = "posix_tests.rs"]
mod posix_tests;
```

Add to `src/lib.rs` (after `pub mod error;`):

```rust
pub mod quote;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib quote::posix`
Expected: FAIL — compile error `cannot find function split in module crate::quote::posix`.

- [ ] **Step 3: Write minimal implementation**

Replace `src/quote/posix.rs` with:

```rust
//! Strict-POSIX (IEEE 1003.1-2024 §2.2) word splitting, byte-oriented.
//!
//! Ported from JetBrains qodana-cli `internal/foundation/shlex` (Apache-2.0).
//! Deviations from strict POSIX, preserved from the original:
//!   1. CR (0x0D) is whitespace; `\<CR><LF>` is a line continuation. A bare
//!      `\<CR>` (no `<LF>`) is NOT a continuation — it falls to the `\X` rule.
//!   2. `#` is never a comment introducer (we split argv, not scripts).
//!   3. `$`, backtick, `$(...)`, `${...}` are literal — this is a lexer, not a shell.

use crate::error::{QuoteError, QuoteErrorKind};

#[derive(Clone, Copy)]
enum State {
    Whitespace,
    Word,
    SingleQuote,
    DoubleQuote,
}

/// Split `s` into argv words. Empty/whitespace-only input yields an empty vec.
pub fn split(s: &[u8]) -> Result<Vec<Vec<u8>>, QuoteError> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut state = State::Whitespace;
    let mut has_token = false;
    let mut quote_open = 0usize;
    let mut i = 0usize;

    while i < s.len() {
        let c = s[i];
        match state {
            State::Whitespace => {
                if is_ws(c) {
                    i += 1;
                } else if c == b'\'' {
                    state = State::SingleQuote;
                    has_token = true;
                    quote_open = i;
                    i += 1;
                } else if c == b'"' {
                    state = State::DoubleQuote;
                    has_token = true;
                    quote_open = i;
                    i += 1;
                } else if c == b'\\' {
                    i = backslash_unquoted(s, i, &mut buf)?;
                    has_token = true;
                    state = State::Word;
                } else {
                    buf.push(c);
                    has_token = true;
                    state = State::Word;
                    i += 1;
                }
            }
            State::Word => {
                if is_ws(c) {
                    emit(&mut out, &mut buf, &mut has_token);
                    state = State::Whitespace;
                    i += 1;
                } else if c == b'\'' {
                    state = State::SingleQuote;
                    quote_open = i;
                    i += 1;
                } else if c == b'"' {
                    state = State::DoubleQuote;
                    quote_open = i;
                    i += 1;
                } else if c == b'\\' {
                    i = backslash_unquoted(s, i, &mut buf)?;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            State::SingleQuote => {
                if c == b'\'' {
                    state = State::Word;
                } else {
                    buf.push(c);
                }
                i += 1;
            }
            State::DoubleQuote => {
                if c == b'"' {
                    state = State::Word;
                    i += 1;
                } else if c == b'\\' {
                    i = backslash_in_dquote(s, i, &mut buf)?;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
        }
    }

    match state {
        State::Whitespace | State::Word => {
            emit(&mut out, &mut buf, &mut has_token);
            Ok(out)
        }
        State::SingleQuote => Err(QuoteError::new(quote_open, QuoteErrorKind::UnterminatedSingleQuote)),
        State::DoubleQuote => Err(QuoteError::new(quote_open, QuoteErrorKind::UnterminatedDoubleQuote)),
    }
}

fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r'
}

fn emit(out: &mut Vec<Vec<u8>>, buf: &mut Vec<u8>, has_token: &mut bool) {
    if *has_token {
        out.push(std::mem::take(buf));
        *has_token = false;
    }
}

/// `i` is at a backslash, outside any quotes. Returns the next index.
fn backslash_unquoted(s: &[u8], i: usize, buf: &mut Vec<u8>) -> Result<usize, QuoteError> {
    if i + 1 >= s.len() {
        return Err(QuoteError::new(i, QuoteErrorKind::TrailingBackslash));
    }
    if let Some(consumed) = line_continuation(s, i) {
        return Ok(i + consumed);
    }
    buf.push(s[i + 1]);
    Ok(i + 2)
}

/// `i` is at a backslash, inside double quotes. Returns the next index.
fn backslash_in_dquote(s: &[u8], i: usize, buf: &mut Vec<u8>) -> Result<usize, QuoteError> {
    if i + 1 >= s.len() {
        return Err(QuoteError::new(i, QuoteErrorKind::TrailingBackslash));
    }
    if let Some(consumed) = line_continuation(s, i) {
        return Ok(i + consumed);
    }
    match s[i + 1] {
        n @ (b'$' | b'`' | b'"' | b'\\') => {
            buf.push(n);
            Ok(i + 2)
        }
        _ => {
            // Backslash is literal; the next byte is re-processed next iteration.
            buf.push(b'\\');
            Ok(i + 1)
        }
    }
}

/// If `i` (at a backslash) begins a line continuation, return its total byte
/// length (2 for `\<LF>`, 3 for `\<CR><LF>`). Precondition: `i + 1 < s.len()`.
fn line_continuation(s: &[u8], i: usize) -> Option<usize> {
    match s[i + 1] {
        b'\n' => Some(2),
        b'\r' if i + 2 < s.len() && s[i + 2] == b'\n' => Some(3),
        _ => None,
    }
}

#[cfg(test)]
#[path = "posix_tests.rs"]
mod posix_tests;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib quote::posix`
Expected: PASS — all `posix_tests` (13 tests) pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/quote.rs src/quote/posix.rs src/quote/posix_tests.rs src/quote/windows.rs
git commit -m "feat: add byte-oriented POSIX shlex split"
```

---

### Task 4: POSIX shlex `quote` + `join` (with round-trip property)

Port of qodana `join.go` (CPython-compatible safe set; `'"'"'` escaping).

**Files:**
- Modify: `src/quote/posix.rs` (append `quote`, `join`)
- Modify: `src/quote/posix_tests.rs` (append tests)

**Interfaces:**
- Produces:
  - `pub fn crate::quote::posix::quote(s: &[u8]) -> Vec<u8>`
  - `pub fn crate::quote::posix::join(args: &[&[u8]]) -> Vec<u8>`
- Guarantees: `split(join(args)) == args` for all `args` (the round-trip property).

- [ ] **Step 1: Write the failing test**

Append to `src/quote/posix_tests.rs`:

```rust
use crate::quote::posix::{join, quote};

#[test]
fn quote_passes_safe_strings_through() {
    assert_eq!(quote(b"abcXYZ_0-9@%+=:,./"), b"abcXYZ_0-9@%+=:,./");
}

#[test]
fn quote_wraps_empty_in_single_quotes() {
    assert_eq!(quote(b""), b"''");
}

#[test]
fn quote_wraps_unsafe_and_escapes_single_quotes() {
    assert_eq!(quote(b"a b"), b"'a b'");
    assert_eq!(quote(b"it's"), br#"'it'"'"'s'"#);
}

#[test]
fn join_empty_is_empty() {
    let empty: Vec<&[u8]> = vec![];
    assert_eq!(join(&empty), b"");
}

#[test]
fn join_separates_with_single_space() {
    let args: Vec<&[u8]> = vec![b"a", b"b c", b"d"];
    assert_eq!(join(&args), b"a 'b c' d");
}

#[test]
fn split_join_round_trips() {
    let cases: Vec<Vec<&[u8]>> = vec![
        vec![b"echo", b"hello world"],
        vec![b"a", b"", b"c"],          // empty element
        vec![br"weird $`\ chars", b"'q'"],
        vec![b"a\nb", b"\ttab"],
        vec![b"\x00\xff", b"non-utf8"], // arbitrary bytes
    ];
    for args in cases {
        let line = join(&args);
        let back = split(&line).unwrap();
        let expected: Vec<Vec<u8>> = args.iter().map(|a| a.to_vec()).collect();
        assert_eq!(back, expected, "round-trip failed for {:?}", args);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib quote::posix`
Expected: FAIL — `cannot find function quote`/`join` in module.

- [ ] **Step 3: Write minimal implementation**

Append to `src/quote/posix.rs` (above the `#[cfg(test)]` block):

```rust
/// Safe bytes need no quoting (CPython `shlex` safe set: alnum plus `_@%+=:,./-`).
const SAFE: [bool; 256] = build_safe();

const fn build_safe() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0usize;
    while i < 256 {
        let b = i as u8;
        t[i] = matches!(b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
            | b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-');
        i += 1;
    }
    t
}

/// Shell-escape `s` so `split` parses it back as exactly one token.
/// NOT idempotent on unsafe input — required for round-trip correctness.
pub fn quote(s: &[u8]) -> Vec<u8> {
    if s.is_empty() {
        return b"''".to_vec();
    }
    if s.iter().all(|&b| SAFE[b as usize]) {
        return s.to_vec();
    }
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s {
        if b == b'\'' {
            out.extend_from_slice(br#"'"'"'"#);
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// Join argv into a POSIX command line such that `split(join(args)) == args`.
pub fn join(args: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for (idx, a) in args.iter().enumerate() {
        if idx > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(&quote(a));
    }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib quote::posix`
Expected: PASS (Task 3 + Task 4 tests, 19 total).

- [ ] **Step 5: Commit**

```bash
git add src/quote/posix.rs src/quote/posix_tests.rs
git commit -m "feat: add POSIX shlex quote/join with round-trip property"
```

---

### Task 5: Windows command-line `join`

MSVCRT / `CommandLineToArgvW` quoting rules over UTF-16 code units. Pure algorithm (`join_wide`) testable everywhere; a `#[cfg(windows)]` round-trip test validates against the real OS parser.

**Files:**
- Modify: `src/quote/windows.rs`
- Create: `src/quote/windows_tests.rs`

**Interfaces:**
- Produces: `pub fn crate::quote::windows::join_wide(args: &[&[u16]]) -> Vec<u16>`.

- [ ] **Step 1: Write the failing test**

Create `src/quote/windows_tests.rs`:

```rust
use crate::quote::windows::join_wide;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
fn jw(args: &[&str]) -> String {
    let wides: Vec<Vec<u16>> = args.iter().map(|a| w(a)).collect();
    let refs: Vec<&[u16]> = wides.iter().map(|v| v.as_slice()).collect();
    String::from_utf16(&join_wide(&refs)).unwrap()
}

#[test]
fn simple_args_separated_by_space() {
    assert_eq!(jw(&["a", "b"]), "a b");
}

#[test]
fn args_with_space_or_tab_are_quoted() {
    assert_eq!(jw(&["a b"]), "\"a b\"");
    assert_eq!(jw(&["a\tb"]), "\"a\tb\"");
}

#[test]
fn empty_arg_becomes_empty_quotes() {
    assert_eq!(jw(&["a", "", "b"]), "a \"\" b");
}

#[test]
fn embedded_quote_is_backslash_escaped() {
    // a"b  ->  a\"b
    assert_eq!(jw(&["a\"b"]), "a\\\"b");
}

#[test]
fn lone_backslashes_not_before_quote_stay_literal() {
    assert_eq!(jw(&["a\\b"]), "a\\b");
    assert_eq!(jw(&["a\\"]), "a\\");
}

#[test]
fn backslashes_before_quote_are_doubled_plus_one() {
    // a\"b  ->  a\\\"b   (one backslash + escaped quote)
    assert_eq!(jw(&["a\\\"b"]), "a\\\\\\\"b");
}

#[test]
fn trailing_backslashes_doubled_before_closing_quote() {
    assert_eq!(jw(&["a\\ b"]), "\"a\\ b\""); // single backslash, space forces quotes
    assert_eq!(jw(&["a b\\"]), "\"a b\\\\\""); // trailing \ doubled before closing "
}
```

Append the Windows-only round-trip oracle to the same file:

```rust
#[cfg(windows)]
mod roundtrip {
    use super::*;

    #[link(name = "shell32")]
    extern "system" {
        fn CommandLineToArgvW(lp_cmd_line: *const u16, p_num_args: *mut i32) -> *mut *mut u16;
    }
    extern "system" {
        fn LocalFree(h_mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    }

    // Parse a command line the way the OS does. Returns the argv vector.
    fn os_parse(cmdline: &[u16]) -> Vec<Vec<u16>> {
        let mut buf: Vec<u16> = cmdline.to_vec();
        buf.push(0); // NUL terminate
        let mut n: i32 = 0;
        // SAFETY: buf is NUL-terminated; the returned array is freed with LocalFree.
        unsafe {
            let argv = CommandLineToArgvW(buf.as_ptr(), &mut n);
            assert!(!argv.is_null(), "CommandLineToArgvW failed");
            let mut out = Vec::with_capacity(n as usize);
            for i in 0..n as isize {
                let p = *argv.offset(i);
                let mut len = 0isize;
                while *p.offset(len) != 0 {
                    len += 1;
                }
                out.push(std::slice::from_raw_parts(p, len as usize).to_vec());
            }
            LocalFree(argv as *mut _);
            out
        }
    }

    #[test]
    fn join_wide_round_trips_through_os_parser() {
        // The OS parses argv[0] by special rules, so prefix a simple program token
        // and compare only argv[1..].
        let cases: Vec<Vec<&str>> = vec![
            vec!["plain", "args"],
            vec!["has space", "a\"b", "a\\b", "a\\\"b", "trail\\", "", "tab\tx"],
        ];
        for case in cases {
            let wides: Vec<Vec<u16>> = case.iter().map(|a| w(a)).collect();
            let refs: Vec<&[u16]> = wides.iter().map(|v| v.as_slice()).collect();
            let line = join_wide(&refs);
            let parsed = os_parse(&line);
            let expected: Vec<Vec<u16>> = wides.clone();
            assert_eq!(parsed, expected, "round-trip mismatch for {:?}", case);
        }
    }
}
```

Wire the test module into the `src/quote/windows.rs` placeholder so the new test file is actually compiled (without this, Step 2 reports a false "0 tests" green instead of a real red). Replace `src/quote/windows.rs` with:

```rust
//! Windows `CommandLineToArgvW`-compatible quoting. Filled in by the
//! "Windows command-line join" task.

#[cfg(test)]
#[path = "windows_tests.rs"]
mod windows_tests;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib quote::windows`
Expected: FAIL — compile error `cannot find function join_wide` (the `windows_tests` module is now wired in, so this is a real red, not "0 tests").

- [ ] **Step 3: Write minimal implementation**

Replace `src/quote/windows.rs` with:

```rust
//! Windows `CommandLineToArgvW`-compatible command-line construction.
//!
//! Operates on UTF-16 code units so the algorithm is testable on any host;
//! a `#[cfg(windows)]` test validates it against the real OS parser. Quoting
//! correctness here is a security boundary (BatBadBut / CVE-2024-24576).

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

fn needs_quotes(arg: &[u16]) -> bool {
    arg.is_empty() || arg.iter().any(|&c| c == SPACE || c == TAB)
}

/// Join argv into a single command-line string per the MSVCRT rules that
/// `CommandLineToArgvW` reverses. Intended for argv[1..]; the program name
/// (argv[0]) is passed separately as `lpApplicationName`.
pub fn join_wide(args: &[&[u16]]) -> Vec<u16> {
    let mut cmd: Vec<u16> = Vec::new();
    for (idx, arg) in args.iter().enumerate() {
        if idx > 0 {
            cmd.push(SPACE);
        }
        append_arg(&mut cmd, arg);
    }
    cmd
}

fn append_arg(cmd: &mut Vec<u16>, arg: &[u16]) {
    let quote = needs_quotes(arg);
    if quote {
        cmd.push(QUOTE);
    }
    let mut backslashes: usize = 0;
    for &x in arg {
        if x == BACKSLASH {
            backslashes += 1;
        } else {
            if x == QUOTE {
                // Already emitted `backslashes` backslashes; add `backslashes + 1`
                // more so a literal quote is preceded by 2n+1 backslashes.
                cmd.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        cmd.push(x);
    }
    if quote {
        // Double the trailing backslash run before the closing quote (2n).
        cmd.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        cmd.push(QUOTE);
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod windows_tests;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib quote::windows`
Expected: PASS. On Windows the `roundtrip` module also runs and passes; on non-Windows it is `cfg`-excluded (compile-time, not a runtime skip).

- [ ] **Step 5: Commit**

```bash
git add src/quote/windows.rs src/quote/windows_tests.rs
git commit -m "feat: add Windows CommandLineToArgvW-compatible join"
```

---

### Task 6: Windows first-token extraction

Derives the program token (for `lpApplicationName`) from a command-line string. **Deliberate deviation from `CommandLineToArgvW`'s argv[0] rule:** we skip leading whitespace and return `None` for empty/whitespace-only input, because we resolve a program from a *user-supplied command line* (spec §5.2), not the Windows loader's argv[0] self-substitution. Otherwise it follows the same shape: a leading `"` makes the token run to the next `"`; unquoted runs to the next whitespace; backslashes are literal in this position.

**Files:**
- Modify: `src/quote/windows.rs` (append `first_token_wide`)
- Modify: `src/quote/windows_tests.rs` (append tests)

**Interfaces:**
- Produces: `pub fn crate::quote::windows::first_token_wide(cmd: &[u16]) -> Option<Vec<u16>>`.

- [ ] **Step 1: Write the failing test**

Append to `src/quote/windows_tests.rs`:

```rust
use crate::quote::windows::first_token_wide;

fn first(s: &str) -> Option<String> {
    first_token_wide(&w(s)).map(|t| String::from_utf16(&t).unwrap())
}

#[test]
fn first_token_stops_at_whitespace() {
    assert_eq!(first("git status --short").as_deref(), Some("git"));
}

#[test]
fn first_token_skips_leading_whitespace_by_design() {
    // Deliberate deviation from CommandLineToArgvW argv[0] (which does NOT skip):
    // we resolve a program from a user command line, so leading blanks are ignored.
    assert_eq!(first("   \t cmd arg").as_deref(), Some("cmd"));
}

#[test]
fn quoted_first_token_spans_to_closing_quote() {
    assert_eq!(
        first("\"C:\\Program Files\\app.exe\" --flag").as_deref(),
        Some("C:\\Program Files\\app.exe")
    );
}

#[test]
fn unterminated_opening_quote_consumes_to_end() {
    assert_eq!(first("\"C:\\no close").as_deref(), Some("C:\\no close"));
}

#[test]
fn backslashes_are_literal_in_first_token() {
    assert_eq!(first("C:\\bin\\tool.exe x").as_deref(), Some("C:\\bin\\tool.exe"));
}

#[test]
fn empty_or_whitespace_only_has_no_first_token() {
    assert_eq!(first(""), None);
    assert_eq!(first("   \t "), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib quote::windows`
Expected: FAIL — compile error `cannot find function first_token_wide` (the `mod windows_tests;` wiring is already present from Task 5, so this is a real red).

- [ ] **Step 3: Write minimal implementation**

Append to `src/quote/windows.rs` (above the `#[cfg(test)]` block):

```rust
/// Extract the program token (argv[0]) from a command line, for deriving
/// `lpApplicationName` from a user-supplied `commandline`.
///
/// Deliberate deviation from `CommandLineToArgvW`'s argv[0] handling: leading
/// whitespace is skipped and empty/whitespace-only input returns `None` (the OS
/// loader instead starts at char 0 and self-substitutes the module path). The
/// token shape otherwise matches: a leading `"` runs to the next `"` (or to the
/// end if unterminated); an unquoted token runs to the next space/tab;
/// backslashes are literal here (no escaping).
pub fn first_token_wide(cmd: &[u16]) -> Option<Vec<u16>> {
    let mut i = 0usize;
    while i < cmd.len() && (cmd[i] == SPACE || cmd[i] == TAB) {
        i += 1;
    }
    if i >= cmd.len() {
        return None;
    }
    let mut out = Vec::new();
    if cmd[i] == QUOTE {
        i += 1;
        while i < cmd.len() && cmd[i] != QUOTE {
            out.push(cmd[i]);
            i += 1;
        }
    } else {
        while i < cmd.len() && cmd[i] != SPACE && cmd[i] != TAB {
            out.push(cmd[i]);
            i += 1;
        }
    }
    Some(out)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib quote::windows`
Expected: PASS (Task 5 + Task 6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/quote/windows.rs src/quote/windows_tests.rs
git commit -m "feat: add Windows first-token (lpApplicationName) extraction"
```

---

### Task 7: Command input model

The `Command` builder's *input* surface only: `new`, `arg`, `args`, `commandline`, `executable`, plus read-only accessors. `args`/`arg` and `commandline` are mutually exclusive sources (last source wins; switching discards the other). Resolution-to-native (wiring the quoters + PATH) is the next plan. The `pub(crate)` accessors and the `CommandLine` field are unused by the lib target in *this* plan (only by tests and the next plan), so they carry `#[allow(dead_code)]` with a rationale comment — keeping `cargo clippy` clean without hiding genuine dead code.

**Files:**
- Modify: `src/lib.rs` (add `mod command; pub use command::Command;`)
- Create: `src/command.rs`
- Create: `src/command_tests.rs`

**Interfaces:**
- Produces (all on `pub struct Command`):
  - `pub fn new() -> Command` (also `impl Default`).
  - `pub fn arg<S: Into<OsString>>(&mut self, a: S) -> &mut Command`
  - `pub fn args<I, S>(&mut self, args: I) -> &mut Command where I: IntoIterator<Item = S>, S: Into<OsString>`
  - `pub fn commandline<S: Into<OsString>>(&mut self, line: S) -> &mut Command`
  - `pub fn executable<P: Into<PathBuf>>(&mut self, path: P) -> &mut Command`
  - `pub(crate) enum CommandInput { Empty, Argv(Vec<OsString>), CommandLine(OsString) }` with `pub(crate) fn input(&self) -> &CommandInput` and `pub(crate) fn executable_path(&self) -> Option<&Path>` for the next plan.

- [ ] **Step 1: Write the failing test and wire the module skeleton**

Create `src/command_tests.rs`:

```rust
use crate::command::{Command, CommandInput};
use std::ffi::OsString;
use std::path::Path;

fn argv(cmd: &Command) -> Vec<String> {
    match cmd.input() {
        CommandInput::Argv(v) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
        other => panic!("expected Argv, got {:?}", other),
    }
}

#[test]
fn new_is_empty() {
    let cmd = Command::new();
    assert!(matches!(cmd.input(), CommandInput::Empty));
    assert!(cmd.executable_path().is_none());
}

#[test]
fn args_sets_and_extends_argv() {
    let mut cmd = Command::new();
    cmd.args(["git", "status"]).args(["--short"]);
    assert_eq!(argv(&cmd), ["git", "status", "--short"]);
}

#[test]
fn arg_appends_one() {
    let mut cmd = Command::new();
    cmd.arg("echo").arg("hi");
    assert_eq!(argv(&cmd), ["echo", "hi"]);
}

#[test]
fn commandline_sets_string_source() {
    let mut cmd = Command::new();
    cmd.commandline(r#"git "status""#);
    match cmd.input() {
        CommandInput::CommandLine(s) => assert_eq!(s, &OsString::from(r#"git "status""#)),
        other => panic!("expected CommandLine, got {:?}", other),
    }
}

#[test]
fn commandline_then_args_switches_source_and_discards() {
    let mut cmd = Command::new();
    cmd.commandline("ignored string").args(["real", "argv"]);
    assert_eq!(argv(&cmd), ["real", "argv"]);
}

#[test]
fn args_then_commandline_switches_to_string() {
    let mut cmd = Command::new();
    cmd.args(["a", "b"]).commandline("c d");
    assert!(matches!(cmd.input(), CommandInput::CommandLine(_)));
}

#[test]
fn executable_overrides_load_path_independently_of_argv() {
    let mut cmd = Command::new();
    cmd.executable("/bin/busybox").args(["sh", "-c", "echo hi"]);
    assert_eq!(cmd.executable_path(), Some(Path::new("/bin/busybox")));
    assert_eq!(argv(&cmd), ["sh", "-c", "echo hi"]);
}
```

Create `src/command.rs` as a skeleton that wires the test module but defines nothing yet:

```rust
//! The `Command` builder (input surface only in this plan).

#[cfg(test)]
#[path = "command_tests.rs"]
mod command_tests;
```

Add to `src/lib.rs` (after `pub mod quote;`):

```rust
mod command;
pub use command::Command;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib command`
Expected: FAIL — `cannot find type Command`/`CommandInput` (not defined yet).

- [ ] **Step 3: Write minimal implementation**

Replace `src/command.rs` with:

```rust
//! The `Command` builder. This plan implements only the input surface
//! (executable / args / commandline); stdio, env, containment, and spawning
//! are added by later plans.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A process to be configured and (later) spawned.
#[derive(Debug, Clone, Default)]
pub struct Command {
    // Read syntactically by input()/executable_path(), so the fields themselves
    // are not flagged; the accessors (consumed only by the next plan) carry the
    // allow below.
    input: CommandInput,
    executable: Option<PathBuf>,
}

/// The argument source of truth. `Argv` and `CommandLine` are mutually
/// exclusive — the last one set wins.
// The CommandLine field is read by tests and the resolution plan, not the lib target yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) enum CommandInput {
    #[default]
    Empty,
    Argv(Vec<OsString>),
    CommandLine(OsString),
}

impl Command {
    /// A fresh command with no arguments. argv is not special: set it via
    /// [`Command::args`]/[`Command::arg`] or [`Command::commandline`].
    pub fn new() -> Command {
        Command::default()
    }

    /// Append one argument, switching to argv mode if a command line was set.
    pub fn arg<S: Into<OsString>>(&mut self, a: S) -> &mut Command {
        match &mut self.input {
            CommandInput::Argv(v) => v.push(a.into()),
            _ => self.input = CommandInput::Argv(vec![a.into()]),
        }
        self
    }

    /// Append several arguments, switching to argv mode if a command line was set.
    pub fn args<I, S>(&mut self, args: I) -> &mut Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let items = args.into_iter().map(Into::into);
        match &mut self.input {
            CommandInput::Argv(v) => v.extend(items),
            _ => self.input = CommandInput::Argv(items.collect()),
        }
        self
    }

    /// Set the argument source to a single command-line string (Windows-native
    /// form). Discards any previously set argv.
    pub fn commandline<S: Into<OsString>>(&mut self, line: S) -> &mut Command {
        self.input = CommandInput::CommandLine(line.into());
        self
    }

    /// Override the executable file that the OS loads, independently of argv[0]
    /// (e.g. load `/bin/busybox` while argv[0] is `sh`).
    pub fn executable<P: Into<PathBuf>>(&mut self, path: P) -> &mut Command {
        self.executable = Some(path.into());
        self
    }

    // Consumed by the resolution plan; unused by the lib target in this plan.
    #[allow(dead_code)]
    pub(crate) fn input(&self) -> &CommandInput {
        &self.input
    }

    #[allow(dead_code)]
    pub(crate) fn executable_path(&self) -> Option<&Path> {
        self.executable.as_deref()
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod command_tests;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib command`
Expected: PASS (all `command_tests`, 7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/command.rs src/command_tests.rs
git commit -m "feat: add Command input model (executable/args/commandline)"
```

---

### Task 8: Public-surface smoke test + full verification

`src/lib.rs` is already fully wired (Tasks 2/3/7). This task adds a crate-level integration test that guards the public surface against regressions, then verifies the whole suite and clippy are clean.

**Files:**
- Create: `tests/foundation_smoke.rs`

**Interfaces:**
- Consumes: the public surface `subprocess::{error, quote, Command}`.
- Produces: nothing new (regression guard only).

- [ ] **Step 1: Write the smoke test (a public-surface regression guard)**

Create `tests/foundation_smoke.rs`:

```rust
use subprocess::error::QuoteErrorKind;
use subprocess::quote::posix;
use subprocess::quote::windows;
use subprocess::Command;

#[test]
fn public_surface_is_usable() {
    // POSIX round-trip via the public path.
    let args: Vec<&[u8]> = vec![b"echo", b"a b"];
    let line = posix::join(&args);
    assert_eq!(posix::split(&line).unwrap(), vec![b"echo".to_vec(), b"a b".to_vec()]);

    // POSIX error surfaced publicly.
    assert_eq!(posix::split(b"x\\").unwrap_err().kind, QuoteErrorKind::TrailingBackslash);

    // Windows join + first-token via the public path.
    let wides: Vec<Vec<u16>> = ["a b", "c"].iter().map(|s| s.encode_utf16().collect()).collect();
    let refs: Vec<&[u16]> = wides.iter().map(|v| v.as_slice()).collect();
    let joined = windows::join_wide(&refs);
    assert_eq!(String::from_utf16(&joined).unwrap(), "\"a b\" c");
    let ft = windows::first_token_wide(&"prog x".encode_utf16().collect::<Vec<_>>()).unwrap();
    assert_eq!(String::from_utf16(&ft).unwrap(), "prog");

    // Command builder is constructible from the crate root.
    let mut cmd = Command::new();
    cmd.args(["ls", "-la"]);
    let _ = cmd; // input model exercised in unit tests
}
```

- [ ] **Step 2: Run the smoke test**

Run: `cargo test --test foundation_smoke`
Expected: PASS — the public surface already exists (lib.rs wired by earlier tasks), so this guard passes immediately.

- [ ] **Step 3: Run the whole suite and clippy**

Run: `cargo test`
Expected: PASS — all unit tests (`error` 3, `quote::posix` 19, `quote::windows` 13 + 1 cfg(windows) roundtrip = 14 on Windows, `command` 7 → 43 lib tests on Windows) plus the `foundation_smoke` integration test. On Windows the `quote::windows::roundtrip` oracle also runs.

Run: `cargo clippy --all-targets`
Expected: `Finished` with **no warnings** (the `repeat_n` and `#[allow(dead_code)]` fixes keep it clean).

- [ ] **Step 4: Commit**

```bash
git add tests/foundation_smoke.rs
git commit -m "test: add foundation public-surface smoke test"
```

---

## Self-Review

**1. Spec coverage (foundation slice only):**
- Error taxonomy (`QuoteError{pos,kind}`, `Error`) — Task 2. ✔
- POSIX shlex split/quote/join, byte-oriented, with deviations + corpus — Tasks 3, 4. ✔
- Windows `CommandLineToArgvW`-compatible join + round-trip oracle — Task 5. ✔
- Windows first-token (`lpApplicationName`) extraction, documented as a deliberate deviation — Task 6. ✔
- Command input model (executable / args / commandline, mutual exclusion, last-wins) accepting `OsString` — Task 7. ✔
- `MSRV 1.87`, dual license, no-`mod.rs`, sibling `*_tests.rs`, incremental module wiring — Tasks 1–8. ✔
- Deferred to later plans (correctly out of scope here): resolution-to-native + PATH, `Fd`/`Stdio`, spawn, identity, containment, wait, tokio, `run`/`run_line` finalizers. Tracked by the plan sequence.

**2. Placeholder scan:** No TBD/TODO-in-code, no "add error handling", no vague steps — every code step has complete code. The `src/quote/windows.rs` placeholder in Task 3 is a real, compiling doc-only file that Task 5 replaces; called out explicitly. The skeleton-then-fill pattern (Steps 1→3 of Tasks 2/3/7) is intentional TDD bootstrap, each with a real red phase.

**3. Type consistency:** `QuoteError`/`QuoteErrorKind` (Task 2) used identically in Tasks 3–4 and the smoke test. `posix::{split,quote,join}` signatures (`&[u8]`/`Vec<u8>`/`&[&[u8]]`) match across tasks. `windows::{join_wide,first_token_wide}` (`&[&[u16]]`/`&[u16]`/`Vec<u16>`) match. `Command`/`CommandInput` accessors (`input`, `executable_path`) defined in Task 7 are the ones the next plan consumes.

**4. Review fixes applied (from plan-review):** incremental `lib.rs` wiring with real red/green per task (no stub-then-restore); `.gitignore` `Cargo.lock` comment on its own line + `git check-ignore` verification + `.tmp/` ignored wholesale; `first_token_wide` re-documented as a deliberate `CommandLineToArgvW` deviation + unterminated-quote test; `std::iter::repeat_n` instead of `repeat().take()`; `#[allow(dead_code)]` with rationale on next-plan-only items; dropped the unused `PartialEq` derive on `State`; added a `#`-literal test for the documented deviation.
