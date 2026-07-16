# Fuzz regression corpus

Minimized crash inputs found by the fuzzing harness (`../../../../fuzz/`).

Every file here is replayed on **stable** by `tests/fuzz_regressions_test.rs`
through the same parse + navigate + extract path the fuzz targets drive. This
turns a crash that libFuzzer can only find under nightly into a permanent guard
that runs in the normal `cargo test` gate.

## Promoting a crash

When `cargo +nightly fuzz run <target>` reports a crash:

```bash
# 1. Minimize the artifact to the smallest input that still crashes.
cargo +nightly fuzz tmin parse_document fuzz/artifacts/parse_document/crash-<hash>

# 2. Copy the minimized input here with a descriptive name.
cp fuzz/artifacts/parse_document/minimized-from-<hash> \
   oxidize-pdf-core/tests/fixtures/fuzz-regressions/issue_401_negative_length.pdf

# 3. Confirm it reproduces on stable, then fix the parser until it passes.
cargo test -p oxidize-pdf fuzz_regression_corpus_does_not_crash
```

Files here need **not** be valid PDFs — they are whatever bytes triggered the
crash. Keep them small (minimized) so the stable gate stays fast.

The first byte of each fixture is the fuzz target's mode selector (`% 4` picks
strict / tolerant / lenient / skip-errors); the rest is the input document.

## Promoting a proptest failure

`tests/prop_parser_panic_invariants.rs` hunts the same class on stable, and its
shrunk counterexamples belong here too — `.proptest-regressions` files are
gitignored on purpose (the durable pin is the fixture, not the RNG seed, which
does not survive a proptest upgrade). Rebuild the shrunk input, prepend a
selector byte, and drop it in.
