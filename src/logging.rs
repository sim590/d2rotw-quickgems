use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Journal simple : sortie console + fichier `logs/quickgems.log`.
pub struct Logger {
    file: Option<File>,
    path: PathBuf,
}

impl Logger {
    pub fn new() -> Self {
        let _ = fs::create_dir_all("logs");
        let path = PathBuf::from("logs/quickgems.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Self { file, path }
    }

    pub fn log(&mut self, message: impl AsRef<str>) {
        let line = format!("[{}] {}", epoch_seconds(), message.as_ref());
        println!("{line}");
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
