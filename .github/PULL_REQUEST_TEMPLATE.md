## Summary
<!-- What this changes and why. Link the issue, roadmap step or ADR. -->

## Changes
<!-- The main changes, file by file if that helps. -->

## Testing
<!-- How you tested it. For posting, holds, reversals or verification: which
     database tests cover it. -->

## Checklist
- [ ] The title is a Conventional Commit starting with a lowercase letter (`feat: add holds`)
- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass
- [ ] `cargo test --workspace` passes with `MEZGEB_TEST_DATABASE_URL` set
- [ ] Docs are updated if behaviour changed
- [ ] No invariant changed, or an ADR records the change
- [ ] No merged migration was edited
