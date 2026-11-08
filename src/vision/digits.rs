//! Lecteur de quantités par gabarits de chiffres.
//!
//! Les quantités de gemmes sont affichées dans une police fixe (Exocet) à des
//! coordonnées fixes. On rend les nombres 0..=99 avec la police embarquée, puis
//! on compare chaque gabarit à la zone du nombre avec un Dice à deux côtés.
//! Aucun OCR générique n'est nécessaire.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use fontdue::Font;
use image::{GrayImage, Luma, RgbImage};

use crate::profile::Rect;

const FONT_BYTES: &[u8] = include_bytes!("../../assets/exocet-blizzard-medium.ttf");

/// Seuil de blancheur pour le texte (le contour noir reste sous ce seuil).
const WHITE_THRESHOLD: f32 = 180.0;
/// Saturation maximale d'un pixel considéré comme du texte blanc.
const SATURATION_THRESHOLD: f32 = 0.35;
/// Un pixel très lumineux est accepté même s'il est légèrement coloré.
const HIGH_LUMA_THRESHOLD: f32 = 220.0;
/// Tolérance verticale autour de la ligne de base des chiffres.
const BASELINE_TOLERANCE: i32 = 3;
/// Nombre maximal rendu. Les quantités de la capture sont <= 33.
const MAX_NUMBER: u16 = 99;
/// Tailles de police essayées. Le texte réel fait ~13 px de haut.
const TEMPLATE_SIZES: [f32; 7] = [16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0];
/// Marge de recherche autour de l'alignement bord droit / ligne de base.
const ALIGN_X: i32 = 2;
const ALIGN_Y: i32 = 1;
/// Fenêtre large essayée pour chaque cellule. Elle couvre les nombres à deux
/// chiffres comme « 20 », même si la fenêtre étroite du profil les coupe.
const WIDE_WINDOW: usize = 30;

#[derive(Debug, Clone)]
pub struct Template {
    pub number: u16,
    pub width: u32,
    pub height: u32,
    pub mask: Vec<bool>,
}

pub struct TemplateSet {
    pub templates: Vec<Template>,
}

impl TemplateSet {
    pub fn load() -> Result<Self> {
        let font = Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .map_err(|error| anyhow!("police Exocet invalide: {error}"))?;

        let mut templates = Vec::new();
        for number in 0..=MAX_NUMBER {
            for size in TEMPLATE_SIZES {
                let image = render_text(&font, &number.to_string(), size);
                if let Some((width, height, mask)) = tight_mask(&image) {
                    templates.push(Template {
                        number,
                        width,
                        height,
                        mask,
                    });
                }
            }
        }

        if templates.is_empty() {
            return Err(anyhow!("aucun gabarit de chiffre n'a pu être rendu"));
        }

        Ok(Self { templates })
    }
}

/// Résultat de lecture d'une cellule.
#[derive(Debug, Clone)]
pub struct CountReading {
    /// Quantité retenue si la lecture est confiante.
    pub number: Option<u16>,
    /// Meilleur candidat, même si la lecture n'est pas confiante.
    pub best_candidate: Option<u16>,
    pub score: f32,
    pub margin: f32,
    pub bbox: Option<Rect>,
    pub kept_components: usize,
    pub actual_pixels: usize,
    pub top_candidates: Vec<(u16, f32)>,
    pub window: usize,
    pub note: Option<String>,
}

impl CountReading {
    pub fn is_confident(&self) -> bool {
        self.number.is_some() && is_confident_score(self.score, self.margin)
    }

    fn empty(note: impl Into<String>) -> Self {
        Self {
            number: None,
            best_candidate: None,
            score: 0.0,
            margin: 0.0,
            bbox: None,
            kept_components: 0,
            actual_pixels: 0,
            top_candidates: Vec::new(),
            window: 0,
            note: Some(note.into()),
        }
    }
}

/// Lit le nombre dans une petite zone (ROI) qui contient le texte blanc et,
/// possiblement, le bas d'une icône de gemme.
///
/// Essaie quelques largeurs de fenêtre autour de celle du profil. Une fenêtre
/// plus étroite enlève mieux le bas d'une icône (crâne, diamant), mais peut
/// couper le premier chiffre d'un nombre large; on garde donc la meilleure
/// lecture, avec un filtre sur la largeur du gabarit.
pub fn read_count(
    roi: &RgbImage,
    baseline: usize,
    base_window: usize,
    templates: &TemplateSet,
    debug: bool,
) -> CountReading {
    let mut windows = Vec::new();
    for delta in [-2i32, 0, 2] {
        let window = (base_window as i32 + delta).max(8) as usize;
        if !windows.contains(&window) {
            windows.push(window);
        }
    }
    if !windows.contains(&WIDE_WINDOW) {
        windows.push(WIDE_WINDOW);
    }

    let mut base_best: Option<CountReading> = None;
    let mut wide_best: Option<CountReading> = None;
    let mut zero: Option<CountReading> = None;

    for window in windows {
        let reading = read_count_window(roi, baseline, window, templates, debug);
        if reading.actual_pixels == 0 {
            if zero.is_none() {
                zero = Some(reading);
            }
            continue;
        }

        if window == WIDE_WINDOW {
            if wide_best
                .as_ref()
                .map_or(true, |current| reading.score > current.score)
            {
                wide_best = Some(reading);
            }
        } else if base_best
            .as_ref()
            .map_or(true, |current| reading.score > current.score)
        {
            base_best = Some(reading);
        }
    }

    if let Some(base) = base_best {
        // La fenêtre étroite est plus fiable près des icônes blanches, mais
        // elle peut ne garder que le « 0 » d'un « 20 ». Si le candidat étroit
        // est exactement zéro et que la fenêtre large trouve un nombre à deux
        // chiffres avec un score élevé, on prend la fenêtre large.
        let narrow_only_zero = base.best_candidate == Some(0);
        if narrow_only_zero {
            if let Some(wide) = &wide_best {
                if let Some(candidate) = wide.best_candidate {
                    if candidate >= 10 && wide.score >= 0.75 {
                        return wide.clone();
                    }
                }
            }
        }
        return base;
    }

    if let Some(wide) = wide_best {
        return wide;
    }

    if let Some(mut zero) = zero {
        // Une cellule sans chiffre est une cellule à zéro. Le garde-fou de
        // `read_table` empêche de confondre une table entièrement vide avec un
        // cache fermé.
        zero.number = Some(0);
        zero.best_candidate = Some(0);
        zero.score = 1.0;
        zero.margin = 1.0;
        zero.top_candidates = vec![(0, 1.0)];
        zero.note = Some("aucun chiffre: quantité zéro".to_string());
        return zero;
    }

    CountReading::empty("aucune fenêtre de lecture")
}

fn read_count_window(
    roi: &RgbImage,
    baseline: usize,
    window: usize,
    templates: &TemplateSet,
    debug: bool,
) -> CountReading {
    let (actual, bbox, kept_components, actual_pixels) = build_actual_mask(roi, baseline, window);
    let Some(bbox) = bbox else {
        return CountReading {
            number: Some(0),
            best_candidate: Some(0),
            score: 0.0,
            margin: 0.0,
            bbox: None,
            kept_components,
            actual_pixels,
            top_candidates: Vec::new(),
            window,
            note: Some("aucun chiffre".to_string()),
        };
    };

    let width = roi.width() as usize;
    let height = roi.height() as usize;
    let actual_width = bbox.width as i32;

    let mut best_by_number: HashMap<u16, (f32, u32)> = HashMap::new();
    for template in &templates.templates {
        let mut best_for_number = 0.0f32;
        for dx in -ALIGN_X..=ALIGN_X {
            for dy in -ALIGN_Y..=ALIGN_Y {
                let score = score_template(&actual, width, height, bbox, template, dx, dy);
                if score > best_for_number {
                    best_for_number = score;
                }
            }
        }
        best_by_number
            .entry(template.number)
            .and_modify(|(score, template_width)| {
                if best_for_number > *score {
                    *score = best_for_number;
                    *template_width = template.width;
                }
            })
            .or_insert((best_for_number, template.width));
    }

    let mut ranked_all: Vec<(u16, f32, u32)> = best_by_number
        .into_iter()
        .map(|(number, (score, width))| (number, score, width))
        .collect();

    // Un candidat dont le gabarit est beaucoup plus étroit que le texte lu est
    // presque certainement un nombre tronqué (ex. « 6 » au lieu de « 26 »).
    // La largeur réelle inclut parfois un morceau d'icône; on garde une
    // tolérance de 6 px.
    let mut ranked: Vec<(u16, f32, u32)> = ranked_all
        .iter()
        .copied()
        .filter(|(_, _, template_width)| {
            let difference = *template_width as i32 - actual_width;
            (-6..=6).contains(&difference)
        })
        .collect();
    if ranked.is_empty() {
        ranked = ranked_all.clone();
    }

    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked_all.clear();

    let (best_number, best_score, best_template_width) = ranked[0];
    let second_score = ranked.get(1).map_or(0.0, |(_, score, _)| *score);
    let margin = best_score - second_score;
    let confident = is_confident_score(best_score, margin);

    let top_candidates = ranked
        .iter()
        .take(5)
        .map(|(number, score, _)| (*number, *score))
        .collect::<Vec<_>>();

    if debug {
        eprintln!(
            "  fenêtre={} bbox={:?} largeur={} gabarit={} kept={} pixels={} \
             best={} ({:.3}) margin={:.3} top={:?}",
            window,
            bbox,
            actual_width,
            best_template_width,
            kept_components,
            actual_pixels,
            best_number,
            best_score,
            margin,
            top_candidates
        );
    }

    CountReading {
        number: confident.then_some(best_number),
        best_candidate: Some(best_number),
        score: best_score,
        margin,
        bbox: Some(bbox),
        kept_components,
        actual_pixels,
        top_candidates,
        window,
        note: (!confident).then(|| "confiance insuffisante".to_string()),
    }
}

/// Règle de confiance commune au calcul et à l'accesseur.
fn is_confident_score(score: f32, margin: f32) -> bool {
    // Un score élevé avec une petite marge reste fiable; un score plus bas
    // exige une marge plus grande pour éviter de deviner. Le seuil de 0.01
    // couvre les nombres proches (ex. 36 vs 35) sur les icônes colorées.
    (score >= 0.75 && margin >= 0.01) || (score >= 0.40 && margin >= 0.04)
}

/// Construit le masque des chiffres en ignorant les pixels colorés de l'icône
/// et les composantes qui ne reposent pas sur la ligne de base.
fn build_actual_mask(
    roi: &RgbImage,
    baseline: usize,
    window: usize,
) -> (Vec<bool>, Option<Rect>, usize, usize) {
    let width = roi.width() as usize;
    let height = roi.height() as usize;
    let window_left = width.saturating_sub(window);
    let mut white = vec![false; width * height];

    for y in 0..height {
        for x in 0..width {
            if x < window_left {
                continue;
            }
            let pixel = roi.get_pixel(x as u32, y as u32).0;
            let red = f32::from(pixel[0]);
            let green = f32::from(pixel[1]);
            let blue = f32::from(pixel[2]);
            let max = red.max(green).max(blue);
            let min = red.min(green).min(blue);
            let luma = 0.299 * red + 0.587 * green + 0.114 * blue;
            let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };

            white[y * width + x] = luma > WHITE_THRESHOLD
                && (saturation < SATURATION_THRESHOLD || luma > HIGH_LUMA_THRESHOLD);
        }
    }

    let mut visited = vec![false; width * height];
    let mut actual = vec![false; width * height];
    let mut kept_components = 0usize;

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !white[index] || visited[index] {
                continue;
            }

            let mut stack = vec![(x, y)];
            let mut pixels = Vec::new();
            visited[index] = true;
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;

            while let Some((current_x, current_y)) = stack.pop() {
                pixels.push((current_x, current_y));
                min_x = min_x.min(current_x);
                max_x = max_x.max(current_x);
                min_y = min_y.min(current_y);
                max_y = max_y.max(current_y);

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let next_x = current_x as i32 + dx;
                        let next_y = current_y as i32 + dy;
                        if next_x < 0
                            || next_y < 0
                            || next_x >= width as i32
                            || next_y >= height as i32
                        {
                            continue;
                        }
                        let next_index = next_y as usize * width + next_x as usize;
                        if white[next_index] && !visited[next_index] {
                            visited[next_index] = true;
                            stack.push((next_x as usize, next_y as usize));
                        }
                    }
                }
            }

            let bottom = max_y + 1;
            let component_height = max_y - min_y + 1;
            let component_width = max_x - min_x + 1;
            let on_baseline = bottom as i32 + BASELINE_TOLERANCE >= baseline as i32
                && bottom as i32 <= baseline as i32 + BASELINE_TOLERANCE;

            if on_baseline && pixels.len() >= 8 && component_height >= 6 && component_width <= 34 {
                kept_components += 1;
                for (pixel_x, pixel_y) in pixels {
                    actual[pixel_y * width + pixel_x] = true;
                }
            }
        }
    }

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut any = false;
    for y in 0..height {
        for x in 0..width {
            if actual[y * width + x] {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    let bbox = any.then_some(Rect {
        x: min_x as i32,
        y: min_y as i32,
        width: (max_x - min_x + 1) as u32,
        height: (max_y - min_y + 1) as u32,
    });
    let actual_pixels = actual.iter().filter(|pixel| **pixel).count();

    (actual, bbox, kept_components, actual_pixels)
}

/// Dice à deux côtés : le gabarit est aligné sur le bord droit et la ligne de
/// base de la zone lue. Les pixels du gabarit hors de la ROI sont comptés dans
/// le dénominateur, ce qui évite de favoriser un nombre trop court.
fn score_template(
    actual: &[bool],
    width: usize,
    height: usize,
    bbox: Rect,
    template: &Template,
    dx: i32,
    dy: i32,
) -> f32 {
    let actual_on = actual.iter().filter(|pixel| **pixel).count() as f32;
    if actual_on == 0.0 {
        return 0.0;
    }

    let template_on = template.mask.iter().filter(|pixel| **pixel).count() as f32;
    if template_on == 0.0 {
        return 0.0;
    }

    let start_x = bbox.right() - template.width as i32 + dx;
    let start_y = bbox.bottom() - template.height as i32 + dy;
    let mut intersection = 0usize;

    for template_y in 0..template.height as i32 {
        for template_x in 0..template.width as i32 {
            let mask_index = template_y as usize * template.width as usize + template_x as usize;
            if !template.mask[mask_index] {
                continue;
            }
            let x = start_x + template_x;
            let y = start_y + template_y;
            if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                continue;
            }
            if actual[y as usize * width + x as usize] {
                intersection += 1;
            }
        }
    }

    2.0 * intersection as f32 / (actual_on + template_on)
}

/// Rend `text` en blanc sur noir, avec un petit rembourrage.
fn render_text(font: &Font, text: &str, size_px: f32) -> GrayImage {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font], &TextStyle::new(text, size_px, 0));

    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return GrayImage::from_pixel(1, 1, Luma([0]));
    }

    const PADDING: u32 = 1;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for glyph in glyphs {
        min_x = min_x.min(glyph.x);
        min_y = min_y.min(glyph.y);
        max_x = max_x.max(glyph.x + glyph.width as f32);
        max_y = max_y.max(glyph.y + glyph.height as f32);
    }

    let width = (max_x - min_x).ceil().max(1.0) as u32 + PADDING * 2;
    let height = (max_y - min_y).ceil().max(1.0) as u32 + PADDING * 2;
    let mut image = GrayImage::from_pixel(width, height, Luma([0]));

    for glyph in glyphs {
        let (metrics, bitmap) = font.rasterize_config(glyph.key);
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let origin_x = (glyph.x - min_x).round() as i32 + PADDING as i32;
        let origin_y = (glyph.y - min_y).round() as i32 + PADDING as i32;

        for glyph_y in 0..metrics.height {
            let y = origin_y + glyph_y as i32;
            if y < 0 || y >= height as i32 {
                continue;
            }
            for glyph_x in 0..metrics.width {
                let x = origin_x + glyph_x as i32;
                if x < 0 || x >= width as i32 {
                    continue;
                }
                let coverage = bitmap[glyph_y * metrics.width + glyph_x];
                if coverage == 0 {
                    continue;
                }
                let pixel = image.get_pixel_mut(x as u32, y as u32);
                pixel.0[0] = pixel.0[0].max(coverage);
            }
        }
    }

    image
}

/// Réduit une image rendue à son masque binaire serré.
#[cfg(test)]
mod tests {
    use super::{is_confident_score, read_count, TemplateSet};

    #[test]
    fn reads_20_without_cutting_the_2() {
        let bytes = include_bytes!("../../tests/fixtures/diamond_normal_20.png");
        let roi = image::load_from_memory(bytes)
            .expect("fixture PNG")
            .to_rgb8();
        let templates = TemplateSet::load().expect("templates");
        let reading = read_count(&roi, 14, 22, &templates, false);
        assert_eq!(reading.best_candidate, Some(20));
    }

    #[test]
    fn reads_20_in_colored_columns() {
        let templates = TemplateSet::load().expect("templates");
        let fixtures: [&[u8]; 2] = [
            &include_bytes!("../../tests/fixtures/emerald_normal_20.png")[..],
            &include_bytes!("../../tests/fixtures/ruby_normal_20.png")[..],
        ];
        for bytes in fixtures {
            let roi = image::load_from_memory(bytes)
                .expect("fixture PNG")
                .to_rgb8();
            let reading = read_count(&roi, 14, 22, &templates, false);
            assert_eq!(reading.best_candidate, Some(20));
        }
    }

    #[test]
    fn reads_diamond_parfait_27() {
        let bytes = include_bytes!("../../tests/fixtures/diamond_parfait_27.png");
        let roi = image::load_from_memory(bytes)
            .expect("fixture PNG")
            .to_rgb8();
        let templates = TemplateSet::load().expect("templates");
        let reading = read_count(&roi, 14, 22, &templates, false);
        assert_eq!(reading.best_candidate, Some(27));
        assert!(reading.is_confident());
    }

    #[test]
    fn accepts_high_score_with_low_margin() {
        assert!(is_confident_score(0.782, 0.014));
        assert!(!is_confident_score(0.782, 0.005));
        assert!(!is_confident_score(0.70, 0.014));
    }

    #[test]
    fn keeps_stricter_rule_for_lower_scores() {
        assert!(is_confident_score(0.50, 0.05));
        assert!(!is_confident_score(0.50, 0.03));
    }
}

fn tight_mask(image: &GrayImage) -> Option<(u32, u32, Vec<bool>)> {
    let width = image.width();
    let height = image.height();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;

    for y in 0..height {
        for x in 0..width {
            if image.get_pixel(x, y).0[0] >= 110 {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if !any {
        return None;
    }

    let mask_width = max_x - min_x + 1;
    let mask_height = max_y - min_y + 1;
    let mut mask = Vec::with_capacity((mask_width * mask_height) as usize);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            mask.push(image.get_pixel(x, y).0[0] >= 110);
        }
    }

    Some((mask_width, mask_height, mask))
}
