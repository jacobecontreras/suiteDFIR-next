//! suiteDFIR core: all application logic, with no Tauri dependency.
//!
//! The module layout is described in docs/ARCHITECTURE.md §5.1.

pub mod acquire;
pub mod case;
pub mod contracts;
pub mod fsutil;
pub mod hashing;
pub mod idevice;
pub mod inspect;
pub mod leapp;
pub mod manifest;
pub mod paths;
pub mod process;
pub mod run;
pub mod runner;
pub mod settings;
pub mod tail;
