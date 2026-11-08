//! Vérification de l'onglet actif du cache.
//!
//! Les onglets inactifs sont sombres; l'onglet actif a un cadre et un texte
//! plus lumineux. On compare la fraction de pixels clairs des onglets
//! « Gemmes » et « Partagé ». Ça évite de lire la grille d'objets de
//! « Partagé » comme si c'était la table des gemmes.

use anyhow::{bail, Result};
use image::{imageops, GenericImageView, RgbImage};

use crate::profile::{Profile, Rect};

const BRIGHT_THRESHOLD: f32 = 160.0;
const MIN_ACTIVE_FRACTION: f32 = 0.005;
const MIN_DIFFERENCE: f32 = 0.002;

pub fn gemmes_active(image: &RgbImage, profile: &Profile) -> Result<bool> {
    let gemmes = bright_fraction(
        image,
        profile
            .tabs
            .gemmes
            .translated(profile.panel.x, profile.panel.y),
    )?;
    let partage = bright_fraction(
        image,
        profile
            .tabs
            .partage
            .translated(profile.panel.x, profile.panel.y),
    )?;

    Ok(gemmes > MIN_ACTIVE_FRACTION && gemmes > partage + MIN_DIFFERENCE)
}

fn bright_fraction(image: &RgbImage, rect: Rect) -> Result<f32> {
    if rect.x < 0 || rect.y < 0 || rect.width == 0 || rect.height == 0 {
        bail!("rectangle d'onglet invalide: {rect:?}");
    }
    let x = rect.x as u32;
    let y = rect.y as u32;
    if x + rect.width > image.width() || y + rect.height > image.height() {
        bail!("rectangle d'onglet hors de l'image");
    }

    let view = imageops::crop_imm(image, x, y, rect.width, rect.height);
    let mut bright = 0u32;
    let mut total = 0u32;
    for (_, _, pixel) in view.pixels() {
        let luma =
            0.299 * f32::from(pixel[0]) + 0.587 * f32::from(pixel[1]) + 0.114 * f32::from(pixel[2]);
        if luma > BRIGHT_THRESHOLD {
            bright += 1;
        }
        total += 1;
    }

    Ok(if total == 0 {
        0.0
    } else {
        bright as f32 / total as f32
    })
}
