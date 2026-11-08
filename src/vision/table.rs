use anyhow::{bail, Result};
use image::{imageops, RgbImage};

use crate::profile::Profile;

use super::digits::{self, TemplateSet};

pub const GEM_TYPES: [&str; 7] = [
    "Diamant",
    "Émeraude",
    "Rubis",
    "Topaze",
    "Améthyste",
    "Saphir",
    "Crâne",
];

pub const TIERS: [&str; 5] = ["Abîmé", "Imparfait", "Normal", "Sans défaut", "Parfait"];

pub struct TableReading {
    /// `cells[colonne][rangée]`, colonne = type de gemme, rangée = palier.
    pub cells: [[Option<u16>; 5]; 7],
    pub scores: [[f32; 5]; 7],
    pub uncertain: Vec<(usize, usize)>,
    pub details: Vec<String>,
}

pub fn read_table(
    image: &RgbImage,
    profile: &Profile,
    templates: &TemplateSet,
    debug: bool,
) -> Result<TableReading> {
    let mut cells = [[None; 5]; 7];
    let mut scores = [[0.0f32; 5]; 7];
    let mut uncertain = Vec::new();
    let mut details = Vec::new();

    let roi_width = (profile.table.roi_right - profile.table.roi_left) as u32;
    let roi_height = (profile.table.roi_bottom - profile.table.roi_top) as u32;

    for row in 0..5 {
        for col in 0..7 {
            let right = profile.panel.x + profile.table.count_right_edges[col];
            let top = profile.panel.y + profile.table.count_tops[row];
            let roi_x = right + profile.table.roi_left;
            let roi_y = top + profile.table.roi_top;

            if roi_x < 0
                || roi_y < 0
                || roi_x as u32 + roi_width > image.width()
                || roi_y as u32 + roi_height > image.height()
            {
                bail!(
                    "la zone de lecture de la cellule ({col},{row}) sort de l'image: \
                     x={roi_x}, y={roi_y}, w={roi_width}, h={roi_height}"
                );
            }

            let roi = imageops::crop_imm(image, roi_x as u32, roi_y as u32, roi_width, roi_height)
                .to_image();

            let baseline = (top + 13 - roi_y).max(0) as usize;
            let reading = digits::read_count(
                &roi,
                baseline,
                profile.table.count_windows[col],
                templates,
                debug,
            );

            if reading.is_confident() {
                cells[col][row] = reading.number;
            } else {
                uncertain.push((col, row));
            }
            scores[col][row] = reading.score;

            if debug {
                details.push(format!(
                    "{} / {}: fenêtre={} best={:?} score={:.3} margin={:.3} \
                     bbox={:?} composantes={} pixels={} note={:?} top={:?}",
                    GEM_TYPES[col],
                    TIERS[row],
                    reading.window,
                    reading.best_candidate,
                    reading.score,
                    reading.margin,
                    reading.bbox,
                    reading.kept_components,
                    reading.actual_pixels,
                    reading.note,
                    reading.top_candidates,
                ));
            }
        }
    }

    Ok(TableReading {
        cells,
        scores,
        uncertain,
        details,
    })
}

impl TableReading {
    pub fn known_total(&self) -> u32 {
        self.cells
            .iter()
            .flat_map(|column| column.iter())
            .filter_map(|value| *value)
            .map(u32::from)
            .sum()
    }
}
