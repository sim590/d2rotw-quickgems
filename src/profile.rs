use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TableProfile {
    pub count_right_edges: Vec<i32>,
    pub count_tops: Vec<i32>,
    pub roi_left: i32,
    pub roi_right: i32,
    pub roi_top: i32,
    pub roi_bottom: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CubeProfile {
    pub slots: Vec<Rect>,
}

/// Coordonnées de clic, relatives au panneau.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ClickProfile {
    /// Décalage du centre de la cellule source par rapport au bord droit et au
    /// haut du nombre.
    pub source_dx: i32,
    pub source_dy: i32,
    /// Centre du bouton « Transmuter ».
    pub transmute_x: i32,
    pub transmute_y: i32,
    /// Point neutre où stationner le curseur pour éviter les infobulles.
    pub park_x: i32,
    pub park_y: i32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TabsProfile {
    pub gemmes: Rect,
    pub partage: Rect,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub image_width: u32,
    pub image_height: u32,
    pub panel: Rect,
    pub table: TableProfile,
    pub cube: CubeProfile,
    pub click: ClickProfile,
    pub tabs: TabsProfile,
}

impl Profile {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("impossible de lire le profil {}", path.display()))?;
        Self::from_toml(&text, &path.display().to_string())
    }

    pub fn from_toml(text: &str, origin: &str) -> Result<Self> {
        let profile: Profile =
            toml::from_str(text).with_context(|| format!("profil TOML invalide: {origin}"))?;
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<()> {
        if self.image_width == 0 || self.image_height == 0 {
            bail!("les dimensions de l'image du profil doivent être > 0");
        }
        if self.panel.width == 0 || self.panel.height == 0 {
            bail!("le panneau du profil doit avoir une taille > 0");
        }
        if self.table.count_right_edges.len() != 7 {
            bail!(
                "le profil doit contenir 7 bords droits de colonnes, pas {}",
                self.table.count_right_edges.len()
            );
        }
        if self.table.count_tops.len() != 5 {
            bail!(
                "le profil doit contenir 5 hauts de rangées, pas {}",
                self.table.count_tops.len()
            );
        }
        if self.table.roi_right <= self.table.roi_left {
            bail!("la zone de lecture du nombre est vide");
        }
        if self.table.roi_bottom <= self.table.roi_top {
            bail!("la zone de lecture du nombre est vide");
        }
        if self.cube.slots.len() != 12 {
            bail!(
                "le profil doit contenir 12 cases de cube, pas {}",
                self.cube.slots.len()
            );
        }
        if self.tabs.gemmes.width == 0
            || self.tabs.gemmes.height == 0
            || self.tabs.partage.width == 0
            || self.tabs.partage.height == 0
        {
            bail!("les rectangles des onglets doivent avoir une taille > 0");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_2560_profile_is_valid() {
        let text = include_str!("../profiles/2560x1440.toml");
        let profile: Profile = toml::from_str(text).expect("profile TOML");
        profile.validate().expect("valid profile");
        assert_eq!(profile.table.count_right_edges.len(), 7);
        assert_eq!(profile.table.count_tops.len(), 5);
        assert_eq!(profile.cube.slots.len(), 12);
    }
}
