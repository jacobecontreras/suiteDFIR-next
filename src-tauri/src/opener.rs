//! Opening files in their default app and revealing folders, for `open_report`, `open_text_file`,
//! `open_acq_file` and `reveal_path`. The app uses `tauri-plugin-opener`'s Rust free functions; the
//! plugin itself is never registered, so the webview gets no opener permission (ARCHITECTURE.md
//! D5). Tests use a recording opener instead.

use std::path::Path;

/// Opens and reveals paths the commands have already checked.
pub trait Opener: Send + Sync {
    /// Opens `path` in the OS default app (reports in the system browser, D17).
    fn open(&self, path: &Path) -> Result<(), String>;
    /// Shows `path` in the OS file manager.
    fn reveal(&self, path: &Path) -> Result<(), String>;
}

/// The real opener.
pub struct SystemOpener;

impl Opener for SystemOpener {
    fn open(&self, path: &Path) -> Result<(), String> {
        tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
    }

    fn reveal(&self, path: &Path) -> Result<(), String> {
        tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub mod testing {
    //! An opener that records what it was asked to do.

    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::Opener;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Opened {
        Open(PathBuf),
        Reveal(PathBuf),
    }

    #[derive(Default)]
    pub struct RecordingOpener {
        pub calls: Mutex<Vec<Opened>>,
    }

    impl RecordingOpener {
        pub fn calls(&self) -> Vec<Opened> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Opener for RecordingOpener {
        fn open(&self, path: &Path) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(Opened::Open(path.to_path_buf()));
            Ok(())
        }

        fn reveal(&self, path: &Path) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(Opened::Reveal(path.to_path_buf()));
            Ok(())
        }
    }
}
