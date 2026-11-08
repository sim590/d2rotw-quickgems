mod profile;
mod vision;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            image,
            profile,
            debug,
        } => inspect(&image, &profile, debug),
    }
}

fn inspect(image_path: &Path, profile_path: &Path, debug: bool) -> Result<()> {
    let profile = Profile::load(profile_path)?;
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

    let game =
        image::imageops::crop_imm(&original, 0, 0, profile.image_width, profile.image_height)
            .to_image();

    let templates = TemplateSet::load()?;
    let table = table::read_table(&game, &profile, &templates, debug)?;

    println!(
        "Table des gemmes ({}x{})",
        profile.image_width, profile.image_height
    );
    println!(
        "{:<10} {:>6} {:>10} {:>7} {:>12} {:>9}",
        "Type", "Abîmé", "Imparfait", "Normal", "Sans défaut", "Parfait"
    );

    for col in 0..7 {
        let cells = (0..5)
            .map(|row| cell_text(table.cells[col][row], column_width(row)))
            .collect::<Vec<_>>();
        println!(
            "{:<10} {} {} {} {} {}",
            GEM_TYPES[col], cells[0], cells[1], cells[2], cells[3], cells[4]
        );
    }

    println!("\nTotal connu: {}", table.known_total());
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

    let cube = vision::cube::read_cube(&game, &profile)?;
    println!("\nCube horadrique (E=vide, X=occupé):");
    for row in 0..4 {
        let mut line = String::new();
        for col in 0..3 {
            let index = row * 3 + col;
            line.push_str(if cube.occupied[index] { "X " } else { "E " });
        }
        println!("{}", line.trim_end());
    }

    if debug {
        println!("\nDétails de lecture:");
        for detail in &table.details {
            println!("{detail}");
        }
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
