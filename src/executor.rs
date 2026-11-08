//! Exécuteur de transmutations.
//!
//! Le mode par défaut est la simulation : on lit l'état, on calcule le plan et
//! on affiche la première action sans cliquer. Le mode `--execute` utilise X11
//! et XTest, vérifie chaque étape et tente de récupérer le cube en cas
//! d'échec. F9 et un mouvement de souris sont des arrêts d'urgence.

use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};

use crate::config::Config;
use crate::logging::{epoch_seconds, Logger};
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

/// Erreur d'arrêt de sécurité : on ne tente pas de reprendre le cube, car
/// l'utilisateur a demandé l'arrêt ou a repris la souris.
#[derive(Debug)]
struct SafetyStop(String);

impl std::fmt::Display for SafetyStop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for SafetyStop {}

fn safety(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(SafetyStop(message.into()))
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

    let mut logger = Logger::new();
    let log_path = logger.path().display().to_string();
    logger.log(format!("Journal: {log_path}"));

    let image = crate::load_game_image(image_path, profile)?;
    let table = table::read_table(&image, profile, templates, options.debug)?;
    if !table.tab_active {
        bail!("onglet Gemmes non actif: sélectionne l'onglet Gemmes");
    }
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

    logger.log(format!("Table lue: {} gemmes connues", table.known_total()));
    logger.log(format!(
        "Cube: {} case(s) occupée(s)",
        cube.occupied.iter().filter(|occupied| **occupied).count()
    ));
    logger.log(format!(
        "Plan: {} transmutations, {} parfaites gagnées",
        plan.conversion_count(),
        plan.total_perfect_gain()
    ));

    if let Some(action) = planner::next_action(&counts, &keep) {
        let x = profile.panel.x
            + profile.table.count_right_edges[action.gem_type]
            + profile.click.source_dx;
        let y = profile.panel.y + profile.table.count_tops[action.tier] + profile.click.source_dy;
        logger.log(format!(
            "Première action: {} {} -> {} (source panneau: {x},{y})",
            GEM_TYPES[action.gem_type],
            TIERS[action.tier],
            TIERS[action.tier + 1]
        ));
    } else {
        logger.log("Aucune transmutation à faire.");
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
    let mut logger = Logger::new();
    let log_path = logger.path().display().to_string();
    logger.log(format!("Journal: {log_path}"));
    logger.log(format!(
        "Fenêtre: {} ({}x{}) @ ({}, {})",
        window.title, window.width, window.height, window.x, window.y
    ));
    logger.log("Arrêt d'urgence: F9, ou déplace la souris hors du point de repos.");

    let result = run_live_inner(
        &x11,
        &window,
        profile,
        config,
        templates,
        options,
        &mut logger,
    );

    if let Err(error) = &result {
        logger.log(format!("ERREUR: {error:#}"));
        save_error_frame(&x11, &window, &mut logger);
        if error.downcast_ref::<SafetyStop>().is_none() {
            match recover_cube(&x11, &window, profile, &mut logger) {
                Ok(()) => logger.log("Reprise: cube vidé."),
                Err(recovery_error) => {
                    logger.log(format!("Reprise impossible: {recovery_error:#}"));
                }
            }
        } else {
            logger.log("Arrêt de sécurité: aucune reprise automatique.");
        }
    }

    result
}

fn run_live_inner(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    config: &Config,
    templates: &TemplateSet,
    options: RunOptions,
    logger: &mut Logger,
) -> Result<()> {
    if window.width != profile.image_width || window.height != profile.image_height {
        return Err(safety(format!(
            "la fenêtre du jeu fait {}x{}, mais le profil attend {}x{}",
            window.width, window.height, profile.image_width, profile.image_height
        )));
    }

    x11.focus_window(window)
        .map_err(|error| safety(format!("impossible de donner le focus au jeu: {error:#}")))?;
    sleep(Duration::from_millis(200));
    check_focus(x11, window)?;

    park(x11, window, profile)?;
    let image = x11.capture_window(window)?;
    let table = table::read_table(&image, profile, templates, options.debug)
        .map_err(|error| safety(format!("lecture de la table impossible: {error:#}")))?;
    if !table.tab_active {
        return Err(safety(
            "onglet Gemmes non actif: sélectionne l'onglet Gemmes",
        ));
    }
    if !table.detected {
        return Err(safety(
            "table non détectée: ouvre le cache et l'onglet Gemmes",
        ));
    }
    if !table.uncertain.is_empty() {
        return Err(safety(format!(
            "impossible d'exécuter: {} cellule(s) incertaine(s); ouvre le cache, \
             l'onglet Gemmes, puis réessaie",
            table.uncertain.len()
        )));
    }

    let cube = cube::read_cube(&image, profile)?;
    if cube.occupied.iter().any(|occupied| *occupied) {
        return Err(safety(
            "le cube horadrique doit être vide avant de commencer",
        ));
    }

    let counts = crate::counts_from_table(&table)?;
    let keep = config.keep_matrix()?;
    let plan = planner::project(counts.clone(), &keep);
    logger.log(format!(
        "Plan: {} transmutations, {} parfaites gagnées",
        plan.conversion_count(),
        plan.total_perfect_gain()
    ));

    if !options.execute {
        if let Some(action) = planner::next_action(&counts, &keep) {
            let (x, y) = cell_click_point(window, profile, action.gem_type, action.tier);
            logger.log(format!(
                "Mode simulation: première action {} {} -> {} (clic source à {x},{y})",
                GEM_TYPES[action.gem_type],
                TIERS[action.tier],
                TIERS[action.tier + 1]
            ));
        } else {
            logger.log("Aucune transmutation à faire.");
        }
        return Ok(());
    }

    let mut counts = counts;
    let mut done = 0usize;

    for step in 1..=options.max_transmutations {
        check_focus(x11, window)?;
        check_emergency(x11)?;

        let Some(action) = planner::next_action(&counts, &keep) else {
            logger.log("Plus aucune transmutation valide.");
            break;
        };

        let source_before = counts[action.gem_type][action.tier];
        let target_before = counts[action.gem_type][action.tier + 1];
        logger.log(format!(
            "[{step}] {} {} -> {}",
            GEM_TYPES[action.gem_type],
            TIERS[action.tier],
            TIERS[action.tier + 1]
        ));

        acquire_source(
            x11,
            window,
            profile,
            templates,
            logger,
            action.gem_type,
            action.tier,
            source_before,
        )?;

        let cube_after_source = read_cube_live(x11, window, profile)?;
        let occupied = cube_after_source
            .occupied
            .iter()
            .filter(|occupied| **occupied)
            .count();
        logger.log(format!(
            "  cube après source: {occupied} case(s) occupée(s)"
        ));
        if occupied != 3 {
            bail!(
                "le cube devrait contenir 3 gemmes après l'envoi, \
                 il en contient {occupied}"
            );
        }

        let result_slot = transmute_with_retry(x11, window, profile, logger)?;
        logger.log(format!("  gemme transmutée dans la case {result_slot}"));

        retrieve_with_retry(
            x11,
            window,
            profile,
            templates,
            logger,
            action.gem_type,
            action.tier + 1,
            target_before,
        )?;

        counts[action.gem_type][action.tier] = source_before.saturating_sub(3);
        counts[action.gem_type][action.tier + 1] = target_before.saturating_add(1);
        done += 1;
        logger.log("  ok");
    }

    logger.log(format!("{done} transmutation(s) effectuée(s)."));

    park(x11, window, profile)?;
    let final_image = x11.capture_window(window)?;
    let final_table = table::read_table(&final_image, profile, templates, false)?;
    logger.log(format!(
        "Total lu après exécution: {}",
        final_table.known_total()
    ));
    if !final_table.detected {
        logger.log("Attention: table non détectée après l'exécution.");
    } else if !final_table.uncertain.is_empty() {
        logger.log(format!(
            "Attention: {} cellule(s) incertaine(s) après l'exécution.",
            final_table.uncertain.len()
        ));
    }

    Ok(())
}

/// Envoie un trio dans le cube. Le cube sert de source de vérité si la lecture
/// du nombre source est en retard. Si seulement 1 ou 2 gemmes sont arrivées,
/// on complète avec des clics gauches.
fn acquire_source(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    col: usize,
    row: usize,
    source_before: u32,
) -> Result<()> {
    let expected = source_before.saturating_sub(3);
    let (source_x, source_y) = cell_click_point(window, profile, col, row);

    for attempt in 1..=3 {
        check_focus(x11, window)?;
        check_emergency(x11)?;
        check_tab_live(x11, window, profile)?;
        x11.click(source_x, source_y, MouseButton::Right, Modifiers::CtrlShift)?;

        if wait_cell(
            x11,
            window,
            profile,
            templates,
            col,
            row,
            expected,
            Duration::from_secs(2),
        )?
        .is_some()
        {
            return Ok(());
        }

        let cube = read_cube_live(x11, window, profile)?;
        let occupied = cube.occupied.iter().filter(|occupied| **occupied).count();
        logger.log(format!(
            "  source tentative {attempt}: {occupied} gemme(s) dans le cube"
        ));

        match occupied {
            3 => return Ok(()),
            1 | 2 => {
                for _ in occupied..3 {
                    check_focus(x11, window)?;
                    check_emergency(x11)?;
                    check_tab_live(x11, window, profile)?;
                    x11.click(source_x, source_y, MouseButton::Left, Modifiers::CtrlShift)?;
                    sleep(Duration::from_millis(150));
                }
                let cube = read_cube_live(x11, window, profile)?;
                let completed = cube.occupied.iter().filter(|occupied| **occupied).count();
                if completed == 3 {
                    return Ok(());
                }
                bail!("le trio est incomplet dans le cube ({completed}/3)");
            }
            0 => continue,
            other => bail!("état de cube inattendu pendant l'envoi: {other} cases"),
        }
    }

    bail!("impossible d'envoyer 3 gemmes dans le cube après 3 tentatives")
}

fn transmute_with_retry(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    logger: &mut Logger,
) -> Result<usize> {
    let (transmute_x, transmute_y) = transmute_point(window, profile);

    for attempt in 1..=3 {
        check_focus(x11, window)?;
        check_emergency(x11)?;
        check_tab_live(x11, window, profile)?;
        x11.click(transmute_x, transmute_y, MouseButton::Left, Modifiers::None)?;
        sleep(Duration::from_millis(500));

        let cube = read_cube_live(x11, window, profile)?;
        let occupied: Vec<usize> = cube
            .occupied
            .iter()
            .enumerate()
            .filter_map(|(index, occupied)| occupied.then_some(index))
            .collect();

        match occupied.len() {
            1 => return Ok(occupied[0]),
            3 => {
                logger.log(format!(
                    "  bouton Transmuter tentative {attempt} sans effet, reprise"
                ));
            }
            0 => bail!("le cube est vide après le clic Transmuter"),
            other => bail!("état de cube inattendu après Transmuter: {other} cases"),
        }
    }

    bail!("le bouton Transmuter n'a pas fonctionné après 3 tentatives")
}

fn retrieve_with_retry(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    col: usize,
    row: usize,
    target_before: u32,
) -> Result<()> {
    let expected = target_before.saturating_add(1);

    for attempt in 1..=3 {
        check_focus(x11, window)?;
        check_emergency(x11)?;
        check_tab_live(x11, window, profile)?;

        let cube = read_cube_live(x11, window, profile)?;
        let Some(result_slot) = cube.occupied.iter().position(|occupied| *occupied) else {
            if wait_cell(
                x11,
                window,
                profile,
                templates,
                col,
                row,
                expected,
                Duration::from_secs(1),
            )?
            .is_some()
            {
                return Ok(());
            }
            bail!("le cube est vide mais la quantité cible n'a pas augmenté");
        };

        let (result_x, result_y) = cube_slot_point(window, profile, result_slot);
        x11.click(result_x, result_y, MouseButton::Left, Modifiers::CtrlShift)?;

        if wait_cell(
            x11,
            window,
            profile,
            templates,
            col,
            row,
            expected,
            Duration::from_secs(2),
        )?
        .is_some()
        {
            let cube = read_cube_live(x11, window, profile)?;
            if cube.occupied.iter().any(|occupied| *occupied) {
                bail!("le cube n'est pas vide après la réinscription de la gemme");
            }
            return Ok(());
        }

        logger.log(format!(
            "  réinsertion tentative {attempt} sans effet, reprise"
        ));
    }

    bail!("impossible de réinsérer la gemme dans la table après 3 tentatives")
}

/// Attend que la cellule atteigne la valeur attendue. Retourne `None` si le
/// délai expire sans erreur de sécurité.
fn wait_cell(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    templates: &TemplateSet,
    col: usize,
    row: usize,
    expected: u32,
    timeout: Duration,
) -> Result<Option<CountReading>> {
    let expected_u16 = u16::try_from(expected)
        .map_err(|_| anyhow!("valeur attendue trop grande pour une cellule: {expected}"))?;
    let deadline = Instant::now() + timeout;

    loop {
        park(x11, window, profile)?;
        let image = x11.capture_window(window)?;
        let reading = table::read_cell(&image, profile, templates, col, row, false)?;
        if reading.is_confident() && reading.number == Some(expected_u16) {
            return Ok(Some(reading));
        }

        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(Duration::from_millis(100));
    }
}

fn read_cube_live(x11: &X11, window: &GameWindow, profile: &Profile) -> Result<CubeReading> {
    park(x11, window, profile)?;
    let image = x11.capture_window(window)?;
    cube::read_cube(&image, profile)
}

/// Déplace le curseur au point de repos. Avant de le déplacer, vérifie que
/// l'utilisateur n'a pas bougé la souris depuis la dernière action du
/// programme. C'est le failsafe de souris, en plus de F9.
fn park(x11: &X11, window: &GameWindow, profile: &Profile) -> Result<()> {
    if x11.pointer_moved_by_user()? {
        return Err(safety("mouvement de souris détecté: arrêt"));
    }

    let (park_x, park_y) = park_point(window, profile);
    x11.move_pointer(park_x, park_y)?;
    sleep(Duration::from_millis(120));
    check_emergency(x11)?;
    x11.note_pointer()?;
    Ok(())
}

fn check_focus(x11: &X11, window: &GameWindow) -> Result<()> {
    if !x11.is_focused(window)? {
        return Err(safety("le jeu a perdu le focus"));
    }
    Ok(())
}

fn check_emergency(x11: &X11) -> Result<()> {
    if x11.emergency_requested()? {
        return Err(safety("arrêt d'urgence demandé (F9)"));
    }
    Ok(())
}

fn check_tab_live(x11: &X11, window: &GameWindow, profile: &Profile) -> Result<()> {
    let image = x11.capture_window(window)?;
    if !crate::vision::tabs::gemmes_active(&image, profile)? {
        return Err(safety("l'onglet Gemmes n'est plus actif"));
    }
    Ok(())
}

fn recover_cube(
    x11: &X11,
    window: &GameWindow,
    profile: &Profile,
    logger: &mut Logger,
) -> Result<()> {
    for attempt in 1..=3 {
        let cube = read_cube_live(x11, window, profile)?;
        let occupied: Vec<usize> = cube
            .occupied
            .iter()
            .enumerate()
            .filter_map(|(index, occupied)| occupied.then_some(index))
            .collect();
        if occupied.is_empty() {
            return Ok(());
        }

        logger.log(format!(
            "  reprise cube tentative {attempt}: {} case(s) à vider",
            occupied.len()
        ));
        for index in occupied {
            check_focus(x11, window)?;
            check_emergency(x11)?;
            let (x, y) = cube_slot_point(window, profile, index);
            x11.click(x, y, MouseButton::Left, Modifiers::CtrlShift)?;
            sleep(Duration::from_millis(200));
        }
    }

    let cube = read_cube_live(x11, window, profile)?;
    if cube.occupied.iter().any(|occupied| *occupied) {
        bail!("le cube n'a pas pu être vidé");
    }
    Ok(())
}

fn save_error_frame(x11: &X11, window: &GameWindow, logger: &mut Logger) {
    let path = format!("logs/error-{}.png", epoch_seconds());
    match x11.capture_window(window) {
        Ok(image) => match image.save(&path) {
            Ok(()) => logger.log(format!("Capture d'erreur: {path}")),
            Err(error) => logger.log(format!("Échec de sauvegarde de la capture: {error}")),
        },
        Err(error) => logger.log(format!("Échec de capture d'erreur: {error:#}")),
    }
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

fn park_point(window: &GameWindow, profile: &Profile) -> (i32, i32) {
    (
        window.x + profile.panel.x + profile.click.park_x,
        window.y + profile.panel.y + profile.click.park_y,
    )
}
