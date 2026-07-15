# oxidize-pdf fuzzing harness

Coverage-guided fuzzing for the parser and text-extraction pipeline, using
[`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) / libFuzzer.

This crate is **excluded from the main workspace** (root `Cargo.toml`
`[workspace].exclude`), like `lints/`, because libFuzzer requires a nightly
toolchain. It never touches the MSRV / stable CI gates.

## Why

The parser has repeatedly shipped crash / silent-data-loss bugs on malformed
input that were each fixed one example at a time, with no guard against the next
one: #401 (negative `/Length` capacity-overflow panic), #82 (stack overflow on
circular refs), #260 (`/Length` mismatch), #415, #426 (recovery resolves a
stale `/Pages` root). Fuzzing finds this whole class automatically instead of
waiting for a user to report the next instance.

## Targets

| Target            | Drives                                              | Guards the class of |
|-------------------|-----------------------------------------------------|---------------------|
| `parse_document`  | parse (all 4 strictness modes) + page-tree walk     | #401, #82, #260, #415, #426 |
| `extract_text`    | lenient parse + text extraction, reorder off/on     | #389/#403/#408/#417/#422/#425 (crash-only) |

`extract_text` is a **crash-only** guard. The *logical* invariant for that
family — "reordering columns must not shred a token" — is a wrong-but-not-
crashing bug libFuzzer cannot see; that is enforced separately by the stable
proptest harness (token-preservation property).

## Run

```bash
cargo install cargo-fuzz          # once
cargo +nightly fuzz run parse_document          # runs until a crash or Ctrl-C
cargo +nightly fuzz run parse_document -- -max_total_time=300   # time-boxed
cargo +nightly fuzz run extract_text
```

### Seeding the corpus (optional but much faster)

Real PDFs as seeds get libFuzzer to interesting states far quicker than
starting from scratch. Seed from the committed test PDFs — **never** from
`.private/`:

```bash
cp test-pdfs/*.pdf fuzz/corpus/parse_document/
cp test-pdfs/*.pdf fuzz/corpus/extract_text/
```

Seed files committed to `corpus/<target>/` must be named `seed_*` (see
`.gitignore`). libFuzzer-discovered inputs on top are machine-generated and
git-ignored.

## When a crash is found

libFuzzer writes the input to `fuzz/artifacts/<target>/crash-<hash>`.

1. Minimize it: `cargo +nightly fuzz tmin parse_document fuzz/artifacts/parse_document/crash-<hash>`
2. Promote the minimized input into
   `oxidize-pdf-core/tests/fixtures/fuzz-regressions/` with a descriptive name.
3. It is now replayed on **stable** by `tests/fuzz_regressions_test.rs` in the
   normal `cargo test` gate — so the crash can never silently regress.
4. Fix the code until both the stable test and `cargo fuzz run` pass.

## CI

Run as a scheduled (cron) job, like the T2–T6 corpus tests — never on PRs (too
slow, non-deterministic wall-clock). The stable regression bridge
(`fuzz_regression_corpus_does_not_crash`) is what runs on every PR.
