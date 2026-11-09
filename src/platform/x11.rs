//! Capture et injection d'entrées X11/XWayland.
//!
//! D2R sous Proton est un client XWayland. On capture le drawable de la
//! fenêtre et on injecte les clics avec l'extension XTest.

use std::cell::Cell;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use image::{ImageBuffer, Rgb, RgbImage};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, GrabMode,
    ImageFormat, InputFocus, ModMask, StackMode, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::{
    ControlSignals, InputInjector, Modifiers, MouseButton, Point, ScreenCapture, Size, WindowId,
    WindowInfo, WindowSystem,
};

const XTEST_MOTION_NOTIFY: u8 = 6;
const XTEST_BUTTON_PRESS: u8 = 4;
const XTEST_BUTTON_RELEASE: u8 = 5;
const XTEST_KEY_PRESS: u8 = 2;
const XTEST_KEY_RELEASE: u8 = 3;

const KEYSYM_CONTROL_L: u32 = 0xffe3;
const KEYSYM_SHIFT_L: u32 = 0xffe1;
const KEYSYM_F8: u32 = 0xffc5;
const KEYSYM_F9: u32 = 0xffc6;

impl MouseButton {
    fn code(self) -> u8 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Right => 3,
        }
    }
}

pub struct X11 {
    connection: RustConnection,
    root: Window,
    net_client_list: u32,
    net_wm_name: u32,
    utf8_string: u32,
    net_active_window: u32,
    ctrl_keycode: u8,
    shift_keycode: u8,
    pause_keycode: u8,
    emergency_keycode: u8,
    pause_pending: Cell<bool>,
    last_pointer: Cell<Option<(i32, i32)>>,
}

impl X11 {
    pub fn new() -> Result<Self> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| anyhow!("connexion X11 impossible: {error}"))?;
        let root = connection.setup().roots[screen_number].root;
        let net_client_list = intern_atom(&connection, b"_NET_CLIENT_LIST")?;
        let net_wm_name = intern_atom(&connection, b"_NET_WM_NAME")?;
        let utf8_string = intern_atom(&connection, b"UTF8_STRING")?;
        let net_active_window = intern_atom(&connection, b"_NET_ACTIVE_WINDOW")?;

        let mut x11 = Self {
            connection,
            root,
            net_client_list,
            net_wm_name,
            utf8_string,
            net_active_window,
            ctrl_keycode: 0,
            shift_keycode: 0,
            pause_keycode: 0,
            emergency_keycode: 0,
            pause_pending: Cell::new(false),
            last_pointer: Cell::new(None),
        };
        x11.ctrl_keycode = x11.find_keycode(KEYSYM_CONTROL_L)?;
        x11.shift_keycode = x11.find_keycode(KEYSYM_SHIFT_L)?;
        x11.pause_keycode = x11.find_keycode(KEYSYM_F8).unwrap_or(0);
        if x11.pause_keycode != 0 {
            let grab_ok = x11
                .connection
                .grab_key(
                    false,
                    x11.root,
                    ModMask::ANY,
                    x11.pause_keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .is_ok();
            if !grab_ok {
                eprintln!(
                    "Impossible de capturer F8 pour la pause; \
                     la pause clavier restera indisponible."
                );
                x11.pause_keycode = 0;
            }
            let _ = x11.connection.flush();
        }
        x11.emergency_keycode = x11.find_keycode(KEYSYM_F9).unwrap_or(0);
        if x11.emergency_keycode != 0 {
            let grab_ok = x11
                .connection
                .grab_key(
                    false,
                    x11.root,
                    ModMask::ANY,
                    x11.emergency_keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .is_ok();
            if !grab_ok {
                eprintln!(
                    "Impossible de capturer F9 pour l'arrêt d'urgence; \
                     le failsafe de souris reste actif."
                );
                x11.emergency_keycode = 0;
            }
            let _ = x11.connection.flush();
        }
        Ok(x11)
    }

    /// Position absolue du curseur (coordonnées racine).
    pub fn pointer_position(&self) -> Result<(i32, i32)> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .context("XQueryPointer a échoué")?
            .reply()
            .context("XQueryPointer sans réponse")?;
        Ok((i32::from(reply.root_x), i32::from(reply.root_y)))
    }

    /// Mémorise la position actuelle comme référence du failsafe.
    pub fn note_pointer(&self) -> Result<()> {
        self.last_pointer.set(Some(self.pointer_position()?));
        Ok(())
    }

    /// Vrai si le curseur a bougé depuis la dernière position laissée par le
    /// programme. Une petite tolérance absorbe les arrondis X11.
    pub fn pointer_moved_by_user(&self) -> Result<bool> {
        let Some((last_x, last_y)) = self.last_pointer.get() else {
            return Ok(false);
        };
        let (current_x, current_y) = self.pointer_position()?;
        Ok((current_x - last_x).abs() > 5 || (current_y - last_y).abs() > 5)
    }

    /// Vrai si F9 a été pressé depuis le dernier appel.
    pub fn emergency_requested(&self) -> Result<bool> {
        if self.emergency_keycode == 0 && self.pause_keycode == 0 {
            return Ok(false);
        }

        let mut stop = false;
        while let Some(event) = self
            .connection
            .poll_for_event()
            .context("poll_for_event X11 a échoué")?
        {
            if let Event::KeyPress(key) = event {
                if self.pause_keycode != 0 && key.detail == self.pause_keycode {
                    self.pause_pending.set(true);
                }
                if self.emergency_keycode != 0 && key.detail == self.emergency_keycode {
                    stop = true;
                }
            }
        }
        Ok(stop)
    }

    /// Consomme une demande de pause F8.
    pub fn take_pause(&self) -> bool {
        self.pause_pending.replace(false)
    }

    pub fn find_game_window(&self) -> Result<WindowInfo> {
        if let Some(window) = self.find_by_client_list()? {
            return self.window_info(window);
        }
        if let Some(window) = self.find_in_tree(self.root, 3)? {
            return self.window_info(window);
        }
        bail!("fenêtre de Diablo II introuvable")
    }

    pub fn capture_window(&self, window: &WindowInfo) -> Result<RgbImage> {
        let reply = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                window.id.0 as u32,
                0,
                0,
                window.size.width as u16,
                window.size.height as u16,
                !0,
            )
            .context("XGetImage a échoué")?
            .reply()
            .context("XGetImage n'a pas retourné d'image")?;

        let pixel_count = window.size.width as usize * window.size.height as usize;
        if pixel_count == 0 || reply.data.len() % pixel_count != 0 {
            bail!(
                "format d'image X11 inattendu: {} octets pour {} pixels",
                reply.data.len(),
                pixel_count
            );
        }
        let bytes_per_pixel = reply.data.len() / pixel_count;
        if bytes_per_pixel != 3 && bytes_per_pixel != 4 {
            bail!("format d'image X11 non supporté: {bytes_per_pixel} octets/pixel");
        }

        let mut rgb = Vec::with_capacity(pixel_count * 3);
        for pixel in reply.data.chunks_exact(bytes_per_pixel) {
            // X11 expose souvent du BGR en little-endian.
            rgb.push(pixel[2]);
            rgb.push(pixel[1]);
            rgb.push(pixel[0]);
        }

        ImageBuffer::<Rgb<u8>, _>::from_raw(window.size.width, window.size.height, rgb)
            .ok_or_else(|| anyhow!("buffer RGB invalide"))
    }

    pub fn focus_window(&self, window: &WindowInfo) -> Result<()> {
        // Demande au WM de donner le focus, puis force le focus clavier.
        let event = ClientMessageEvent::new(
            32,
            window.id.0 as u32,
            self.net_active_window,
            [2u32, 0, 0, 0, 0],
        );
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .context("impossible d'envoyer _NET_ACTIVE_WINDOW")?;
        self.connection
            .configure_window(
                window.id.0 as u32,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .context("impossible de lever la fenêtre du jeu")?;
        self.connection
            .set_input_focus(InputFocus::PARENT, window.id.0 as u32, 0u32)
            .context("impossible de donner le focus au jeu")?;
        self.connection.flush().context("flush X11 impossible")?;
        sleep(Duration::from_millis(150));
        Ok(())
    }

    pub fn is_focused(&self, window: &WindowInfo) -> Result<bool> {
        let focus = self
            .connection
            .get_input_focus()
            .context("XGetInputFocus a échoué")?
            .reply()
            .context("XGetInputFocus n'a pas retourné de réponse")?
            .focus;
        if focus == 0 {
            return Ok(false);
        }
        self.is_descendant(focus, window.id.0 as u32)
    }

    pub fn click(&self, x: i32, y: i32, button: MouseButton, modifiers: Modifiers) -> Result<()> {
        self.move_pointer(x, y)?;
        sleep(Duration::from_millis(25));

        if let Err(error) = self.press_modifiers(modifiers) {
            let _ = self.release_modifiers(modifiers);
            return Err(error);
        }
        let click_result = self.press_and_release_button(button);
        let release_result = self.release_modifiers(modifiers);
        click_result?;
        release_result?;
        self.note_pointer()?;
        Ok(())
    }

    pub fn move_pointer(&self, x: i32, y: i32) -> Result<()> {
        self.connection
            .xtest_fake_input(XTEST_MOTION_NOTIFY, 0, 0, self.root, x as i16, y as i16, 0)
            .context("XTest motion a échoué")?;
        self.connection.flush().context("flush X11 impossible")?;
        Ok(())
    }

    fn press_and_release_button(&self, button: MouseButton) -> Result<()> {
        self.connection
            .xtest_fake_input(XTEST_BUTTON_PRESS, button.code(), 0, self.root, 0, 0, 0)
            .context("XTest button press a échoué")?;
        self.connection.flush().context("flush X11 impossible")?;
        sleep(Duration::from_millis(35));
        self.connection
            .xtest_fake_input(XTEST_BUTTON_RELEASE, button.code(), 0, self.root, 0, 0, 0)
            .context("XTest button release a échoué")?;
        self.connection.flush().context("flush X11 impossible")?;
        // Laisser au jeu une ou deux images pour traiter le clic complet
        // avant de relâcher Ctrl+Shift.
        sleep(Duration::from_millis(50));
        Ok(())
    }

    fn press_modifiers(&self, modifiers: Modifiers) -> Result<()> {
        if modifiers == Modifiers::CTRL_SHIFT {
            self.key(XTEST_KEY_PRESS, self.ctrl_keycode)?;
            self.key(XTEST_KEY_PRESS, self.shift_keycode)?;
            self.connection.flush().context("flush X11 impossible")?;
            sleep(Duration::from_millis(80));

            if !self.modifiers_down()? {
                self.key(XTEST_KEY_PRESS, self.ctrl_keycode)?;
                self.key(XTEST_KEY_PRESS, self.shift_keycode)?;
                self.connection.flush().context("flush X11 impossible")?;
                sleep(Duration::from_millis(80));
                if !self.modifiers_down()? {
                    bail!("Ctrl+Shift ne sont pas actifs avant le clic");
                }
            }
        }
        Ok(())
    }

    fn modifiers_down(&self) -> Result<bool> {
        let keys = self
            .connection
            .query_keymap()
            .context("XQueryKeymap a échoué")?
            .reply()
            .context("XQueryKeymap sans réponse")?
            .keys;
        let is_down = |keycode: u8| keys[(keycode / 8) as usize] & (1 << (keycode % 8)) != 0;
        Ok(is_down(self.ctrl_keycode) && is_down(self.shift_keycode))
    }

    fn release_modifiers(&self, modifiers: Modifiers) -> Result<()> {
        if modifiers == Modifiers::CTRL_SHIFT {
            self.key(XTEST_KEY_RELEASE, self.shift_keycode)?;
            self.key(XTEST_KEY_RELEASE, self.ctrl_keycode)?;
            self.connection.flush().context("flush X11 impossible")?;
        }
        Ok(())
    }

    fn key(&self, event_type: u8, keycode: u8) -> Result<()> {
        self.connection
            .xtest_fake_input(event_type, keycode, 0, self.root, 0, 0, 0)
            .context("XTest key event a échoué")?;
        Ok(())
    }

    fn find_keycode(&self, keysym: u32) -> Result<u8> {
        let setup = self.connection.setup();
        let min_keycode = setup.min_keycode;
        let count = setup.max_keycode - min_keycode + 1;
        let reply = self
            .connection
            .get_keyboard_mapping(min_keycode, count)
            .context("XGetKeyboardMapping a échoué")?
            .reply()
            .context("XGetKeyboardMapping n'a pas retourné de réponse")?;
        let per_keycode = reply.keysyms_per_keycode as usize;

        for (index, keysyms) in reply.keysyms.chunks(per_keycode).enumerate() {
            if keysyms.iter().any(|candidate| *candidate == keysym) {
                return Ok(min_keycode + index as u8);
            }
        }
        bail!("keysym {keysym:#x} introuvable dans le clavier X11")
    }

    fn is_descendant(&self, mut child: Window, ancestor: Window) -> Result<bool> {
        loop {
            if child == ancestor {
                return Ok(true);
            }
            if child == 0 || child == self.root {
                return Ok(false);
            }
            let tree = self
                .connection
                .query_tree(child)
                .context("XQueryTree a échoué")?
                .reply()
                .context("XQueryTree n'a pas retourné de réponse")?;
            if tree.parent == child {
                return Ok(false);
            }
            child = tree.parent;
        }
    }

    fn find_by_client_list(&self) -> Result<Option<Window>> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.net_client_list,
                AtomEnum::WINDOW,
                0,
                u32::MAX,
            )
            .context("XGetProperty _NET_CLIENT_LIST a échoué")?
            .reply()
            .context("_NET_CLIENT_LIST sans réponse")?;
        if let Some(values) = reply.value32() {
            for window in values {
                if self.is_game_window(window)? {
                    return Ok(Some(window));
                }
            }
        }
        Ok(None)
    }

    fn find_in_tree(&self, window: Window, depth: u32) -> Result<Option<Window>> {
        if self.is_game_window(window)? {
            return Ok(Some(window));
        }
        if depth == 0 {
            return Ok(None);
        }
        let tree = self
            .connection
            .query_tree(window)
            .context("XQueryTree a échoué")?
            .reply()
            .context("XQueryTree sans réponse")?;
        for child in tree.children {
            if let Some(found) = self.find_in_tree(child, depth - 1)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    fn is_game_window(&self, window: Window) -> Result<bool> {
        let title = self.window_title(window).unwrap_or_default().to_lowercase();
        let class = self.window_class(window).unwrap_or_default().to_lowercase();
        Ok(
            (title.contains("diablo ii") && title.contains("resurrected"))
                || (class == "steam_app_default" && title.contains("diablo")),
        )
    }

    fn window_title(&self, window: Window) -> Result<String> {
        let reply = self
            .connection
            .get_property(false, window, self.net_wm_name, self.utf8_string, 0, 1024)
            .context("XGetProperty _NET_WM_NAME a échoué")?
            .reply()
            .context("_NET_WM_NAME sans réponse")?;
        if !reply.value.is_empty() {
            return Ok(String::from_utf8_lossy(&reply.value).to_string());
        }
        let reply = self
            .connection
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            .context("XGetProperty WM_NAME a échoué")?
            .reply()
            .context("WM_NAME sans réponse")?;
        Ok(String::from_utf8_lossy(&reply.value).to_string())
    }

    fn window_class(&self, window: Window) -> Result<String> {
        let reply = self
            .connection
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
            .context("XGetProperty WM_CLASS a échoué")?
            .reply()
            .context("WM_CLASS sans réponse")?;
        let parts: Vec<&[u8]> = reply.value.split(|byte| *byte == 0).collect();
        let value = parts
            .get(1)
            .or_else(|| parts.first())
            .copied()
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(value).to_string())
    }

    fn window_info(&self, window: Window) -> Result<WindowInfo> {
        let geometry = self
            .connection
            .get_geometry(window)
            .context("XGetGeometry a échoué")?
            .reply()
            .context("XGetGeometry sans réponse")?;
        let position = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .context("XTranslateCoordinates a échoué")?
            .reply()
            .context("XTranslateCoordinates sans réponse")?;
        Ok(WindowInfo {
            id: WindowId(u64::from(window)),
            title: self.window_title(window).unwrap_or_default(),
            origin: Point {
                x: i32::from(position.dst_x),
                y: i32::from(position.dst_y),
            },
            size: Size {
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
            },
        })
    }
}

fn intern_atom(connection: &RustConnection, name: &[u8]) -> Result<u32> {
    connection
        .intern_atom(false, name)
        .context("intern_atom a échoué")?
        .reply()
        .context("intern_atom sans réponse")
        .map(|reply| reply.atom)
}

// Le contrat commun délègue aux méthodes inhérentes de X11, qui restent
// spécifiques à X11 (noms et signatures historiques). Les chemins `X11::`
// évitent toute ambiguïté entre la méthode inhérente et celle du trait.

impl WindowSystem for X11 {
    fn find_game_window(&self) -> Result<WindowInfo> {
        X11::find_game_window(self)
    }

    fn focus(&self, window: &WindowInfo) -> Result<()> {
        X11::focus_window(self, window)
    }

    fn is_focused(&self, window: &WindowInfo) -> Result<bool> {
        X11::is_focused(self, window)
    }
}

impl ScreenCapture for X11 {
    fn capture(&self, window: &WindowInfo) -> Result<RgbImage> {
        X11::capture_window(self, window)
    }
}

impl InputInjector for X11 {
    fn move_pointer(&self, point: Point) -> Result<()> {
        X11::move_pointer(self, point.x, point.y)
    }

    fn click(&self, point: Point, button: MouseButton, modifiers: Modifiers) -> Result<()> {
        X11::click(self, point.x, point.y, button, modifiers)
    }

    fn note_pointer(&self) -> Result<()> {
        X11::note_pointer(self)
    }

    fn pointer_moved_by_user(&self) -> Result<bool> {
        X11::pointer_moved_by_user(self)
    }
}

impl ControlSignals for X11 {
    fn emergency_requested(&self) -> Result<bool> {
        X11::emergency_requested(self)
    }

    fn take_pause(&self) -> bool {
        X11::take_pause(self)
    }
}
