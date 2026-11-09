mod config;
mod config_paths;
mod executor;
mod hotkeys;
mod logging;
mod planner;
mod platform;
mod profile;
mod vision;

use std::io::{ErrorKind, Write};
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
enum ConfigCommand {
    /// Crée le fichier de configuration utilisateur à partir du contenu intégré.
    Generate,
}

#[derive(Subcommand)]
enum Command {
    /// Gère la configuration utilisateur.
    Config {
        /// Sous-commande de gestion de la configuration.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Lit une capture d'écran et affiche le tableau des gemmes.
    Inspect {
        /// Capture d'écran du jeu (PNG), par exemple 5120x1440 avec le jeu à gauche.
        #[arg(long)]
        image: PathBuf,
        /// Profil de disposition (TOML); profil intégré si absent.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Affiche les détails de lecture par cellule.
        #[arg(long)]
        debug: bool,
    },
    /// Lit la capture et calcule le plan de transmutations.
    Plan {
        /// Capture d'écran du jeu (PNG), par exemple 5120x1440 avec le jeu à gauche.
        #[arg(long)]
        image: PathBuf,
        /// Profil de disposition (TOML); profil intégré si absent.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Prescription des quantités minimales à conserver; priorité sur la
        /// configuration utilisateur (XDG) et sur la configuration intégrée.
        #[arg(long)]
        config: Option<PathBuf>,
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
        /// Profil de disposition (TOML); profil intégré si absent.
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    /// Lit le jeu en direct et exécute les transmutations (simulation par défaut).
    Run {
        /// Utilise une capture PNG au lieu de la fenêtre X11 (simulation seulement).
        #[arg(long)]
        image: Option<PathBuf>,
        /// Profil de disposition (TOML); profil intégré si absent.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Prescription des quantités minimales à conserver; priorité sur la
        /// configuration utilisateur (XDG) et sur la configuration intégrée.
        #[arg(long)]
        config: Option<PathBuf>,
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
        Command::Config { command } => match command {
            ConfigCommand::Generate => generate_initial_config(&config_paths::NativePaths),
        },
        Command::Inspect {
            image,
            profile,
            debug,
        } => inspect(&image, profile.as_deref(), debug),
        Command::Plan {
            image,
            profile,
            config,
            max_transmutations,
            start_tier,
            debug,
        } => plan(
            &image,
            profile.as_deref(),
            config.as_deref(),
            max_transmutations,
            start_tier,
            debug,
        ),
        Command::Capture { output, profile } => capture_live(&output, profile.as_deref()),
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
            profile.as_deref(),
            config.as_deref(),
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

const BUILTIN_PROFILE: &str = include_str!("../profiles/2560x1440.toml");
const BUILTIN_CONFIG: &str = include_str!("../quickgems.toml");

/// Charge le profil demandé. Sans argument, le profil intégré au binaire est
/// utilisé; un chemin explicite le remplace.
fn resolve_profile(path: Option<&Path>) -> Result<Profile> {
    match path {
        Some(path) => Profile::load(path),
        None => Profile::from_toml(BUILTIN_PROFILE, "profil intégré 2560x1440"),
    }
}

/// Crée le fichier de configuration utilisateur à partir du contenu intégré.
///
/// Contrairement à `resolve_config`, cette commande ne se replie jamais
/// silencieusement sur la configuration intégrée : si le fichier ne peut pas
/// être créé, c'est une erreur.
fn generate_initial_config(locator: &impl config_paths::ConfigLocator) -> Result<()> {
    let Some(path) = locator.config_paths().into_iter().next() else {
        bail!("aucun chemin de configuration utilisateur pour cette plateforme");
    };

    if path.exists() {
        println!(
            "Configuration utilisateur déjà présente: {}",
            path.display()
        );
        return Ok(());
    }

    create_user_config(&path).with_context(|| format!("impossible de créer {}", path.display()))?;
    Config::load(&path).context("le fichier de configuration créé est invalide")?;
    println!("Configuration utilisateur créée: {}", path.display());
    Ok(())
}

/// Charge la configuration demandée. Priorité :
/// 1. chemin explicite (`--config`);
/// 2. fichier utilisateur de la plateforme, créé au besoin à partir du
///    contenu intégré;
/// 3. configuration intégrée au binaire.
fn resolve_config(
    path: Option<&Path>,
    locator: &impl config_paths::ConfigLocator,
) -> Result<Config> {
    if let Some(path) = path {
        return Config::load(path);
    }

    for candidate in locator.config_paths() {
        if candidate.exists() {
            return Config::load(&candidate);
        }

        match create_user_config(&candidate) {
            Ok(()) => {
                println!("Configuration utilisateur créée: {}", candidate.display());
                return Config::load(&candidate);
            }
            Err(error) => {
                eprintln!(
                    "Attention: impossible de créer {}: {error}; \
                     configuration intégrée utilisée.",
                    candidate.display()
                );
            }
        }
    }

    Config::from_toml(BUILTIN_CONFIG, "configuration intégrée")
}

/// Crée le fichier de configuration utilisateur à partir du contenu intégré.
/// Le fichier n'est jamais écrasé, même en cas de course entre processus.
fn create_user_config(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(BUILTIN_CONFIG.as_bytes()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn capture_live(output: &Path, profile_path: Option<&Path>) -> Result<()> {
    let profile = resolve_profile(profile_path)?;
    let x11 = platform::native()?;
    let window = x11.find_game_window()?;
    println!(
        "Fenêtre: {} ({}x{}) @ ({}, {})",
        window.title, window.size.width, window.size.height, window.origin.x, window.origin.y
    );
    if window.size.width != profile.image_width || window.size.height != profile.image_height {
        bail!(
            "la fenêtre du jeu fait {}x{}, mais le profil attend {}x{}",
            window.size.width,
            window.size.height,
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
    profile_path: Option<&Path>,
    config_path: Option<&Path>,
    execute: bool,
    max_transmutations: Option<usize>,
    until_done: bool,
    start_tier: usize,
    audit_every: usize,
    debug: bool,
) -> Result<()> {
    let profile = resolve_profile(profile_path)?;
    let config = resolve_config(config_path, &config_paths::NativePaths)?;
    let templates = TemplateSet::load()?;
    let limit = resolve_run_limit(until_done, max_transmutations)?;
    let options = executor::RunOptions {
        execute,
        max_transmutations: limit,
        start_tier,
        audit_every,
        debug,
        timing: executor::Timing::real(),
    };

    match image_path {
        Some(path) => executor::run_from_image(path, &profile, &config, &templates, options),
        None => executor::run_live(&profile, &config, &templates, options),
    }
}

fn inspect(image_path: &Path, profile_path: Option<&Path>, debug: bool) -> Result<()> {
    let profile = resolve_profile(profile_path)?;
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
    profile_path: Option<&Path>,
    config_path: Option<&Path>,
    max_transmutations: Option<usize>,
    start_tier: usize,
    debug: bool,
) -> Result<()> {
    let profile = resolve_profile(profile_path)?;
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

    let config = resolve_config(config_path, &config_paths::NativePaths)?;
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
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{resolve_config, resolve_run_limit, BUILTIN_CONFIG};
    use crate::config::Config;
    use crate::config_paths::ConfigLocator;

    struct MockLocator(Vec<PathBuf>);

    impl ConfigLocator for MockLocator {
        fn config_paths(&self) -> Vec<PathBuf> {
            self.0.clone()
        }
    }

    /// Répertoire temporaire supprimé à la fin du test.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "d2rotw-quickgems-test-dir-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).expect("répertoire temporaire");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Fichier de configuration temporaire, supprimé à la fin du test.
    struct TempConfig {
        path: PathBuf,
    }

    impl TempConfig {
        fn new(content: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "d2rotw-quickgems-test-cfg-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).expect("répertoire temporaire");
            let path = dir.join("quickgems.toml");
            std::fs::write(&path, content).expect("écriture du fichier de test");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn normal_keep(config: &Config) -> u32 {
        config.keep_matrix().expect("keep")[0][2]
    }

    fn builtin_keep() -> [[u32; 5]; 7] {
        Config::from_toml(BUILTIN_CONFIG, "configuration intégrée")
            .expect("configuration intégrée")
            .keep_matrix()
            .expect("keep")
    }

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

    #[test]
    fn explicit_config_wins_over_user_config() {
        let user = TempConfig::new("[keep.default]\nnormal = 7\n");
        let explicit = TempConfig::new("[keep.default]\nnormal = 3\n");
        let locator = MockLocator(vec![user.path().to_path_buf()]);

        let config = resolve_config(Some(explicit.path()), &locator).expect("configuration");
        assert_eq!(normal_keep(&config), 3);
    }

    #[test]
    fn user_config_wins_over_builtin() {
        let user = TempConfig::new("[keep.default]\nnormal = 7\n");
        let locator = MockLocator(vec![user.path().to_path_buf()]);

        let config = resolve_config(None, &locator).expect("configuration");
        assert_eq!(normal_keep(&config), 7);
    }

    #[test]
    fn generate_creates_the_user_config() {
        let dir = TempDir::new();
        let path = dir.path().join("d2rotw-quickgems").join("quickgems.toml");
        let locator = MockLocator(vec![path.clone()]);

        super::generate_initial_config(&locator).expect("génération");

        assert!(path.exists(), "le fichier doit être créé");
        assert_eq!(
            std::fs::read_to_string(&path).expect("lecture"),
            BUILTIN_CONFIG
        );
    }

    #[test]
    fn generate_keeps_an_existing_user_config() {
        let user = TempConfig::new("[keep.default]\nnormal = 7\n");
        let locator = MockLocator(vec![user.path().to_path_buf()]);

        super::generate_initial_config(&locator).expect("génération");

        assert_eq!(
            std::fs::read_to_string(user.path()).expect("lecture"),
            "[keep.default]\nnormal = 7\n"
        );
    }

    #[test]
    fn generate_fails_when_creation_fails() {
        let dir = TempDir::new();
        let blocker = dir.path().join("fichier");
        std::fs::write(&blocker, "x").expect("fichier bloquant");
        let locator = MockLocator(vec![blocker.join("quickgems.toml")]);

        assert!(super::generate_initial_config(&locator).is_err());
    }

    #[test]
    fn generate_fails_without_a_user_path() {
        let locator = MockLocator(Vec::new());
        assert!(super::generate_initial_config(&locator).is_err());
    }

    #[test]
    fn creates_user_config_when_missing() {
        let dir = TempDir::new();
        let path = dir.path().join("d2rotw-quickgems").join("quickgems.toml");
        let locator = MockLocator(vec![path.clone()]);

        let config = resolve_config(None, &locator).expect("configuration");

        assert!(path.exists(), "le fichier utilisateur doit être créé");
        assert_eq!(
            std::fs::read_to_string(&path).expect("lecture"),
            BUILTIN_CONFIG
        );
        assert_eq!(config.keep_matrix().expect("keep"), builtin_keep());
    }

    #[test]
    fn does_not_overwrite_an_existing_user_config() {
        let user = TempConfig::new("[keep.default]\nnormal = 7\n");
        let locator = MockLocator(vec![user.path().to_path_buf()]);

        let config = resolve_config(None, &locator).expect("configuration");

        assert_eq!(normal_keep(&config), 7);
        assert_eq!(
            std::fs::read_to_string(user.path()).expect("lecture"),
            "[keep.default]\nnormal = 7\n"
        );
    }

    #[test]
    fn falls_back_when_user_config_cannot_be_created() {
        let dir = TempDir::new();
        let blocker = dir.path().join("fichier");
        std::fs::write(&blocker, "x").expect("fichier bloquant");
        // Le parent est un fichier: la création du répertoire doit échouer.
        let locator = MockLocator(vec![blocker.join("quickgems.toml")]);

        let config = resolve_config(None, &locator).expect("configuration");
        assert_eq!(config.keep_matrix().expect("keep"), builtin_keep());
    }

    #[test]
    fn explicit_missing_config_is_an_error() {
        let locator = MockLocator(Vec::new());
        assert!(resolve_config(Some(Path::new("/nonexistent/quickgems.toml")), &locator).is_err());
    }

    #[test]
    fn invalid_user_config_is_an_error() {
        let user = TempConfig::new("ceci n'est pas du TOML");
        let locator = MockLocator(vec![user.path().to_path_buf()]);

        assert!(resolve_config(None, &locator).is_err());
    }
}
