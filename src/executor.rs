//! Exécuteur de transmutations.
//!
//! Le mode par défaut est la simulation : on lit l'état, on calcule le plan et
//! on affiche la première action sans cliquer. Le mode `--execute` utilise X11
//! et XTest pour faire les clics Ctrl+Shift.

use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};

use crate::config::Config;
use crate::planner;
use crate::platform::x11::{GameWindow, Modifiers, MouseButton, X11};
use crate::profile::Profile;
use crate::vision::cube::{self, CubeReading};
use crate::vision::digits::{CountReading, TemplateSet};
use crate::vision::table::{self, GEM_TYPES, TIERS};

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub execute: bool,
    pub max_transmutations: usize,
    pub debug: bool,
}

pub fn run_from_image(
    image_path: &Path,
    profile: &Profile,
    config: &Config,
    templates: &TemplateSet,
    options: RunOptions,
) -> Result<()> {
    if options.execute {
        bail!("le mode --image ne peut pas cliquer; utilise une fenêtre X11 pour --execute");
    }

    let image = crate::load_game_image(image_path, profile)?;
    let table = table::read_table(&image, profile, templates, options.debug)?;
    if !table.detected {
        bail!("table non détectée: ouvre le cache et l'onglet Gemmes");
    }
    if !table.uncertain.is_empty() {
        bail!("impossible d'exécuter: la lecture du tableau n'est pas fiable");
    }
    let cube = cube::read_cube(&image, profile)?;
    let counts = crate::counts_from_table(&table)?;
    let keep = config.keep_matrix()?;
    let plan = planner::project(counts.clone(), &keep);

    println!("Table lue: {} gemmes connues", table.known_total());
    println!(
        "Cube: {} case(s) occupée(s)",
        cube.occupied.iter().filter(|occupied| **occupied).count()
    );
    println!(
        "Plan: {} transmutations, {} parfaites gagnées",
        plan.conversion_count(),
        plan.total_perfect_gain()
    );

    if let Some(action) = planner::next_action(&counts, &keep) {
        let x = profile.panel.x
            + profile.table.count_right_edges[action.gem_type]
            + profile.click.source_dx;
        let y = profile.panel.y + profile.table.count_tops[action.tier] + profile.click.source_dy;
        println!(
            "Première action: {} {} -> {} (source panneau: {x},{y})",
            GEM_TYPES[action.gem_type],
            TIERS[action.tier],
            TIERS[action.tier + 1]
        );
    } else {
        println!("Aucune transmutation à faire.");
    }

    Ok(())
}

pub fn run_live(
    profile: &Profile,
    config: &Config,
    templates: &TemplateSet,
    options: RunOptions,
) -> Result<()> {
    let x11 = X11::new()?;
    let window = x11.find_game_window()?;
    println!(
        "Fenêtre: {} ({}x{}) @ ({}, {})",
        window.title, window.width, window.height, window.x, window.y
    );

    if window.width != profile.image_width || window.height != profile.image_height {
        bail!(
            "la fenêtre du jeu fait {}x{}, mais le profil attend {}x{}. \
             Ouvre le jeu à la résolution du profil.",
            window.width,
            window.height,
            profile.image_width,
            profile.image_height
        );
    }

    x11.focus_window(&window)?;
    sleep(Duration::from_millis(200));
    if !x11.is_focused(&window)? {
        bail!("le jeu n'a pas le focus; clique la fenêtre du jeu puis réessaie");
    }

    park(&x11, &window, profile)?;
    let image = x11.capture_window(&window)?;
    let table = table::read_table(&image, profile, templates, options.debug)?;
    if !table.detected {
        bail!("table non détectée: ouvre le cache et l'onglet Gemmes");
    }
    if !table.uncertain.is_empty() {
        bail!(
            "impossible d'exécuter: {} cellule(s) incertaine(s); ouvre le cache, \
             l'onglet Gemmes, puis réessaie",
            table.uncertain.len()
        );
    }

    let cube = cube::read_cube(&image, profile)?;
    if cube.occupied.iter().any(|occupied| *occupied) {
        bail!("le cube horadrique doit être vide avant de commencer");
    }

    let counts = crate::counts_from_table(&table)?;
    let keep = config.keep_matrix()?;
    let plan = planner::project(counts.clone(), &keep);
    println!(
        "Plan: {} transmutations, {} parfaites gagnées",
        plan.conversion_count(),
        plan.total_perfect_gain()
    );

    if !options.execute {
        if let Some(action) = planner::next_action(&counts, &keep) {
            let (x, y) = cell_click_point(&window, profile, action.gem_type, action.tier);
            println!(
                "Mode simulation: première action {} {} -> {} (clic source à {x},{y})",
                GEM_TYPES[action.gem_type],
                TIERS[action.tier],
                TIERS[action.tier + 1]
            );
        } else {
            println!("Aucune transmutation à faire.");
        }
        return Ok(());
    }

    let mut counts = counts;
    let mut done = 0usize;
    for step in 1..=options.max_transmutations {
        let Some(action) = planner::next_action(&counts, &keep) else {
            println!("Plus aucune transmutation valide.");
            break;
        };

        if !x11.is_focused(&window)? {
            bail!("le jeu a perdu le focus à l'étape {step}; arrêt");
        }

        let source_before = counts[action.gem_type][action.tier];
        let target_before = counts[action.gem_type][action.tier + 1];
        println!(
            "[{step}] {} {} -> {}",
            GEM_TYPES[action.gem_type],
            TIERS[action.tier],
            TIERS[action.tier + 1]
        );

        let (source_x, source_y) = cell_click_point(&window, profile, action.gem_type, action.tier);
        x11.click(source_x, source_y, MouseButton::Right, Modifiers::CtrlShift)?;

        park(&x11, &window, profile)?;
        wait_cell(
            &x11,
            &window,
            profile,
            templates,
            action.gem_type,
            action.tier,
            source_before.saturating_sub(3),
            Duration::from_secs(3),
        )?;

        let cube_after_source = read_cube_live(&x11, &window, profile)?;
        println!(
            "  cube après source: {} case(s) occupée(s)",
            cube_after_source
                .occupied
                .iter()
                .filter(|occupied| **occupied)
                .count()
        );

        let (transmute_x, transmute_y) = transmute_point(&window, profile);
        x11.click(transmute_x, transmute_y, MouseButton::Left, Modifiers::None)?;
        sleep(Duration::from_millis(500));

        let cube_after_transmute = read_cube_live(&x11, &window, profile)?;
        let result_slot = cube_after_transmute
            .occupied
            .iter()
            .position(|occupied| *occupied)
            .ok_or_else(|| anyhow!("la gemme transmutée n'apparaît pas dans le cube"))?;
        let (result_x, result_y) = cube_slot_point(&window, profile, result_slot);
        x11.click(result_x, result_y, MouseButton::Left, Modifiers::CtrlShift)?;

        park(&x11, &window, profile)?;
        wait_cell(
            &x11,
            &window,
            profile,
            templates,
            action.gem_type,
            action.tier + 1,
            target_before.saturating_add(1),
            Duration::from_secs(3),
        )?;

        let cube_after_retrieve = read_cube_live(&x11, &window, profile)?;
        if cube_after_retrieve
            .occupied
            .iter()
            .any(|occupied| *occupied)
        {
            bail!("le cube n'est pas vide après la réinscription de la gemme");
        }

        counts[action.gem_type][action.tier] = source_before.saturating_sub(3);
        counts[action.gem_type][action.tier + 1] = target_before.saturating_add(1);
        done += 1;
    }

    println!("{done} transmutation(s) effectuée(s).");
    if done < options.max_transmutations {
        println!("Arrêt: plus aucune action valide ou limite atteinte.");
    }

    park(&x11, &window, profile)?;
    let final_image = x11.capture_window(&window)?;
    let final_table = table::read_table(&final_image, profile, templates, false)?;
    println!("Total lu après exécution: {}", final_table.known_total());
    if !final_table.uncertain.is_empty() {
        println!(
            "Attention: {} cellule(s) incertaine(s) après l'exécution.",
            final_table.uncertain.len()
        );
    }

    Ok(())
}

fn wait_cell(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    templates: &TemplateSet,
    col: usize,
    row: usize,
    expected: u32,
    timeout: Duration,
) -> Result<CountReading> {
    let expected_u16 = u16::try_from(expected)
        .map_err(|_| anyhow!("valeur attendue trop grande pour une cellule: {expected}"))?;
    let deadline = Instant::now() + timeout;
    let mut last_value = None;

    loop {
        park(x11, window, profile)?;
        let image = x11.capture_window(window)?;
        let reading = table::read_cell(&image, profile, templates, col, row, false)?;
        if reading.is_confident() {
            last_value = reading.number;
            if reading.number == Some(expected_u16) {
                return Ok(reading);
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "délai dépassé en attendant {} / {} = {} (dernière lecture: {:?})",
                GEM_TYPES[col],
                TIERS[row],
                expected,
                last_value
            );
        }
        sleep(Duration::from_millis(100));
    }
}

fn read_cube_live(x11: &X11, window: &GameWindow, profile: &Profile) -> Result<CubeReading> {
    park(x11, window, profile)?;
    let image = x11.capture_window(window)?;
    cube::read_cube(&image, profile)
}

fn park(x11: &X11, window: &GameWindow, profile: &Profile) -> Result<()> {
    let x = window.x + profile.panel.x + profile.click.park_x;
    let y = window.y + profile.panel.y + profile.click.park_y;
    x11.move_pointer(x, y)?;
    sleep(Duration::from_millis(120));
    Ok(())
}

fn cell_click_point(window: &GameWindow, profile: &Profile, col: usize, row: usize) -> (i32, i32) {
    let x =
        window.x + profile.panel.x + profile.table.count_right_edges[col] + profile.click.source_dx;
    let y = window.y + profile.panel.y + profile.table.count_tops[row] + profile.click.source_dy;
    (x, y)
}

fn transmute_point(window: &GameWindow, profile: &Profile) -> (i32, i32) {
    (
        window.x + profile.panel.x + profile.click.transmute_x,
        window.y + profile.panel.y + profile.click.transmute_y,
    )
}

fn cube_slot_point(window: &GameWindow, profile: &Profile, index: usize) -> (i32, i32) {
    let slot = profile.cube.slots[index];
    (
        window.x + profile.panel.x + slot.x + slot.width as i32 / 2,
        window.y + profile.panel.y + slot.y + slot.height as i32 / 2,
    )
}
