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
use crate::platform::{Modifiers, MouseButton, Platform, Point, WindowInfo};
use crate::profile::Profile;
use crate::vision::cube::{self, CubeReading};
use crate::vision::digits::{CountReading, TemplateSet};
use crate::vision::table::{self, GEM_TYPES, TIERS};

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub execute: bool,
    /// `None` signifie jusqu'à épuisement des actions valides.
    pub max_transmutations: Option<usize>,
    pub start_tier: usize,
    pub audit_every: usize,
    pub debug: bool,
    pub timing: Timing,
}

/// Temporisations d'orchestration de l'exécuteur.
///
/// Les délais internes au clic (maintien des modificateurs, du bouton, etc.)
/// restent dans le composant dorsal : ils font partie de la garantie du
/// contrat. Ces valeurs-ci ne concernent que la boucle d'exécution.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub after_focus: Duration,
    pub transmute_wait: Duration,
    pub poll: Duration,
    pub park_settle: Duration,
    pub retry_pause: Duration,
    pub cell_timeout: Duration,
    pub cube_timeout: Duration,
}

impl Timing {
    pub fn real() -> Self {
        Self {
            after_focus: Duration::from_millis(200),
            transmute_wait: Duration::from_millis(350),
            poll: Duration::from_millis(60),
            park_settle: Duration::from_millis(60),
            retry_pause: Duration::from_millis(100),
            cell_timeout: Duration::from_secs(2),
            cube_timeout: Duration::from_millis(500),
        }
    }

    pub fn fast() -> Self {
        Self {
            after_focus: Duration::from_millis(1),
            transmute_wait: Duration::from_millis(1),
            poll: Duration::from_millis(1),
            park_settle: Duration::from_millis(1),
            retry_pause: Duration::from_millis(1),
            cell_timeout: Duration::from_millis(5),
            cube_timeout: Duration::from_millis(5),
        }
    }
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
    let plan = planner::project(counts.clone(), &keep, options.start_tier);

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
    logger.log(format!(
        "Départ: {} ({})",
        TIERS[options.start_tier], options.start_tier
    ));

    if let Some(action) = planner::next_action(&counts, &keep, options.start_tier) {
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
    let _terminal_guard = crate::hotkeys::start_terminal();
    let platform = crate::platform::native()?;
    run_on_platform(&platform, profile, config, templates, options)
}

/// Boucle d'exécution sur une plateforme donnée. Séparée de `run_live` pour
/// que les tests puissent fournir une implémentation factice.
fn run_on_platform(
    platform: &impl Platform,
    profile: &Profile,
    config: &Config,
    templates: &TemplateSet,
    options: RunOptions,
) -> Result<()> {
    let window = platform.find_game_window()?;
    let timing = options.timing;
    let mut logger = Logger::new();
    let log_path = logger.path().display().to_string();
    logger.log(format!("Journal: {log_path}"));
    logger.log(format!(
        "Fenêtre: {} ({}x{}) @ ({}, {})",
        window.title, window.size.width, window.size.height, window.origin.x, window.origin.y
    ));
    logger.log("Pause/reprise: F8. Arrêt d'urgence: F9 ou mouvement de souris.");

    let result = run_live_inner(
        platform,
        &window,
        profile,
        config,
        templates,
        options,
        &mut logger,
    );

    if let Err(error) = &result {
        logger.log(format!("ERREUR: {error:#}"));
        notify("QuickGems - erreur", &format!("{error:#}"));
        save_error_frame(platform, &window, &mut logger);
        if error.downcast_ref::<SafetyStop>().is_none() {
            match recover_cube(platform, &window, profile, &mut logger, timing) {
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
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    config: &Config,
    templates: &TemplateSet,
    options: RunOptions,
    logger: &mut Logger,
) -> Result<()> {
    let timing = options.timing;
    if window.size.width != profile.image_width || window.size.height != profile.image_height {
        return Err(safety(format!(
            "la fenêtre du jeu fait {}x{}, mais le profil attend {}x{}",
            window.size.width, window.size.height, profile.image_width, profile.image_height
        )));
    }

    platform
        .focus(window)
        .map_err(|error| safety(format!("impossible de donner le focus au jeu: {error:#}")))?;
    sleep(timing.after_focus);
    check_focus(platform, window)?;

    park(platform, window, profile, timing)?;
    let image = platform.capture(window)?;
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
    let plan = planner::project(counts.clone(), &keep, options.start_tier);
    logger.log(format!(
        "Plan: {} transmutations, {} parfaites gagnées",
        plan.conversion_count(),
        plan.total_perfect_gain()
    ));
    logger.log(format!(
        "Départ: {} ({})",
        TIERS[options.start_tier], options.start_tier
    ));
    if options.max_transmutations.is_none() {
        logger.log("Mode: jusqu'à épuisement des actions valides.");
    }

    if !options.execute {
        if let Some(action) = planner::next_action(&counts, &keep, options.start_tier) {
            let point = cell_click_point(window, profile, action.gem_type, action.tier);
            logger.log(format!(
                "Mode simulation: première action {} {} -> {} (clic source à {},{})",
                GEM_TYPES[action.gem_type],
                TIERS[action.tier],
                TIERS[action.tier + 1],
                point.x,
                point.y
            ));
        } else {
            logger.log("Aucune transmutation à faire.");
        }
        return Ok(());
    }

    let timing = options.timing;
    let mut counts = counts;
    let mut done = 0usize;
    let started = Instant::now();
    let mut performed = [[0u32; 4]; 7];

    let mut step = 0usize;
    loop {
        step += 1;
        if let Some(max) = options.max_transmutations {
            if step > max {
                break;
            }
        }
        check_emergency(platform)?;
        if platform.take_pause() || crate::hotkeys::take_pause() {
            wait_for_resume(
                platform,
                window,
                profile,
                templates,
                logger,
                &mut counts,
                timing,
            )?;
        }
        check_focus(platform, window)?;

        let Some(action) = planner::next_action(&counts, &keep, options.start_tier) else {
            report_full_receivers(logger, &counts, &keep, options.start_tier);
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
            platform,
            window,
            profile,
            templates,
            logger,
            action.gem_type,
            action.tier,
            source_before,
            timing,
        )?;

        let cube_after_source = read_cube_live(platform, window, profile, timing)?;
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

        let result_slot = transmute_with_retry(platform, window, profile, logger, timing)?;
        logger.log(format!("  gemme transmutée dans la case {result_slot}"));

        retrieve_with_retry(
            platform,
            window,
            profile,
            templates,
            logger,
            action.gem_type,
            action.tier + 1,
            target_before,
            timing,
        )?;

        counts[action.gem_type][action.tier] = source_before.saturating_sub(3);
        counts[action.gem_type][action.tier + 1] = target_before.saturating_add(1);
        performed[action.gem_type][action.tier] += 1;
        done += 1;

        if options.audit_every > 0 && step % options.audit_every == 0 {
            audit_table(
                platform, window, profile, templates, logger, &counts, step, timing,
            )?;
        }
        logger.log("  ok");
    }

    logger.log(format!("{done} transmutation(s) effectuée(s)."));

    park(platform, window, profile, timing)?;
    let final_image = platform.capture(window)?;
    let final_table = table::read_table(&final_image, profile, templates, false)?;
    let model_total: u32 = counts.iter().flatten().sum();
    logger.log(format!("Total selon le modèle: {model_total}"));
    logger.log(format!(
        "Total relu après exécution: {}",
        final_table.known_total()
    ));
    if !final_table.detected {
        logger.log("Attention: table non détectée après l'exécution.");
    } else if !final_table.uncertain.is_empty() {
        logger.log(format!(
            "Note: {} cellule(s) relue(s) avec une marge faible; \
             les actions ont été vérifiées avec la valeur attendue.",
            final_table.uncertain.len()
        ));
    }

    let seconds = started.elapsed().as_secs_f64();
    let per_conversion = if done > 0 { seconds / done as f64 } else { 0.0 };
    logger.log(format!(
        "Durée totale (pauses incluses): {seconds:.1}s pour {done} conversion(s), \
         {per_conversion:.2}s/conversion."
    ));
    logger.log("Conversions effectuées:");
    for gem_type in 0..7 {
        for tier in 0..4 {
            let count = performed[gem_type][tier];
            if count > 0 {
                logger.log(format!(
                    "  {}: {} -> {} x{}",
                    GEM_TYPES[gem_type],
                    TIERS[tier],
                    TIERS[tier + 1],
                    count
                ));
            }
        }
    }
    notify(
        "QuickGems",
        &format!("{done} conversion(s), total modèle {model_total}"),
    );

    Ok(())
}

/// Envoie un trio dans le cube. Le cube sert de source de vérité si la lecture
/// du nombre source est en retard. Si seulement 1 ou 2 gemmes sont arrivées,
/// on complète avec des clics gauches.
fn acquire_source(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    col: usize,
    row: usize,
    source_before: u32,
    timing: Timing,
) -> Result<()> {
    let expected = source_before.saturating_sub(3);
    let source_point = cell_click_point(window, profile, col, row);

    for attempt in 1..=3 {
        check_focus(platform, window)?;
        check_emergency(platform)?;
        check_tab_live(platform, window, profile)?;
        platform.click(source_point, MouseButton::Right, Modifiers::CTRL_SHIFT)?;

        if let Some(reading) = wait_cell(
            platform, window, profile, templates, col, row, expected, timing,
        )? {
            if !reading.is_confident() {
                logger.log(format!(
                    "  source acceptée avec marge faible: {:?} score {:.3} marge {:.3}",
                    reading.best_candidate, reading.score, reading.margin
                ));
            }
            return Ok(());
        }

        let cube = read_cube_live(platform, window, profile, timing)?;
        let occupied = cube.occupied.iter().filter(|occupied| **occupied).count();
        logger.log(format!(
            "  source tentative {attempt}: {occupied} gemme(s) dans le cube"
        ));

        match occupied {
            3 => return Ok(()),
            1 | 2 => {
                for _ in occupied..3 {
                    check_focus(platform, window)?;
                    check_emergency(platform)?;
                    check_tab_live(platform, window, profile)?;
                    platform.click(source_point, MouseButton::Left, Modifiers::CTRL_SHIFT)?;
                    sleep(timing.retry_pause);
                }
                let cube = read_cube_live(platform, window, profile, timing)?;
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
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    logger: &mut Logger,
    timing: Timing,
) -> Result<usize> {
    let transmute_at = transmute_point(window, profile);

    for attempt in 1..=3 {
        check_focus(platform, window)?;
        check_emergency(platform)?;
        check_tab_live(platform, window, profile)?;
        platform.click(transmute_at, MouseButton::Left, Modifiers::NONE)?;
        sleep(timing.transmute_wait);

        // La transmutation peut prendre quelques images. On interroge le cube
        // jusqu'à ce qu'il passe de 3 à 1 case occupée, sans dépasser
        // `cube_timeout`.
        let deadline = Instant::now() + timing.cube_timeout;
        let cube = loop {
            let cube = read_cube_live(platform, window, profile, timing)?;
            let count = cube.occupied.iter().filter(|occupied| **occupied).count();
            if count != 3 || Instant::now() >= deadline {
                break cube;
            }
            sleep(timing.poll);
        };
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
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    col: usize,
    row: usize,
    target_before: u32,
    timing: Timing,
) -> Result<()> {
    let expected = target_before.saturating_add(1);

    for attempt in 1..=3 {
        check_focus(platform, window)?;
        check_emergency(platform)?;
        check_tab_live(platform, window, profile)?;

        let cube = read_cube_live(platform, window, profile, timing)?;
        let Some(result_slot) = cube.occupied.iter().position(|occupied| *occupied) else {
            if wait_cell(
                platform, window, profile, templates, col, row, expected, timing,
            )?
            .is_some()
            {
                return Ok(());
            }
            let image = platform.capture(window)?;
            let reading = table::read_cell(&image, profile, templates, col, row, false)?;
            if reading.best_candidate == Some(planner::MAX_STACK as u16) {
                logger.log(full_case_message(col, row));
                bail!(
                    "la case cible est pleine ({}); la gemme produite n'a pas pu y être réinsérée",
                    planner::MAX_STACK
                );
            }
            bail!(
                "le cube est vide mais la quantité cible n'a pas augmenté \
                 (lu: {:?}, score {:.3}, attendu {expected}); \
                 vérifie l'inventaire, une gemme a peut-être été déplacée",
                reading.best_candidate,
                reading.score
            );
        };

        let result_point = cube_slot_point(window, profile, result_slot);
        platform.click(result_point, MouseButton::Left, Modifiers::CTRL_SHIFT)?;

        if let Some(reading) = wait_cell(
            platform, window, profile, templates, col, row, expected, timing,
        )? {
            if !reading.is_confident() {
                logger.log(format!(
                    "  cible acceptée avec marge faible: {:?} score {:.3} marge {:.3}",
                    reading.best_candidate, reading.score, reading.margin
                ));
            }
            let cube = read_cube_live(platform, window, profile, timing)?;
            if cube.occupied.iter().any(|occupied| *occupied) {
                bail!("le cube n'est pas vide après la réinscription de la gemme");
            }
            return Ok(());
        }

        logger.log(format!(
            "  réinsertion tentative {attempt} sans effet, reprise"
        ));
    }

    if target_full(platform, window, profile, templates, col, row)? {
        logger.log(full_case_message(col, row));
        bail!(
            "la case cible est pleine ({}); la gemme reste dans le cube",
            planner::MAX_STACK
        );
    }

    bail!("impossible de réinsérer la gemme dans la table après 3 tentatives")
}

/// Vrai si la case cible a atteint la capacité maximale d'une pile.
fn target_full(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    col: usize,
    row: usize,
) -> Result<bool> {
    let image = platform.capture(window)?;
    let reading = table::read_cell(&image, profile, templates, col, row, false)?;
    Ok(reading.best_candidate == Some(planner::MAX_STACK as u16))
}

fn full_case_message(col: usize, row: usize) -> String {
    format!(
        "Case pleine: {} / {} = {}. Une gemme transmutée ne peut pas y être \
         réinsérée; vide cette case ou utilise les gemmes avant de relancer.",
        GEM_TYPES[col],
        TIERS[row],
        planner::MAX_STACK
    )
}

/// Signale sur la sortie standard les cases pleines qui bloquent le plan.
fn report_full_receivers(
    logger: &mut Logger,
    counts: &planner::Counts,
    keep: &planner::Keep,
    start_tier: usize,
) {
    for (gem_type, target_tier) in planner::full_receivers(counts, keep, start_tier) {
        logger.log(full_case_message(gem_type, target_tier));
    }
}

/// Attend que la cellule atteigne la valeur attendue. Retourne `None` si le
/// délai expire sans erreur de sécurité.
fn wait_cell(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    col: usize,
    row: usize,
    expected: u32,
    timing: Timing,
) -> Result<Option<CountReading>> {
    let expected_u16 = u16::try_from(expected)
        .map_err(|_| anyhow!("valeur attendue trop grande pour une cellule: {expected}"))?;
    let deadline = Instant::now() + timing.cell_timeout;

    // Le curseur reste au point de repos pendant toute l'attente: on le place
    // une seule fois, puis on relit la cellule jusqu'à la valeur attendue.
    park(platform, window, profile, timing)?;
    loop {
        check_emergency(platform)?;
        let image = platform.capture(window)?;
        let reading = table::read_cell(&image, profile, templates, col, row, false)?;
        if expected_accepted(&reading, expected_u16) {
            return Ok(Some(reading));
        }

        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(timing.poll);
    }
}

/// Vérification d'action : la valeur attendue est connue. Si elle est le
/// meilleur candidat avec un score élevé, une marge un peu faible est
/// acceptable; c'est le cas typique d'un chiffre à 36 lu à 0.78 de score mais
/// à 0.014 de marge.
fn expected_accepted(reading: &CountReading, expected: u16) -> bool {
    reading.best_candidate == Some(expected) && reading.score >= 0.70
}

fn read_cube_live(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    timing: Timing,
) -> Result<CubeReading> {
    park(platform, window, profile, timing)?;
    let image = platform.capture(window)?;
    cube::read_cube(&image, profile)
}

/// Déplace le curseur au point de repos. Avant de le déplacer, vérifie que
/// l'utilisateur n'a pas bougé la souris depuis la dernière action du
/// programme. C'est le failsafe de souris, en plus de F9.
fn park(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    timing: Timing,
) -> Result<()> {
    if platform.pointer_moved_by_user()? {
        return Err(safety("mouvement de souris détecté: arrêt"));
    }

    let park_at = park_point(window, profile);
    platform.move_pointer(park_at)?;
    sleep(timing.park_settle);
    check_emergency(platform)?;
    platform.note_pointer()?;
    Ok(())
}

fn check_focus(platform: &impl Platform, window: &WindowInfo) -> Result<()> {
    if !platform.is_focused(window)? {
        return Err(safety("le jeu a perdu le focus"));
    }
    Ok(())
}

fn check_emergency(platform: &impl Platform) -> Result<()> {
    if crate::hotkeys::stop_requested() {
        return Err(safety("arrêt d'urgence demandé (F9/Ctrl+C terminal)"));
    }
    if platform.emergency_requested()? {
        return Err(safety("arrêt d'urgence demandé (F9)"));
    }
    Ok(())
}

fn check_tab_live(platform: &impl Platform, window: &WindowInfo, profile: &Profile) -> Result<()> {
    let image = platform.capture(window)?;
    if !crate::vision::tabs::gemmes_active(&image, profile)? {
        return Err(safety("l'onglet Gemmes n'est plus actif"));
    }
    Ok(())
}

/// Relit les 35 cellules et compare l'état réel au modèle. Une différence
/// arrête le run avant de continuer sur un état incohérent.
fn audit_table(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    expected: &planner::Counts,
    step: usize,
    timing: Timing,
) -> Result<()> {
    park(platform, window, profile, timing)?;
    let image = platform.capture(window)?;
    let table = table::read_table(&image, profile, templates, false)
        .map_err(|error| safety(format!("audit étape {step}: lecture impossible: {error:#}")))?;

    if !table.tab_active {
        return Err(safety(format!("audit étape {step}: onglet Gemmes inactif")));
    }
    if !table.detected {
        return Err(safety(format!("audit étape {step}: table non détectée")));
    }
    if !table.uncertain.is_empty() {
        return Err(safety(format!(
            "audit étape {step}: {} cellule(s) incertaine(s)",
            table.uncertain.len()
        )));
    }

    let actual = crate::counts_from_table(&table)
        .map_err(|error| safety(format!("audit étape {step}: {error:#}")))?;
    let differences = compare_counts(expected, &actual);
    if differences.is_empty() {
        logger.log(format!("  audit étape {step}: ok"));
        return Ok(());
    }

    logger.log(format!(
        "  audit étape {step}: {} différence(s) avec le modèle",
        differences.len()
    ));
    for difference in &differences {
        logger.log(format!("    {difference}"));
    }
    Err(safety(format!(
        "audit étape {step}: la table a changé de façon inattendue"
    )))
}

fn compare_counts(expected: &planner::Counts, actual: &planner::Counts) -> Vec<String> {
    let mut differences = Vec::new();
    for gem_type in 0..7 {
        for tier in 0..5 {
            if expected[gem_type][tier] != actual[gem_type][tier] {
                differences.push(format!(
                    "{} / {}: modèle {}, table {}",
                    GEM_TYPES[gem_type],
                    TIERS[tier],
                    expected[gem_type][tier],
                    actual[gem_type][tier]
                ));
            }
        }
    }
    differences
}

fn wait_for_resume(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    logger: &mut Logger,
    counts: &mut planner::Counts,
    timing: Timing,
) -> Result<()> {
    logger.log("Pause: F8 pour reprendre, F9/Ctrl+C pour arrêter.");
    loop {
        if crate::hotkeys::stop_requested() || platform.emergency_requested()? {
            return Err(safety("arrêt d'urgence demandé (F9/Ctrl+C)"));
        }
        if platform.take_pause() || crate::hotkeys::take_pause() {
            logger.log("Reprise demandée; relecture de la table et du cube...");
            match reload_state(platform, window, profile, templates, timing) {
                Ok(new_counts) => {
                    *counts = new_counts;
                    let _ = platform.take_pause();
                    let _ = crate::hotkeys::take_pause();
                    logger.log(format!(
                        "Reprise: total table {}",
                        counts.iter().flatten().sum::<u32>()
                    ));
                    return Ok(());
                }
                Err(error) if error.downcast_ref::<SafetyStop>().is_some() => {
                    return Err(error);
                }
                Err(error) => {
                    logger.log(format!(
                        "Reprise impossible: {error:#}; corrige l'état puis presse F8."
                    ));
                }
            }
        }
        sleep(Duration::from_millis(200));
    }
}

fn reload_state(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    templates: &TemplateSet,
    timing: Timing,
) -> Result<planner::Counts> {
    platform.focus(window)?;
    sleep(timing.after_focus);
    if !platform.is_focused(window)? {
        bail!("le jeu n'a pas le focus");
    }
    park_for_resume(platform, window, profile, timing)?;
    let image = platform.capture(window)?;
    let table = table::read_table(&image, profile, templates, false)
        .map_err(|error| anyhow!("relecture de la table impossible: {error:#}"))?;

    if !table.tab_active {
        bail!("l'onglet Gemmes n'est pas actif");
    }
    if !table.detected {
        bail!("la table n'est pas détectée");
    }
    if !table.uncertain.is_empty() {
        bail!("{} cellule(s) incertaine(s)", table.uncertain.len());
    }

    let cube = cube::read_cube(&image, profile)?;
    if cube.occupied.iter().any(|occupied| *occupied) {
        bail!("le cube doit être vide avant de reprendre");
    }

    crate::counts_from_table(&table)
}

fn park_for_resume(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    timing: Timing,
) -> Result<()> {
    let park_at = park_point(window, profile);
    platform.move_pointer(park_at)?;
    sleep(timing.park_settle);
    check_emergency(platform)?;
    platform.note_pointer()?;
    Ok(())
}

fn notify(title: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .arg(title)
        .arg(body)
        .spawn();
}

fn recover_cube(
    platform: &impl Platform,
    window: &WindowInfo,
    profile: &Profile,
    logger: &mut Logger,
    timing: Timing,
) -> Result<()> {
    for attempt in 1..=3 {
        let cube = read_cube_live(platform, window, profile, timing)?;
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
            check_focus(platform, window)?;
            check_emergency(platform)?;
            let point = cube_slot_point(window, profile, index);
            platform.click(point, MouseButton::Left, Modifiers::CTRL_SHIFT)?;
            sleep(timing.retry_pause);
        }
    }

    let cube = read_cube_live(platform, window, profile, timing)?;
    if cube.occupied.iter().any(|occupied| *occupied) {
        bail!("le cube n'a pas pu être vidé");
    }
    Ok(())
}

fn save_error_frame(platform: &impl Platform, window: &WindowInfo, logger: &mut Logger) {
    let path = crate::logging::run_dir().join(format!("error-{}.png", epoch_seconds()));
    match platform.capture(window) {
        Ok(image) => match image.save(&path) {
            Ok(()) => logger.log(format!("Capture d'erreur: {}", path.display())),
            Err(error) => logger.log(format!("Échec de sauvegarde de la capture: {error}")),
        },
        Err(error) => logger.log(format!("Échec de capture d'erreur: {error:#}")),
    }
}

fn cell_click_point(window: &WindowInfo, profile: &Profile, col: usize, row: usize) -> Point {
    Point {
        x: window.origin.x
            + profile.panel.x
            + profile.table.count_right_edges[col]
            + profile.click.source_dx,
        y: window.origin.y
            + profile.panel.y
            + profile.table.count_tops[row]
            + profile.click.source_dy,
    }
}

fn transmute_point(window: &WindowInfo, profile: &Profile) -> Point {
    Point {
        x: window.origin.x + profile.panel.x + profile.click.transmute_x,
        y: window.origin.y + profile.panel.y + profile.click.transmute_y,
    }
}

fn cube_slot_point(window: &WindowInfo, profile: &Profile, index: usize) -> Point {
    let slot = profile.cube.slots[index];
    Point {
        x: window.origin.x + profile.panel.x + slot.x + slot.width as i32 / 2,
        y: window.origin.y + profile.panel.y + slot.y + slot.height as i32 / 2,
    }
}

fn park_point(window: &WindowInfo, profile: &Profile) -> Point {
    Point {
        x: window.origin.x + profile.panel.x + profile.click.park_x,
        y: window.origin.y + profile.panel.y + profile.click.park_y,
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_counts, expected_accepted};
    use crate::vision::digits::CountReading;

    fn reading(best: u16, score: f32, margin: f32) -> CountReading {
        CountReading {
            number: None,
            best_candidate: Some(best),
            score,
            margin,
            bbox: None,
            kept_components: 2,
            actual_pixels: 88,
            top_candidates: vec![(best, score)],
            window: 22,
            note: None,
            corroborated: false,
        }
    }

    #[test]
    fn accepts_expected_value_with_low_margin_but_high_score() {
        let high = reading(36, 0.782, 0.014);
        assert!(expected_accepted(&high, 36));
        assert!(!expected_accepted(&high, 35));

        // Cas Émeraude/Parfait observé en live.
        let borderline = reading(36, 0.737, 0.062);
        assert!(expected_accepted(&borderline, 36));
    }

    #[test]
    fn rejects_expected_value_with_low_score() {
        let reading = reading(36, 0.65, 0.05);
        assert!(!expected_accepted(&reading, 36));
    }

    #[test]
    fn audit_compare_accepts_identical_tables() {
        let counts = [[0u32; 5]; 7];
        assert!(compare_counts(&counts, &counts).is_empty());
    }

    #[test]
    fn audit_compare_detects_a_changed_cell() {
        let expected = [[0u32; 5]; 7];
        let mut actual = expected;
        actual[2][3] = 20;

        let differences = compare_counts(&expected, &actual);
        assert_eq!(differences.len(), 1);
        assert!(differences[0].contains("Rubis"));
        assert!(differences[0].contains("Sans défaut"));
        assert!(differences[0].contains("20"));
    }
}
