@echo off
rem Cargo runner for Windows release builds (ROADMAP F1): cargo tauri build --runner <abs path to this file>
rem builds with cargo-auditable, which embeds the dependency list in the binary (cargo auditable
rem must be installed: cargo install cargo-auditable --version "=0.7.6" --locked).
cargo auditable %*
exit /b %ERRORLEVEL%
