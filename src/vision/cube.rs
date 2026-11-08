use anyhow::{bail, Result};
use image::{imageops, GenericImageView, RgbImage};

use crate::profile::Profile;

/// Marge intérieure pour éviter le cadre doré des cases du cube.
const SLOT_INSET: i32 = 8;
/// Seuil de luminance maximale pour considérer une case comme vide.
const EMPTY_MAX_LUMA: f32 = 35.0;
/// Seuil d'écart-type pour considérer une case comme vide.
const EMPTY_MAX_STD: f32 = 8.0;

pub struct CubeReading {
    /// Ordre row-major, 3 colonnes x 4 rangées.
    pub occupied: [bool; 12],
    pub max_luma: [f32; 12],
    pub std_dev: [f32; 12],
}

pub fn read_cube(image: &RgbImage, profile: &Profile) -> Result<CubeReading> {
    let mut occupied = [false; 12];
    let mut max_luma = [0.0f32; 12];
    let mut std_dev = [0.0f32; 12];

    for (index, slot) in profile.cube.slots.iter().enumerate() {
        let rect = slot.translated(profile.panel.x, profile.panel.y);
        let x = rect.x + SLOT_INSET;
        let y = rect.y + SLOT_INSET;
        let width = rect.width.saturating_sub((SLOT_INSET * 2) as u32);
        let height = rect.height.saturating_sub((SLOT_INSET * 2) as u32);

        if width == 0
            || height == 0
            || x < 0
            || y < 0
            || x as u32 + width > image.width()
            || y as u32 + height > image.height()
        {
            bail!("la case de cube {index} sort de l'image");
        }

        let view = imageops::crop_imm(image, x as u32, y as u32, width, height);
        let mut maximum = 0.0f32;
        let mut sum = 0.0f32;
        let mut sum_squared = 0.0f32;
        let mut count = 0.0f32;

        for (_, _, pixel) in view.pixels() {
            let red = f32::from(pixel[0]);
            let green = f32::from(pixel[1]);
            let blue = f32::from(pixel[2]);
            let luma = 0.299 * red + 0.587 * green + 0.114 * blue;
            maximum = maximum.max(luma);
            sum += luma;
            sum_squared += luma * luma;
            count += 1.0;
        }

        if count == 0.0 {
            bail!("la case de cube {index} est vide dans l'image");
        }

        let mean = sum / count;
        let variance = (sum_squared / count - mean * mean).max(0.0);
        let deviation = variance.sqrt();

        max_luma[index] = maximum;
        std_dev[index] = deviation;
        occupied[index] = maximum > EMPTY_MAX_LUMA || deviation > EMPTY_MAX_STD;
    }

    Ok(CubeReading {
        occupied,
        max_luma,
        std_dev,
    })
}
