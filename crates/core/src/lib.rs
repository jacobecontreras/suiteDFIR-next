//! suiteDFIR core: all application logic, with no Tauri dependency.
//!
//! The module layout is described in docs/ARCHITECTURE.md §5.1.

/// TEMPORARY (M0.2 acceptance demo, reverted in the next commit): `needless_return` makes clippy fail.
pub fn clippy_demo(x: u32) -> u32 {
    return x + 1;
}
