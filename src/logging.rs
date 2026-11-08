use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Répertoire des artefacts du run : journal et captures d'erreur.
///
/// Il est créé sous le répertoire temporaire du système, dans un dossier
/// unique `d2rotw-quickgems-<suffixe>` avec des permissions privées. Le
/// programme n'écrit donc rien dans le répertoire courant.
pub fn run_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("d2rotw-quickgems-{}", random_suffix()));
        let _ = fs::create_dir_all(&dir);
        set_private_permissions(&dir);
        dir
    })
}

/// Suffixe aléatoire court, tiré de `/dev/urandom` quand c'est possible.
fn random_suffix() -> String {
    use std::io::Read;

    let mut bytes = [0u8; 6];
    if let Ok(mut source) = File::open("/dev/urandom") {
        if source.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        }
    }

    // Repli sans source aléatoire : PID et seconde courante.
    format!("{:x}-{:x}", std::process::id(), epoch_seconds())
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

/// Journal simple : sortie console + fichier `quickgems.log` dans le
/// répertoire du run.
pub struct Logger {
    file: Option<File>,
    path: PathBuf,
}

impl Logger {
    pub fn new() -> Self {
        let path = run_dir().join("quickgems.log");
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

#[cfg(test)]
mod tests {
    use super::run_dir;

    #[test]
    fn run_dir_is_private_and_named() {
        let dir = run_dir();
        assert!(dir.starts_with(std::env::temp_dir()));
        assert!(
            dir.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("d2rotw-quickgems-"))
                .unwrap_or(false),
            "répertoire inattendu: {}",
            dir.display()
        );
        assert!(dir.is_dir());
        assert_eq!(run_dir(), dir, "le répertoire du run doit être stable");
    }
}
