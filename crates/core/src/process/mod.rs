//! Spawning tools in a new session (Unix) or job object (Windows) with stdin null and stdout/stderr
//! to files; cancel with escalation; `wait` returning the exit info; per-job temp directories.
//!
//! Implemented by ROADMAP task B2, which adds `unix.rs` and `windows.rs`.
