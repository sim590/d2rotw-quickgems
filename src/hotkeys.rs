//! Raccourcis lus depuis le terminal.
//!
//! Sur Wayland, un grab X11 ne reçoit pas les touches quand le terminal natif
//! a le focus. On lit donc aussi F8/F9/Ctrl+C sur stdin en mode brut. Quand le
//! jeu XWayland a le focus, le grab X11 de `platform::x11` prend le relais.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

static PAUSE: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Démarre l'écoute du terminal si stdin est un TTY. Le guard restaure le mode
/// normal du terminal à la sortie.
pub fn start_terminal() -> TerminalGuard {
    if !std::io::stdin().is_terminal() {
        return TerminalGuard;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return TerminalGuard;
    }

    std::thread::spawn(|| {
        let _ = crossterm::terminal::enable_raw_mode();
        while RUNNING.load(Ordering::SeqCst) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    if let Ok(Event::Key(key)) = event::read() {
                        if key.kind == KeyEventKind::Press {
                            match key.code {
                                KeyCode::F(8) => {
                                    PAUSE.store(true, Ordering::SeqCst);
                                }
                                KeyCode::F(9) => {
                                    STOP.store(true, Ordering::SeqCst);
                                }
                                KeyCode::Char('c')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    STOP.store(true, Ordering::SeqCst);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(false) => {}
                Err(_) => break,
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
    });

    TerminalGuard
}

pub fn take_pause() -> bool {
    PAUSE.swap(false, Ordering::SeqCst)
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}
