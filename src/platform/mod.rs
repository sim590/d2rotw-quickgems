//! Contrat commun aux plateformes.
//!
//! Tout le code qui dépend du système d'exploitation vit dans ce module. Le
//! reste du programme ne connaît que les traits et les types de valeur
//! définis ici. Ajouter une plateforme consiste à écrire un module qui
//! implémente les quatre contrats, puis à l'exposer via `NativePlatform`.
//!
//! Les identifiants sont en anglais comme le reste du code; la documentation
//! reste en français.

use anyhow::Result;
use image::RgbImage;

#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(test)]
pub mod mock;

mod paths;

pub use paths::ConfigLocator;

#[cfg(target_os = "linux")]
pub use paths::NativePaths;

/// Point en coordonnées écran globales. Les moniteurs à coordonnées négatives
/// sont permis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Taille en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Identifiant opaque d'une fenêtre. `u64` couvre les identifiants X11,
/// Win32 et macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowId(pub u64);

/// Fenêtre du jeu et géométrie de sa zone cliente.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    /// Coin supérieur gauche de la zone cliente, en coordonnées écran.
    pub origin: Point,
    /// Zone cliente, en pixels physiques.
    pub size: Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

/// Modificateurs appliqués pendant un clic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        shift: false,
    };
    pub const CTRL_SHIFT: Self = Self {
        ctrl: true,
        shift: true,
    };
}

/// Découverte de la fenêtre du jeu et gestion du focus.
pub trait WindowSystem {
    /// Trouve la fenêtre du jeu. Retourne une erreur si aucune ne correspond.
    fn find_game_window(&self) -> Result<WindowInfo>;

    /// Met la fenêtre au premier plan et lui donne le focus clavier. Peut
    /// échouer (politique du gestionnaire de fenêtres ou de l'OS); l'appelant
    /// revérifie avec `is_focused`.
    fn focus(&self, window: &WindowInfo) -> Result<()>;

    /// Vrai si la fenêtre est réellement la fenêtre active.
    fn is_focused(&self, window: &WindowInfo) -> Result<bool>;
}

/// Capture de la zone cliente d'une fenêtre.
pub trait ScreenCapture {
    /// Capture la zone cliente en pixels physiques, sans le curseur, avec
    /// l'origine en haut à gauche. Doit pouvoir être appelée en boucle pour
    /// attendre une mise à jour de l'interface.
    fn capture(&self, window: &WindowInfo) -> Result<RgbImage>;
}

/// Déplacement du pointeur, clics et garde-fou du pointeur.
pub trait InputInjector {
    fn move_pointer(&self, point: Point) -> Result<()>;

    /// Clique en garantissant que `modifiers` est actif avant l'appui du
    /// bouton et le reste jusqu'après son relâchement. Le backend possède les
    /// temporisations internes du clic et, s'il le peut, la vérification de
    /// l'état des modificateurs.
    fn click(&self, point: Point, button: MouseButton, modifiers: Modifiers) -> Result<()>;

    /// Mémorise la position laissée par le programme comme référence du
    /// garde-fou.
    fn note_pointer(&self) -> Result<()>;

    /// Vrai si la souris a bougé depuis le dernier `note_pointer`.
    fn pointer_moved_by_user(&self) -> Result<bool>;
}

/// Signaux de pause et d'arrêt d'urgence lus sur la plateforme.
///
/// Ces méthodes sont interrogées par la boucle principale. Elles ne doivent
/// pas bloquer; elles peuvent toutefois pomper la file d'événements de la
/// plateforme.
pub trait ControlSignals {
    fn emergency_requested(&self) -> Result<bool>;
    fn take_pause(&self) -> bool;
}

/// Contrat complet d'une plateforme.
pub trait Platform: WindowSystem + ScreenCapture + InputInjector + ControlSignals {}

impl<T> Platform for T where T: WindowSystem + ScreenCapture + InputInjector + ControlSignals {}

/// Plateforme native de la cible de compilation.
#[cfg(target_os = "linux")]
pub use x11::X11 as NativePlatform;

/// Construit la plateforme native.
#[cfg(target_os = "linux")]
pub fn native() -> Result<NativePlatform> {
    NativePlatform::new()
}

#[cfg(not(any(target_os = "linux")))]
compile_error!(
    "aucun backend de plateforme pour cet OS; voir docs/platform.md avant d'en ajouter un"
);
