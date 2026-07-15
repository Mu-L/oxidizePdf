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
