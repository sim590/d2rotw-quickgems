use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::planner::Keep;

/// Limites pour un type de gemme. Les champs absents prennent la valeur du
/// défaut global.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierKeepConfig {
    pub abime: Option<u16>,
    pub imparfait: Option<u16>,
    pub normal: Option<u16>,
    pub sans_defaut: Option<u16>,
    pub parfait: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepTablesConfig {
    #[serde(default)]
    pub default: TierKeepConfig,
    #[serde(default)]
    pub diamant: Option<TierKeepConfig>,
    #[serde(default)]
    pub emeraude: Option<TierKeepConfig>,
    #[serde(default)]
    pub rubis: Option<TierKeepConfig>,
    #[serde(default)]
    pub topaze: Option<TierKeepConfig>,
    #[serde(default)]
    pub amethyste: Option<TierKeepConfig>,
    #[serde(default)]
    pub saphir: Option<TierKeepConfig>,
    #[serde(default)]
    pub crane: Option<TierKeepConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub keep: KeepTablesConfig,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("impossible de lire la configuration {}", path.display()))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("configuration TOML invalide: {}", path.display()))?;
        Ok(config)
    }

    /// Fusionne le défaut et les surcharges par type dans la matrice 7x5
    /// utilisée par le planificateur.
    pub fn keep_matrix(&self) -> Result<Keep> {
        let overrides = [
            self.keep.diamant.as_ref(),
            self.keep.emeraude.as_ref(),
            self.keep.rubis.as_ref(),
            self.keep.topaze.as_ref(),
            self.keep.amethyste.as_ref(),
            self.keep.saphir.as_ref(),
            self.keep.crane.as_ref(),
        ];

        let mut keep = [[0u32; 5]; 7];
        for (gem_type, override_config) in overrides.iter().enumerate() {
            keep[gem_type] = resolve_tier(&self.keep.default, *override_config)?;
        }

        Ok(keep)
    }
}

fn resolve_tier(
    default: &TierKeepConfig,
    override_config: Option<&TierKeepConfig>,
) -> Result<[u32; 5]> {
    let base = [
        default.abime.unwrap_or(0),
        default.imparfait.unwrap_or(0),
        default.normal.unwrap_or(0),
        default.sans_defaut.unwrap_or(0),
        default.parfait.unwrap_or(0),
    ];

    let resolved = if let Some(override_config) = override_config {
        [
            override_config.abime.unwrap_or(base[0]),
            override_config.imparfait.unwrap_or(base[1]),
            override_config.normal.unwrap_or(base[2]),
            override_config.sans_defaut.unwrap_or(base[3]),
            override_config.parfait.unwrap_or(base[4]),
        ]
    } else {
        base
    };

    for value in resolved {
        if value > 999 {
            bail!("une limite keep dépasse 999: {value}");
        }
    }

    Ok(resolved.map(u32::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_default_and_type_overrides() {
        let text = r#"
            [keep.default]
            abime = 10
            imparfait = 6
            normal = 4
            sans_defaut = 2
            parfait = 0

            [keep.diamant]
            abime = 20
            imparfait = 12
        "#;
        let config: Config = toml::from_str(text).expect("config TOML");
        let keep = config.keep_matrix().expect("keep matrix");

        assert_eq!(keep[0], [20, 12, 4, 2, 0]); // diamant
        assert_eq!(keep[1], [10, 6, 4, 2, 0]); // émeraude, valeurs par défaut
        assert_eq!(keep[6], [10, 6, 4, 2, 0]); // crâne, valeurs par défaut
    }

    #[test]
    fn rejects_old_english_type_keys() {
        let text = "[keep.diamond]\nabime = 10\n";
        assert!(toml::from_str::<Config>(text).is_err());

        let text = "[keep.diamant]\nchipped = 10\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }

    #[test]
    fn rejects_absurd_keep_values() {
        let text = r#"
            [keep.default]
            abime = 1000
        "#;
        let config: Config = toml::from_str(text).expect("config TOML");
        assert!(config.keep_matrix().is_err());
    }
}
