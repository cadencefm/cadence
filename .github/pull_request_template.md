## Summary

- Describe what changed and why.
- Link any related issue (`Closes #...`).

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo clippy --no-default-features --all-targets -- -D warnings`
- [ ] `cargo test -p cadence-engine --lib`
- [ ] `cargo test --workspace`
- [ ] `cargo test --release --test audio_invariants` *(if touching `core`, `engine`, or `examples`)*
- [ ] Visual PNG inspection of GPU renders *(if modifying GUI rendering or layouts)*
