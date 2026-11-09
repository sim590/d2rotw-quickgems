//! Localisation des fichiers de configuration utilisateur.
//!
//! Le tracé des chemins est une préoccupation de plateforme : XDG sur Linux,
//! `%APPDATA%` sur Windows, `Application Support` sur macOS. Ce module ne
//! charge rien : il donne les chemins à essayer, du plus prioritaire au moins
//! prioritaire, et l'appelant décide quoi faire.

use std::path::{Path, PathBuf};

/// Nom du répertoire de l'application sous le répertoire de configuration.
pub const APP_DIR: &str = "d2rotw-quickgems";
/// Nom du fichier de configuration.
pub const CONFIG_FILE: &str = "quickgems.toml";

/// Localise les fichiers de configuration utilisateur.
pub trait ConfigLocator {
    /// Fichiers à essayer, du plus prioritaire au moins prioritaire.
    /// Les chemins absents sont ignorés par l'appelant.
    fn config_paths(&self) -> Vec<PathBuf>;
}

/// Chemins natifs de la plateforme cible.
#[cfg(target_os = "linux")]
pub struct NativePaths;

#[cfg(target_os = "linux")]
impl ConfigLocator for NativePaths {
    fn config_paths(&self) -> Vec<PathBuf> {
        dirs::config_dir()
            .map(|base| vec![app_config_path(&base)])
            .unwrap_or_default()
    }
}

/// Compose le chemin du fichier de configuration à partir du répertoire de
/// base fourni par la plateforme.
fn app_config_path(base: &Path) -> PathBuf {
    base.join(APP_DIR).join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn composes_the_app_config_path() {
        let path = super::app_config_path(Path::new("config"));
        assert_eq!(path, Path::new("config/d2rotw-quickgems/quickgems.toml"));
    }
}
