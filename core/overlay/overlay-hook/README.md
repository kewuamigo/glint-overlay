# overlay-hook

MinHook-based function hooking used internally by `overlay-core`.

## Pregenerated stubs

Large architecture-specific binding files live in `src/`:

- `pregenerated-x64.rs`
- `pregenerated-x86.rs`

They are checked in so MSVC builds do not require running a codegen step on every machine. Regenerate only when updating MinHook exports or target ABIs (see crate `build.rs` if present).

## Usage

Do not depend on this crate directly — use `glint-overlay-core`.
