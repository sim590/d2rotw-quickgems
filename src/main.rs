mod config;
mod executor;
mod hotkeys;
mod logging;
mod planner;
mod platform;
mod profile;
mod vision;

use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use image::RgbImage;

use config::Config;
use profile::Profile;
use vision::digits::TemplateSet;
use vision::table::{self, GEM_TYPES, TIERS};

#[derive(Parser)]
#[command(
    name = "quickgems",
    version,
    about = "Lecteur et automate de gemmes pour Diablo II: Resurrected"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lit une capture d'écran et affiche le tableau des gemmes.
    Inspect {
        /// Capture d'écran du jeu (PNG), par exemple 5120x1440 avec le jeu à gauche.
        #[arg(long)]
        image: PathBuf,
        /// Profil de disposition (TOML).
        #[arg(long, default_value = "profiles/2560x1440.toml")]
        profile: PathBuf,
        /// Affiche les détails de lecture par cellule.
        #[arg(long)]
        debug: bool,
    },
    /// Lit la capture et calcule le plan de transmutations.
    Plan {
        /// Capture d'écran du jeu (PNG), par exemple 5120x1440 avec le jeu à gauche.
        #[arg(long)]
        image: PathBuf,
        /// Profil de disposition (TOML).
        #[arg(long, default_value = "profiles/2560x1440.toml")]
        profile: PathBuf,
        /// Prescription des quantités minimales à conserver.
        #[arg(long, default_value = "quickgems.toml")]
        config: PathBuf,
        /// Limite la projection à N transmutations (budget).
        #[arg(long)]
        max_transmutations: Option<usize>,
        /// Palier source minimal (0=abîmé, 1=imparfait, 2=normal, 3=sans défaut).
        #[arg(
            long,
            default_value_t = 0,
            value_parser = parse_start_tier,
            visible_alias = "depuis-palier"
        )]
        start_tier: usize,
        /// Affiche les détails de lecture par cellule.
        #[arg(long)]
        debug: bool,
    },
    /// Capture la fenêtre du jeu dans un PNG (diagnostic, aucun clic).
    Capture {
        /// Fichier PNG de sortie.
        #[arg(long)]
        output: PathBuf,
        /// Profil de disposition (TOML).
        #[arg(long, default_value = "profiles/2560x1440.toml")]
        profile: PathBuf,
    },
    /// Lit le jeu en direct et exécute les transmutations (simulation par défaut).
    Run {
        /// Utilise une capture PNG au lieu de la fenêtre X11 (simulation seulement).
        #[arg(long)]
        image: Option<PathBuf>,
        /// Profil de disposition (TOML).
        #[arg(long, default_value = "profiles/2560x1440.toml")]
        profile: PathBuf,
        /// Prescription des quantités minimales à conserver.
        #[arg(long, default_value = "quickgems.toml")]
        config: PathBuf,
        /// Fait réellement les clics. Sans ce drapeau, le programme simule.
        #[arg(long)]
        execute: bool,
        /// Nombre maximal de transmutations (défaut: 1).
        #[arg(long)]
        max_transmutations: Option<usize>,
        /// Exécute jusqu'à ce qu'il n'y ait plus d'action valide.
        #[arg(long)]
        until_done: bool,
        /// Palier source minimal (0=abîmé, 1=imparfait, 2=normal, 3=sans défaut).
        #[arg(
            long,
            default_value_t = 0,
            value_parser = parse_start_tier,
            visible_alias = "depuis-palier"
        )]
        start_tier: usize,
        /// Relit les 35 cellules toutes les N actions (0 désactive l'audit).
        #[arg(long, default_value_t = 5)]
        audit_every: usize,
        /// Affiche les détails de lecture par cellule.
        #[arg(long)]
        debug: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            image,
            profile,
            debug,
        } => inspect(&image, &profile, debug),
        Command::Plan {
            image,
            profile,
            config,
            max_transmutations,
            start_tier,
            debug,
        } => plan(
            &image,
            &profile,
            &config,
            max_transmutations,
            start_tier,
            debug,
        ),
        Command::Capture { output, profile } => capture_live(&output, &profile),
        Command::Run {
            image,
            profile,
            config,
            execute,
            max_transmutations,
            until_done,
            start_tier,
            audit_every,
            debug,
        } => run(
            image.as_deref(),
            &profile,
            &config,
            execute,
            max_transmutations,
            until_done,
            start_tier,
            audit_every,
            debug,
        ),
    }
}

fn resolve_run_limit(until_done: bool, max_transmutations: Option<usize>) -> Result<Option<usize>> {
    match (until_done, max_transmutations) {
        (true, Some(_)) => bail!("--until-done et --max-transmutations sont incompatibles"),
        (true, None) => Ok(None),
        (false, Some(max)) => Ok(Some(max)),
        (false, None) => Ok(Some(1)),
    }
}

fn parse_start_tier(value: &str) -> std::result::Result<usize, String> {
    let tier: usize = value
        .parse()
        .map_err(|_| "palier invalide; utilise un nombre de 0 à 3".to_string())?;
    if tier > 3 {
        return Err(
            "le palier source doit être entre 0 (abîmé) et 3 (sans défaut); \
             le parfait ne peut pas être converti"
                .to_string(),
        );
    }
    Ok(tier)
}

fn capture_live(output: &Path, profile_path: &Path) -> Result<()> {
    let profile = Profile::load(profile_path)?;
    let x11 = platform::x11::X11::new()?;
    let window = x11.find_game_window()?;
    println!(
        "Fenêtre: {} ({}x{}) @ ({}, {})",
        window.title, window.width, window.height, window.x, window.y
    );
    if window.width != profile.image_width || window.height != profile.image_height {
        bail!(
            "la fenêtre du jeu fait {}x{}, mais le profil attend {}x{}",
            window.width,
            window.height,
            profile.image_width,
            profile.image_height
        );
    }

    x11.focus_window(&window)?;
    sleep(Duration::from_millis(200));
    let image = x11.capture_window(&window)?;
    image
        .save(output)
        .with_context(|| format!("impossible d'écrire la capture {}", output.display()))?;
    println!("Capture écrite: {}", output.display());
    Ok(())
}

fn run(
    image_path: Option<&Path>,
    profile_path: &Path,
    config_path: &Path,
    execute: bool,
    max_transmutations: Option<usize>,
    until_done: bool,
    start_tier: usize,
    audit_every: usize,
    debug: bool,
) -> Result<()> {
    let profile = Profile::load(profile_path)?;
    let config = Config::load(config_path)?;
    let templates = TemplateSet::load()?;
    let limit = resolve_run_limit(until_done, max_transmutations)?;
    let options = executor::RunOptions {
        execute,
        max_transmutations: limit,
        start_tier,
        audit_every,
        debug,
    };

    match image_path {
        Some(path) => executor::run_from_image(path, &profile, &config, &templates, options),
        None => executor::run_live(&profile, &config, &templates, options),
    }
}

fn inspect(image_path: &Path, profile_path: &Path, debug: bool) -> Result<()> {
    let profile = Profile::load(profile_path)?;
    let game = load_game_image(image_path, &profile)?;
    let templates = TemplateSet::load()?;
    let table = table::read_table(&game, &profile, &templates, debug)?;

    println!(
        "Table des gemmes ({}x{})",
        profile.image_width, profile.image_height
    );
    print_table(&table.cells);
    println!("\nTotal connu: {}", table.known_total());
    print_uncertain(&table);

    let cube = vision::cube::read_cube(&game, &profile)?;
    print_cube(&cube);

    if debug {
        print_table_details(&table);
        println!("\nDétails du cube:");
        for index in 0..12 {
            println!(
                "  case {index:2}: max={:.1} écart-type={:.1}",
                cube.max_luma[index], cube.std_dev[index]
            );
        }
    }

    Ok(())
}

fn plan(
    image_path: &Path,
    profile_path: &Path,
    config_path: &Path,
    max_transmutations: Option<usize>,
    start_tier: usize,
    debug: bool,
) -> Result<()> {
    let profile = Profile::load(profile_path)?;
    let game = load_game_image(image_path, &profile)?;
    let templates = TemplateSet::load()?;
    let table = table::read_table(&game, &profile, &templates, debug)?;

    println!(
        "Table actuelle ({}x{})",
        profile.image_width, profile.image_height
    );
    print_table(&table.cells);
    print_uncertain(&table);

    if !table.tab_active {
        bail!("onglet Gemmes non actif: sélectionne l'onglet Gemmes");
    }
    if !table.detected {
        bail!("table non détectée: ouvre le cache et l'onglet Gemmes");
    }
    if !table.uncertain.is_empty() {
        bail!("impossible de planifier: la lecture du tableau n'est pas fiable");
    }

    let config = Config::load(config_path)?;
    let keep = config.keep_matrix()?;
    let counts = counts_from_table(&table)?;
    let plan = match max_transmutations {
        Some(max) => planner::project_limited(counts, &keep, max, start_tier),
        None => planner::project(counts, &keep, start_tier),
    };
    let conversions = plan.conversion_count();
    let source_gems = conversions.saturating_mul(3);
    let produced_gems = conversions;
    let net_loss = conversions.saturating_mul(2);

    println!("\nLimites keep (minimum à conserver):");
    print_keep(&keep);

    println!("\nDépart: {} ({start_tier})", TIERS[start_tier]);
    println!("\nPlan: {conversions} transmutations");
    if let Some(max) = max_transmutations {
        println!("(Projection limitée à {max} actions.)");
    }
    println!(
        "Budget: {source_gems} gemmes sources consommées, \
         {produced_gems} gemmes produites, net -{net_loss}."
    );
    println!("Coût par type:");
    for gem_type in 0..7 {
        let count: u32 = plan.conversions[gem_type].iter().sum();
        if count > 0 {
            let suffix = if count > 1 { "s" } else { "" };
            println!(
                "  {}: {} sources -> {} gemme{}",
                GEM_TYPES[gem_type],
                count.saturating_mul(3),
                count,
                suffix
            );
        }
    }
    for gem_type in 0..7 {
        let mut parts = Vec::new();
        for tier in 0..4 {
            let count = plan.conversions[gem_type][tier];
            if count > 0 {
                parts.push(format!("{} -> {} x{}", TIERS[tier], TIERS[tier + 1], count));
            }
        }
        if !parts.is_empty() {
            println!("  {}: {}", GEM_TYPES[gem_type], parts.join(", "));
        }
    }

    println!("\nParfaites gagnées: {}", plan.total_perfect_gain());
    println!("\nTable projetée:");
    print_counts(&plan.final_counts);

    let full = planner::full_receivers(&counts, &keep, start_tier);
    if !full.is_empty() {
        println!("\nCases pleines ({}):", planner::MAX_STACK);
        for (gem_type, target_tier) in full {
            println!(
                "  {} / {}: la gemme produite ne peut pas y être réinsérée; \
                 vide cette case ou utilise les gemmes avant de relancer.",
                GEM_TYPES[gem_type], TIERS[target_tier]
            );
        }
    }

    if debug {
        print_table_details(&table);
    }

    Ok(())
}

pub(crate) fn load_game_image(image_path: &Path, profile: &Profile) -> Result<RgbImage> {
    let original = image::open(image_path)
        .with_context(|| format!("impossible d'ouvrir l'image {}", image_path.display()))?
        .to_rgb8();

    if original.width() < profile.image_width || original.height() < profile.image_height {
        bail!(
            "l'image est plus petite que le profil ({}x{}, attendu au moins {}x{})",
            original.width(),
            original.height(),
            profile.image_width,
            profile.image_height
        );
    }

    Ok(
        image::imageops::crop_imm(&original, 0, 0, profile.image_width, profile.image_height)
            .to_image(),
    )
}

pub(crate) fn counts_from_table(table: &table::TableReading) -> Result<planner::Counts> {
    let mut counts = [[0u32; 5]; 7];
    for col in 0..7 {
        for row in 0..5 {
            let value = table.cells[col][row].ok_or_else(|| {
                anyhow!("cellule incertaine: {} / {}", GEM_TYPES[col], TIERS[row])
            })?;
            counts[col][row] = u32::from(value);
        }
    }
    Ok(counts)
}

fn print_header() {
    println!(
        "{:<10} {:>6} {:>10} {:>7} {:>12} {:>9}",
        "Type", "Abîmé", "Imparfait", "Normal", "Sans défaut", "Parfait"
    );
}

fn print_table(cells: &[[Option<u16>; 5]; 7]) {
    print_header();
    for col in 0..7 {
        let values = (0..5)
            .map(|row| cell_text(cells[col][row], column_width(row)))
            .collect::<Vec<_>>();
        println!(
            "{:<10} {} {} {} {} {}",
            GEM_TYPES[col], values[0], values[1], values[2], values[3], values[4]
        );
    }
}

fn print_counts(counts: &planner::Counts) {
    print_header();
    for col in 0..7 {
        println!(
            "{:<10} {:>6} {:>10} {:>7} {:>12} {:>9}",
            GEM_TYPES[col],
            counts[col][0],
            counts[col][1],
            counts[col][2],
            counts[col][3],
            counts[col][4]
        );
    }
}

fn print_keep(keep: &planner::Keep) {
    print_header();
    for col in 0..7 {
        println!(
            "{:<10} {:>6} {:>10} {:>7} {:>12} {:>9}",
            GEM_TYPES[col], keep[col][0], keep[col][1], keep[col][2], keep[col][3], keep[col][4]
        );
    }
}

fn print_uncertain(table: &table::TableReading) {
    if !table.tab_active {
        println!("Onglet Gemmes non actif: sélectionne l'onglet Gemmes.");
        return;
    }
    if !table.detected {
        println!("Table non détectée: ouvre le cache et l'onglet Gemmes.");
        return;
    }
    if table.uncertain.is_empty() {
        println!("Toutes les cellules ont été lues avec confiance.");
    } else {
        println!("Cellules incertaines:");
        for (col, row) in &table.uncertain {
            println!(
                "  {} / {} (score {:.3})",
                GEM_TYPES[*col], TIERS[*row], table.scores[*col][*row]
            );
        }
    }
}

fn print_table_details(table: &table::TableReading) {
    println!("\nDétails de lecture:");
    for detail in &table.details {
        println!("{detail}");
    }
}

fn print_cube(cube: &vision::cube::CubeReading) {
    println!("\nCube horadrique (E=vide, X=occupé):");
    for row in 0..4 {
        let mut line = String::new();
        for col in 0..3 {
            let index = row * 3 + col;
            line.push_str(if cube.occupied[index] { "X " } else { "E " });
        }
        println!("{}", line.trim_end());
    }
}

fn column_width(row: usize) -> usize {
    match row {
        0 => 6,
        1 => 10,
        2 => 7,
        3 => 12,
        _ => 9,
    }
}

fn cell_text(value: Option<u16>, width: usize) -> String {
    match value {
        Some(value) => format!("{value:>width$}"),
        None => format!("{:>width$}", "?"),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_run_limit;

    #[test]
    fn run_limit_defaults_and_until_done() {
        assert_eq!(resolve_run_limit(false, None).expect("défaut"), Some(1));
        assert_eq!(resolve_run_limit(false, Some(3)).expect("limite"), Some(3));
        assert_eq!(
            resolve_run_limit(true, None).expect("jusqu'à épuisement"),
            None
        );
        assert!(resolve_run_limit(true, Some(3)).is_err());
    }
}
