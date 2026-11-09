//! Plateforme factice pour les tests.
//!
//! Elle ne touche ni X11 ni le jeu : les images sont fournies par le test et
//! les entrées sont enregistrées en mémoire.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use anyhow::{bail, Result};
use image::RgbImage;

use super::{
    ControlSignals, InputInjector, Modifiers, MouseButton, Point, ScreenCapture, Size, WindowId,
    WindowInfo, WindowSystem,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedClick {
    pub point: Point,
    pub button: MouseButton,
    pub modifiers: Modifiers,
}

pub struct MockPlatform {
    window: WindowInfo,
    images: RefCell<VecDeque<RgbImage>>,
    last_image: RefCell<Option<RgbImage>>,
    pub clicks: RefCell<Vec<RecordedClick>>,
    pointer: Cell<Point>,
    noted_pointer: Cell<Option<Point>>,
    focused: Cell<bool>,
    emergency: Cell<bool>,
    pause: Cell<bool>,
}

impl MockPlatform {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            window: WindowInfo {
                id: WindowId(1),
                title: "jeu de test".to_string(),
                origin: Point { x: 0, y: 0 },
                size: Size { width, height },
            },
            images: RefCell::new(VecDeque::new()),
            last_image: RefCell::new(None),
            clicks: RefCell::new(Vec::new()),
            pointer: Cell::new(Point { x: 0, y: 0 }),
            noted_pointer: Cell::new(None),
            focused: Cell::new(true),
            emergency: Cell::new(false),
            pause: Cell::new(false),
        }
    }

    pub fn window(&self) -> &WindowInfo {
        &self.window
    }

    /// Ajoute une image à la file des captures. Les captures suivantes
    /// réutilisent la dernière image quand la file est vide.
    pub fn push_image(&self, image: RgbImage) {
        *self.last_image.borrow_mut() = Some(image.clone());
        self.images.borrow_mut().push_back(image);
    }

    pub fn set_pointer(&self, point: Point) {
        self.pointer.set(point);
    }

    pub fn set_focused(&self, focused: bool) {
        self.focused.set(focused);
    }

    pub fn set_emergency(&self, emergency: bool) {
        self.emergency.set(emergency);
    }

    pub fn set_pause(&self, pause: bool) {
        self.pause.set(pause);
    }

    pub fn clicks(&self) -> Vec<RecordedClick> {
        self.clicks.borrow().clone()
    }
}

impl WindowSystem for MockPlatform {
    fn find_game_window(&self) -> Result<WindowInfo> {
        Ok(self.window.clone())
    }

    fn focus(&self, _window: &WindowInfo) -> Result<()> {
        self.focused.set(true);
        Ok(())
    }

    fn is_focused(&self, _window: &WindowInfo) -> Result<bool> {
        Ok(self.focused.get())
    }
}

impl ScreenCapture for MockPlatform {
    fn capture(&self, _window: &WindowInfo) -> Result<RgbImage> {
        if let Some(image) = self.images.borrow_mut().pop_front() {
            return Ok(image);
        }
        if let Some(image) = self.last_image.borrow().as_ref() {
            return Ok(image.clone());
        }
        bail!("aucune image de test disponible")
    }
}

impl InputInjector for MockPlatform {
    fn move_pointer(&self, point: Point) -> Result<()> {
        self.pointer.set(point);
        Ok(())
    }

    fn click(&self, point: Point, button: MouseButton, modifiers: Modifiers) -> Result<()> {
        self.pointer.set(point);
        self.clicks.borrow_mut().push(RecordedClick {
            point,
            button,
            modifiers,
        });
        Ok(())
    }

    fn note_pointer(&self) -> Result<()> {
        self.noted_pointer.set(Some(self.pointer.get()));
        Ok(())
    }

    fn pointer_moved_by_user(&self) -> Result<bool> {
        let Some(noted) = self.noted_pointer.get() else {
            return Ok(false);
        };
        let current = self.pointer.get();
        Ok((current.x - noted.x).abs() > 5 || (current.y - noted.y).abs() > 5)
    }
}

impl ControlSignals for MockPlatform {
    fn emergency_requested(&self) -> Result<bool> {
        Ok(self.emergency.get())
    }

    fn take_pause(&self) -> bool {
        self.pause.replace(false)
    }
}
