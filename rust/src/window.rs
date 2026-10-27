//! The X11 window: 180 x 44, always on top, frameless (override-redirect, so it is drawn above fullscreen
//! games), drag with the left button, right-click menu, position and frame flag kept in the config file.
//!
//! Everything is drawn here with the built-in bitmap font ([`crate::font`]) into a pixmap that is copied to
//! the window: no server-side fonts, no toolkit. The display is reached only through its Unix socket
//! ([`crate::display`]).

use crate::config::{self, Config, Monitor};
use crate::display::{self, Connected, LocalDisplay};
use crate::font::{BIG, Font, SMALL};
use crate::meter::{APP, POLL, Poll, Reading};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ButtonPressEvent, ChangeGCAux, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, GrabMode, Gravity, PropMode, Rectangle, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

pub const W: i32 = 180;
pub const H: i32 = 44;
/// The bar: x0, y0, x1, y1.
pub const BAR: (i32, i32, i32, i32) = (44, 6, 130, 18);

const BG: u32 = 0x1d1f21;
const LABEL: u32 = 0xd8cfb4;
const BAR_EDGE: u32 = 0x4a4d50;
const BAR_BG: u32 = 0x2a2c2e;
const HINT: u32 = 0xb8c0c4;
const PCT: u32 = 0xf0f0f0;
const STATUS: u32 = 0xaab4b8;
const MENU_BG: u32 = 0x2a2c2e;
const MENU_FG: u32 = 0xe6e6e6;
const MENU_HI: u32 = 0x3e4246;

/// Green below 50 %, amber below 75 %, red from 75 %; grey without a value.
pub fn bar_color(fraction: Option<f64>) -> u32 {
    match fraction {
        None => 0x85969d,
        Some(f) if f < 0.5 => 0xc7df7a,
        Some(f) if f < 0.75 => 0xe8bf70,
        Some(_) => 0xee7868,
    }
}

/// `#rrggbb`.
pub fn hex(rgb: u32) -> String {
    format!("#{rgb:06x}")
}

/// What the window shows for a reading: percent text, status, bar colour, hint, filled bar width.
pub fn shown(r: &Reading) -> (String, String, u32, String, i32) {
    let frac = r.percent.and(r.fraction);
    let pct = r.percent.map_or("-".to_string(), |p| format!("{p}%"));
    let hint = if r.percent.is_none() { r.hint.clone() } else { String::new() };
    let fill = ((BAR.2 - BAR.0) as f64 * frac.unwrap_or(0.0)) as i32;
    (pct, r.status.clone(), bar_color(frac), hint, fill)
}

/// Whether a RandR version has the monitor list (RandR 1.5).
pub fn randr_has_monitors(major: u32, minor: u32) -> bool {
    (major, minor) >= (1, 5)
}

/// The menu entries, top to bottom (None: separator).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Frame,
    Reset,
    Help,
    Quit,
}

const MENU: [(Option<Item>, &str); 5] = [
    (Some(Item::Frame), "Window frame"),
    (Some(Item::Reset), "Reset position"),
    (Some(Item::Help), "Help / details"),
    (None, ""),
    (Some(Item::Quit), "Quit"),
];
const ITEM_H: i32 = 18;
const SEP_H: i32 = 7;
const MENU_PAD: i32 = 2;
const CHECK_W: i32 = 20;

/// One menu row: entry (None: separator), label, y0, y1.
pub type MenuRow = (Option<Item>, &'static str, i32, i32);

/// Menu size and its rows.
pub fn menu_layout() -> (i32, i32, Vec<MenuRow>) {
    let text_w = MENU.iter().map(|(_, l)| SMALL.width(l, false)).max().unwrap_or(0);
    let mut y = MENU_PAD;
    let mut rows = Vec::new();
    for (item, label) in MENU {
        let h = if item.is_some() { ITEM_H } else { SEP_H };
        rows.push((item, label, y, y + h));
        y += h;
    }
    (CHECK_W + text_w + 16, y + MENU_PAD, rows)
}

/// Wraps `text` to lines of at most `max` characters: words are kept whole where possible and a wrapped line
/// keeps the indent of its paragraph.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let indent: String = para.chars().take_while(|c| *c == ' ').collect();
        let mut line = String::new();
        for word in para.trim_start_matches(' ').split(' ') {
            let mut word = word.to_string();
            loop {
                let room = if line.is_empty() { max } else { max.saturating_sub(line.chars().count() + 1) };
                let first = line.is_empty();
                let lead = if first { indent.chars().count() } else { 0 };
                if word.chars().count() + lead <= room {
                    if first {
                        line.push_str(&indent);
                    } else {
                        line.push(' ');
                    }
                    line.push_str(&word);
                    break;
                }
                if !first {
                    out.push(std::mem::take(&mut line));
                    continue;
                }
                // a word longer than a whole line: split it
                let cut = max.saturating_sub(lead).max(1);
                let head: String = word.chars().take(cut).collect();
                out.push(format!("{indent}{head}"));
                word = word.chars().skip(cut).collect();
                if word.is_empty() {
                    break;
                }
            }
        }
        out.push(line);
    }
    out
}

type Res<T> = Result<T, String>;

fn x<T, E: std::fmt::Display>(r: Result<T, E>) -> Res<T> {
    r.map_err(|e| e.to_string())
}

struct Atoms {
    net_wm_state: u32,
    above: u32,
    wm_protocols: u32,
    wm_delete_window: u32,
    net_wm_name: u32,
    utf8_string: u32,
    window_type: u32,
    type_popup_menu: u32,
    type_dialog: u32,
    frame_extents: u32,
}

struct Menu {
    win: u32,
    pixmap: u32,
    w: i32,
    h: i32,
    hover: Option<usize>,
    /// A button went down on an entry since the menu opened: only then does a release choose it (the release of
    /// the right-click that opened the menu never does, even when the menu opened under the pointer).
    pressed: bool,
}

struct HelpWin {
    win: u32,
    lines: Vec<String>,
}

/// The meter window.
pub struct Window<'m> {
    c: Connected,
    root: u32,
    depth: u8,
    screen_size: (i32, i32),
    atoms: Atoms,
    colors: Vec<(u32, u32)>,
    gc: u32,
    win: u32,
    pixmap: u32,
    meter: &'m mut dyn Poll,
    path: PathBuf,
    pub cfg: Config,
    pub pos: (i32, i32),
    drag: Option<(i32, i32)>,
    reading: Reading,
    said: Option<String>,
    menu: Option<Menu>,
    help: Option<HelpWin>,
    help_text: String,
    /// The text of the last help window (test builds trace it).
    help_text_shown: String,
    done: bool,
}

impl<'m> Window<'m> {
    /// Opens the window on display `d` (a display on this computer, through its Unix socket).
    pub fn new(d: &LocalDisplay, meter: &'m mut dyn Poll, path: PathBuf, help_text: &str) -> Res<Self> {
        let c = display::connect(d)?;
        let setup = c.conn.setup();
        let screen = &setup.roots[c.screen];
        let (root, depth, colormap) = (screen.root, screen.root_depth, screen.default_colormap);
        let screen_size = (i32::from(screen.width_in_pixels), i32::from(screen.height_in_pixels));
        let conn = &c.conn;
        let names = [
            "_NET_WM_STATE",
            "_NET_WM_STATE_ABOVE",
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "_NET_WM_NAME",
            "UTF8_STRING",
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_POPUP_MENU",
            "_NET_WM_WINDOW_TYPE_DIALOG",
            "_NET_FRAME_EXTENTS",
        ];
        let cookies: Vec<_> = names
            .iter()
            .map(|n| conn.intern_atom(false, n.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let mut a = Vec::new();
        for ck in cookies {
            a.push(x(ck.reply())?.atom);
        }
        let atoms = Atoms {
            net_wm_state: a[0],
            above: a[1],
            wm_protocols: a[2],
            wm_delete_window: a[3],
            net_wm_name: a[4],
            utf8_string: a[5],
            window_type: a[6],
            type_popup_menu: a[7],
            type_dialog: a[8],
            frame_extents: a[9],
        };
        let palette = [
            BG, LABEL, BAR_EDGE, BAR_BG, HINT, PCT, STATUS, MENU_BG, MENU_FG, MENU_HI, 0x85969d, 0xc7df7a, 0xe8bf70,
            0xee7868,
        ];
        let cookies: Vec<_> = palette
            .iter()
            .map(|rgb| {
                let ch = |s: u32| ((rgb >> s) & 0xff) as u16 * 0x101;
                conn.alloc_color(colormap, ch(16), ch(8), ch(0))
            })
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let mut colors = Vec::new();
        for (rgb, ck) in palette.iter().zip(cookies) {
            colors.push((*rgb, x(ck.reply())?.pixel));
        }
        let gc = x(conn.generate_id())?;
        x(conn.create_gc(gc, root, &CreateGCAux::new().graphics_exposures(0)))?;
        let cfg = config::load_config(&path);
        let mut w = Window {
            c,
            root,
            depth,
            screen_size,
            atoms,
            colors,
            gc,
            win: 0,
            pixmap: 0,
            meter,
            path,
            cfg,
            pos: (0, 0),
            drag: None,
            reading: Reading::new("waiting for game"),
            said: None,
            menu: None,
            help: None,
            help_text: help_text.to_string(),
            help_text_shown: String::new(),
            done: false,
        };
        w.place();
        w.create_main()?;
        x(w.c.conn.flush())?;
        Ok(w)
    }

    fn px(&self, rgb: u32) -> u32 {
        self.colors.iter().find(|c| c.0 == rgb).map_or(0, |c| c.1)
    }

    /// The X screen's monitors from RandR 1.5, or [] without RandR or with an older one.
    pub fn monitors(&self) -> Vec<Monitor> {
        let conn = &self.c.conn;
        if !matches!(conn.extension_information(randr::X11_EXTENSION_NAME), Ok(Some(_))) {
            return Vec::new();
        }
        let Some(v) = conn.randr_query_version(1, 5).ok().and_then(|c| c.reply().ok()) else { return Vec::new() };
        if !randr_has_monitors(v.major_version, v.minor_version) {
            return Vec::new();
        }
        let Some(reply) = conn.randr_get_monitors(self.root, true).ok().and_then(|c| c.reply().ok()) else {
            return Vec::new();
        };
        reply
            .monitors
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| Monitor {
                x: i32::from(m.x),
                y: i32::from(m.y),
                width: i32::from(m.width),
                height: i32::from(m.height),
                primary: m.primary,
            })
            .collect()
    }

    fn monitors_or_screen(&self) -> Vec<Monitor> {
        let mons = self.monitors();
        if mons.is_empty() {
            vec![Monitor { x: 0, y: 0, width: self.screen_size.0, height: self.screen_size.1, primary: true }]
        } else {
            mons
        }
    }

    fn place(&mut self) {
        let mons = self.monitors_or_screen();
        self.pos = config::choose_position(self.cfg.x, self.cfg.y, &mons, W);
    }

    /// Title, class and close protocol of one of our windows.
    fn describe(&self, win: u32, title: &str) -> Res<()> {
        let conn = &self.c.conn;
        x(conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes()))?;
        x(conn.change_property8(
            PropMode::REPLACE,
            win,
            self.atoms.net_wm_name,
            self.atoms.utf8_string,
            title.as_bytes(),
        ))?;
        x(conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"dehulud\0DeHulud\0"))?;
        x(conn.change_property32(
            PropMode::REPLACE,
            win,
            self.atoms.wm_protocols,
            AtomEnum::ATOM,
            &[self.atoms.wm_delete_window],
        ))?;
        Ok(())
    }

    /// Always on top the way Tk's "-topmost" asks for it: _NET_WM_STATE_ABOVE in the window's _NET_WM_STATE before
    /// it is mapped, and once mapped the EWMH client message to the root window (the window manager's part).
    fn keep_above(&self, win: u32, mapped: bool) -> Res<()> {
        let conn = &self.c.conn;
        if !mapped {
            x(conn.change_property32(
                PropMode::REPLACE,
                win,
                self.atoms.net_wm_state,
                AtomEnum::ATOM,
                &[self.atoms.above],
            ))?;
            return Ok(());
        }
        const NET_WM_STATE_ADD: u32 = 1;
        let ev =
            ClientMessageEvent::new(32, win, self.atoms.net_wm_state, [NET_WM_STATE_ADD, self.atoms.above, 0, 0, 0]);
        x(conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, ev))?;
        Ok(())
    }

    fn create_main(&mut self) -> Res<()> {
        let conn = &self.c.conn;
        let win = x(conn.generate_id())?;
        let aux = CreateWindowAux::new()
            .background_pixel(self.px(BG))
            .override_redirect(u32::from(!self.cfg.frame))
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::STRUCTURE_NOTIFY,
            );
        x(conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            self.root,
            self.pos.0 as i16,
            self.pos.1 as i16,
            W as u16,
            H as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        ))?;
        self.describe(win, APP)?;
        let mut hints = WmSizeHints::new();
        hints.position = Some((WmSizeHintsSpecification::UserSpecified, self.pos.0, self.pos.1));
        hints.size = Some((WmSizeHintsSpecification::ProgramSpecified, W, H));
        hints.min_size = Some((W, H)); // not resizable
        hints.max_size = Some((W, H));
        hints.win_gravity = Some(Gravity::NORTH_WEST);
        x(hints.set_normal_hints(conn, win))?;
        self.keep_above(win, false)?;
        let pixmap = x(conn.generate_id())?;
        x(conn.create_pixmap(self.depth, pixmap, win, W as u16, H as u16))?;
        x(conn.map_window(win))?;
        self.keep_above(win, true)?;
        self.win = win;
        self.pixmap = pixmap;
        self.draw_main()
    }

    fn destroy_main(&mut self) -> Res<()> {
        let conn = &self.c.conn;
        x(conn.free_pixmap(self.pixmap))?;
        x(conn.destroy_window(self.win))?;
        Ok(())
    }

    fn fill(&self, target: u32, rgb: u32, rects: &[Rectangle]) -> Res<()> {
        if rects.is_empty() {
            return Ok(());
        }
        let conn = &self.c.conn;
        x(conn.change_gc(self.gc, &ChangeGCAux::new().foreground(self.px(rgb))))?;
        x(conn.poly_fill_rectangle(target, self.gc, rects))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&self, target: u32, rgb: u32, font: &Font, s: &str, x0: i32, y0: i32, bold: bool) -> Res<()> {
        let rects: Vec<Rectangle> = font
            .runs(s, x0, y0, bold)
            .into_iter()
            .map(|(rx, ry, w)| Rectangle { x: rx as i16, y: ry as i16, width: w as u16, height: 1 })
            .collect();
        self.fill(target, rgb, &rects)
    }

    fn draw_main(&mut self) -> Res<()> {
        let pm = self.pixmap;
        let rect = |x0: i32, y0: i32, w: i32, h: i32| Rectangle {
            x: x0 as i16,
            y: y0 as i16,
            width: w as u16,
            height: h as u16,
        };
        let (pct, status, color, hint, fill) = shown(&self.reading);
        let (x0, y0, x1, y1) = BAR;
        self.fill(pm, BG, &[rect(0, 0, W, H)])?;
        self.text(pm, LABEL, &SMALL, "WORM", 6, 12 - SMALL.ascent / 2, true)?;
        self.fill(pm, BAR_EDGE, &[rect(x0, y0, x1 - x0 + 1, y1 - y0 + 1)])?;
        self.fill(pm, BAR_BG, &[rect(x0 + 1, y0 + 1, x1 - x0 - 1, y1 - y0 - 1)])?;
        if fill > 0 {
            self.fill(pm, color, &[rect(x0, y0, fill, y1 - y0)])?;
        }
        if !hint.is_empty() {
            let w = SMALL.width(&hint, false);
            self.text(pm, HINT, &SMALL, &hint, (x0 + x1) / 2 - w / 2, 12 - SMALL.ascent / 2, false)?;
        }
        self.text(pm, PCT, &BIG, &pct, W - 6 - BIG.width(&pct, false), 12 - BIG.height / 2, false)?;
        self.text(pm, STATUS, &SMALL, &status, 6, 32 - SMALL.ascent / 2, false)?;
        x(self.c.conn.copy_area(pm, self.win, self.gc, 0, 0, 0, 0, W as u16, H as u16))?;
        Ok(())
    }

    /// The position of the window's outer frame: where a window manager with north-west gravity puts the frame
    /// for a requested position, so that the saved position opens the window at the same place again. It is the
    /// window's position on the root window minus the frame's left and top (_NET_FRAME_EXTENTS) when the window
    /// manager reports them.
    fn frame_position(&self) -> Option<(i32, i32)> {
        let conn = &self.c.conn;
        let t = conn.translate_coordinates(self.win, self.root, 0, 0).ok()?.reply().ok()?;
        let extents = conn
            .get_property(false, self.win, self.atoms.frame_extents, AtomEnum::CARDINAL, 0, 4)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()))
            .filter(|v| v.len() == 4)
            .unwrap_or_else(|| vec![0; 4]);
        Some((i32::from(t.dst_x) - extents[0] as i32, i32::from(t.dst_y) - extents[2] as i32))
    }

    fn move_to(&self, pos: (i32, i32)) -> Res<()> {
        x(self.c.conn.configure_window(self.win, &ConfigureWindowAux::new().x(pos.0).y(pos.1)))?;
        Ok(())
    }

    fn save(&self) {
        config::save_config(&self.path, &self.cfg);
    }

    pub fn toggle_frame(&mut self) -> Res<()> {
        self.cfg.frame = !self.cfg.frame;
        // A new window with the other override-redirect value (an X window cannot change it while mapped).
        self.destroy_main()?;
        self.create_main()?;
        self.save();
        Ok(())
    }

    pub fn reset_position(&mut self) -> Res<()> {
        self.cfg.x = None;
        self.cfg.y = None;
        self.place();
        self.move_to(self.pos)?;
        self.save();
        Ok(())
    }

    fn quit(&mut self) -> Res<()> {
        if self.cfg.frame {
            // the window manager may have moved the framed window
            if let Some((fx, fy)) = self.frame_position() {
                self.cfg.x = Some(fx);
                self.cfg.y = Some(fy);
            }
        } else if self.cfg.x.is_some() {
            self.cfg.x = Some(self.pos.0);
            self.cfg.y = Some(self.pos.1);
        }
        self.save();
        self.close_menu()?;
        self.close_help()?;
        self.destroy_main()?;
        x(self.c.conn.flush())?;
        self.done = true;
        Ok(())
    }

    fn monitor_at(&self, px: i32, py: i32) -> Monitor {
        let mons = self.monitors_or_screen();
        mons.iter()
            .find(|m| m.x <= px && px < m.x + m.width && m.y <= py && py < m.y + m.height)
            .or_else(|| mons.iter().find(|m| m.primary))
            .copied()
            .unwrap_or(mons[0])
    }

    fn open_menu(&mut self, root_x: i32, root_y: i32) -> Res<()> {
        self.close_menu()?;
        let (mw, mh, _) = menu_layout();
        let m = self.monitor_at(root_x, root_y);
        let mx = (root_x + 1).min(m.x + m.width - mw).max(m.x);
        let my = (root_y + 1).min(m.y + m.height - mh).max(m.y);
        let conn = &self.c.conn;
        let win = x(conn.generate_id())?;
        let aux = CreateWindowAux::new().background_pixel(self.px(MENU_BG)).override_redirect(1).event_mask(
            EventMask::EXPOSURE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::LEAVE_WINDOW,
        );
        x(conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            self.root,
            mx as i16,
            my as i16,
            mw as u16,
            mh as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        ))?;
        self.describe(win, "DeHulud menu")?;
        x(conn.change_property32(
            PropMode::REPLACE,
            win,
            self.atoms.window_type,
            AtomEnum::ATOM,
            &[self.atoms.type_popup_menu],
        ))?;
        let pixmap = x(conn.generate_id())?;
        x(conn.create_pixmap(self.depth, pixmap, win, mw as u16, mh as u16))?;
        x(conn.map_window(win))?;
        // Clicks elsewhere close the menu: the pointer is grabbed while it is open (as Tk's menus do). Without the
        // grab (another program holds one) the menu still works and closes on a click on DeHulud.
        if let Ok(ck) = conn.grab_pointer(
            true,
            win,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            x11rb::NONE,
            x11rb::CURRENT_TIME,
        ) {
            let _ = ck.reply();
        }
        self.menu = Some(Menu { win, pixmap, w: mw, h: mh, hover: None, pressed: false });
        self.draw_menu()?;
        crate::testhook::trace(|| self.state_json("menu"));
        Ok(())
    }

    fn close_menu(&mut self) -> Res<()> {
        if let Some(m) = self.menu.take() {
            let conn = &self.c.conn;
            x(conn.ungrab_pointer(x11rb::CURRENT_TIME))?;
            x(conn.free_pixmap(m.pixmap))?;
            x(conn.destroy_window(m.win))?;
            crate::testhook::trace(|| self.state_json("menu closed"));
        }
        Ok(())
    }

    fn draw_menu(&self) -> Res<()> {
        let Some(m) = &self.menu else { return Ok(()) };
        let rect = |x0: i32, y0: i32, w: i32, h: i32| Rectangle {
            x: x0 as i16,
            y: y0 as i16,
            width: w as u16,
            height: h as u16,
        };
        let pm = m.pixmap;
        self.fill(pm, BAR_EDGE, &[rect(0, 0, m.w, m.h)])?;
        self.fill(pm, MENU_BG, &[rect(1, 1, m.w - 2, m.h - 2)])?;
        let (_, _, rows) = menu_layout();
        for (i, (item, label, y0, y1)) in rows.iter().enumerate() {
            match item {
                None => self.fill(pm, BAR_EDGE, &[rect(6, (y0 + y1) / 2, m.w - 12, 1)])?,
                Some(item) => {
                    if m.hover == Some(i) {
                        self.fill(pm, MENU_HI, &[rect(2, *y0, m.w - 4, y1 - y0)])?;
                    }
                    let ty = (y0 + y1) / 2 - SMALL.ascent / 2;
                    if *item == Item::Frame && self.cfg.frame {
                        // check mark
                        let tick =
                            [(0, 3, 2), (1, 4, 2), (2, 5, 2), (3, 4, 2), (4, 3, 2), (5, 2, 2), (6, 1, 2), (7, 0, 2)];
                        let rects: Vec<Rectangle> =
                            tick.iter().map(|(dx, dy, h)| rect(6 + dx, ty + dy, 1, *h)).collect();
                        self.fill(pm, MENU_FG, &rects)?;
                    }
                    self.text(pm, MENU_FG, &SMALL, label, CHECK_W, ty, false)?;
                }
            }
        }
        x(self.c.conn.copy_area(pm, m.win, self.gc, 0, 0, 0, 0, m.w as u16, m.h as u16))?;
        Ok(())
    }

    fn menu_row_at(&self, ex: i32, ey: i32) -> Option<usize> {
        let m = self.menu.as_ref()?;
        if ex < 0 || ex >= m.w {
            return None;
        }
        let (_, _, rows) = menu_layout();
        rows.iter().position(|(item, _, y0, y1)| item.is_some() && *y0 <= ey && ey < *y1)
    }

    fn activate(&mut self, item: Item) -> Res<()> {
        self.close_menu()?;
        match item {
            Item::Frame => self.toggle_frame(),
            Item::Reset => self.reset_position(),
            Item::Help => self.show_help(),
            Item::Quit => self.quit(),
        }
    }

    fn show_help(&mut self) -> Res<()> {
        self.close_help()?;
        let text = if self.reading.detail.is_empty() { self.help_text.clone() } else { self.reading.detail.clone() };
        let mut lines = wrap(&text, 80);
        lines.push(String::new());
        lines.push("Click here or press a key to close.".to_string());
        let w = lines.iter().map(|l| SMALL.width(l, false)).max().unwrap_or(0) + 24;
        let h = lines.len() as i32 * 11 + 20;
        let m = self.monitor_at(self.pos.0 + W / 2, self.pos.1 + H / 2);
        let (hx, hy) = (m.x + (m.width - w).max(0) / 2, m.y + (m.height - h).max(0) / 2);
        let conn = &self.c.conn;
        let win = x(conn.generate_id())?;
        let aux = CreateWindowAux::new().background_pixel(self.px(MENU_BG)).event_mask(
            EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::STRUCTURE_NOTIFY,
        );
        x(conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            self.root,
            hx as i16,
            hy as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        ))?;
        self.describe(win, "DeHulud help")?;
        x(conn.change_property32(
            PropMode::REPLACE,
            win,
            self.atoms.window_type,
            AtomEnum::ATOM,
            &[self.atoms.type_dialog],
        ))?;
        let mut hints = WmSizeHints::new();
        hints.position = Some((WmSizeHintsSpecification::ProgramSpecified, hx, hy));
        hints.min_size = Some((w, h));
        hints.max_size = Some((w, h));
        x(hints.set_normal_hints(conn, win))?;
        self.keep_above(win, false)?;
        x(conn.map_window(win))?;
        self.keep_above(win, true)?;
        self.help = Some(HelpWin { win, lines });
        self.help_text_shown = text;
        crate::testhook::trace(|| self.state_json("help"));
        Ok(())
    }

    fn draw_help(&self) -> Res<()> {
        let Some(hw) = &self.help else { return Ok(()) };
        let n = hw.lines.len();
        for (i, line) in hw.lines.iter().enumerate() {
            let rgb = if i + 1 == n { STATUS } else { MENU_FG };
            self.text(hw.win, rgb, &SMALL, line, 12, 10 + i as i32 * 11, false)?;
        }
        Ok(())
    }

    fn close_help(&mut self) -> Res<()> {
        if let Some(hw) = self.help.take() {
            x(self.c.conn.destroy_window(hw.win))?;
        }
        Ok(())
    }

    fn tick(&mut self) -> Res<()> {
        let r = self.meter.poll();
        if !r.detail.is_empty() && self.said.as_deref() != Some(r.detail.as_str()) {
            eprintln!("{}", r.detail);
            self.said = Some(r.detail.clone());
        }
        let changed = r != self.reading;
        self.reading = r;
        if changed {
            self.draw_main()?;
        }
        crate::testhook::trace(|| self.state_json("tick"));
        Ok(())
    }

    fn on_press(&mut self, e: &ButtonPressEvent) -> Res<()> {
        let (rx, ry) = (i32::from(e.root_x), i32::from(e.root_y));
        if let Some(m) = &self.menu {
            if e.event == m.win
                && let Some(i) = self.menu_row_at(i32::from(e.event_x), i32::from(e.event_y))
            {
                if let Some(m) = &mut self.menu {
                    m.pressed = true;
                }
                self.set_hover(Some(i))?;
                return Ok(());
            }
            return self.close_menu(); // a click anywhere else closes the menu
        }
        if self.help.as_ref().is_some_and(|h| h.win == e.event) {
            return self.close_help();
        }
        if e.event != self.win {
            return Ok(());
        }
        match e.detail {
            1 => self.drag = Some((rx - self.pos.0, ry - self.pos.1)),
            3 => self.open_menu(rx, ry)?,
            _ => {}
        }
        Ok(())
    }

    fn set_hover(&mut self, hover: Option<usize>) -> Res<()> {
        if let Some(m) = &mut self.menu
            && m.hover != hover
        {
            m.hover = hover;
            return self.draw_menu();
        }
        Ok(())
    }

    fn handle(&mut self, ev: Event) -> Res<()> {
        match ev {
            Event::Expose(e) if e.count == 0 => {
                if e.window == self.win {
                    x(self.c.conn.copy_area(self.pixmap, self.win, self.gc, 0, 0, 0, 0, W as u16, H as u16))?;
                } else if self.menu.as_ref().is_some_and(|m| m.win == e.window) {
                    self.draw_menu()?;
                } else if self.help.as_ref().is_some_and(|h| h.win == e.window) {
                    self.draw_help()?;
                }
            }
            Event::ButtonPress(e) => self.on_press(&e)?,
            Event::MotionNotify(e) => {
                if self.menu.as_ref().is_some_and(|m| m.win == e.event) {
                    let row = self.menu_row_at(i32::from(e.event_x), i32::from(e.event_y));
                    self.set_hover(row)?;
                } else if e.event == self.win
                    && let Some((dx, dy)) = self.drag
                {
                    self.pos = (i32::from(e.root_x) - dx, i32::from(e.root_y) - dy);
                    self.move_to(self.pos)?;
                }
            }
            Event::LeaveNotify(e) => {
                if self.menu.as_ref().is_some_and(|m| m.win == e.event) {
                    self.set_hover(None)?;
                }
            }
            Event::ButtonRelease(e) => {
                if self.menu.as_ref().is_some_and(|m| m.win == e.event && m.pressed) {
                    if let Some(i) = self.menu_row_at(i32::from(e.event_x), i32::from(e.event_y))
                        && let (Some(item), _, _, _) = menu_layout().2[i]
                    {
                        return self.activate(item);
                    }
                } else if e.event == self.win && e.detail == 1 && self.drag.take().is_some() {
                    self.cfg.x = Some(self.pos.0);
                    self.cfg.y = Some(self.pos.1);
                    self.save();
                    crate::testhook::trace(|| self.state_json("moved"));
                }
            }
            Event::KeyPress(e) => {
                if self.help.as_ref().is_some_and(|h| h.win == e.event) {
                    self.close_help()?;
                }
            }
            Event::ClientMessage(e) => {
                let delete = e.format == 32 && e.data.as_data32()[0] == self.atoms.wm_delete_window;
                if delete && e.window == self.win {
                    self.quit()?;
                } else if delete && self.help.as_ref().is_some_and(|h| h.win == e.window) {
                    self.close_help()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs until Quit, until `should_stop` says so (checked once per poll), or until the display goes away.
    pub fn run(&mut self, should_stop: &mut dyn FnMut() -> bool) -> Res<()> {
        let interval = crate::testhook::poll_interval().unwrap_or(POLL);
        let fd = self.c.conn.stream().as_raw_fd();
        let mut next = Instant::now();
        loop {
            if Instant::now() >= next {
                if should_stop() {
                    return self.quit();
                }
                self.tick()?;
                next = Instant::now() + interval;
            }
            x(self.c.conn.flush())?;
            while let Some(ev) = x(self.c.conn.poll_for_event())? {
                self.handle(ev)?;
                if self.done {
                    return Ok(());
                }
            }
            x(self.c.conn.flush())?;
            let wait = next.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the duration of the call.
            unsafe { libc::poll(&mut pfd, 1, wait.as_millis() as libc::c_int) };
        }
    }

    /// The window's state as one JSON line (test builds trace it).
    pub fn state_json(&self, event: &str) -> String {
        use crate::json::quote;
        let (pct, status, color, hint, fill) = shown(&self.reading);
        let menu = self.menu.as_ref().map_or("null".to_string(), |m| m.win.to_string());
        let items: Vec<String> = menu_layout()
            .2
            .iter()
            .filter(|r| r.0.is_some())
            .map(|(_, l, y0, y1)| format!("[{}, {y0}, {y1}]", quote(l)))
            .collect();
        let help = self.help.as_ref().map_or("null".to_string(), |h| h.win.to_string());
        format!(
            "{{\"event\": {}, \"window\": {}, \"frame\": {}, \"pos\": [{}, {}], \"socket\": {}, \"shown\": [{}, {}, {}, {}], \
             \"fill\": {fill}, \"menu\": {menu}, \"items\": [{}], \"help\": {help}, \"help_text\": {}, \"monitors\": {}}}",
            quote(event),
            self.win,
            self.cfg.frame,
            self.pos.0,
            self.pos.1,
            quote(&self.c.addr.to_string()),
            quote(&pct),
            quote(&status),
            quote(&hex(color)),
            quote(&hint),
            items.join(", "),
            quote(&self.help_text_shown),
            self.monitors().len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_colours() {
        let got: Vec<String> = [None, Some(0.0), Some(0.49), Some(0.5), Some(0.74), Some(0.75), Some(1.0)]
            .iter()
            .map(|f| hex(bar_color(*f)))
            .collect();
        assert_eq!(got, ["#85969d", "#c7df7a", "#c7df7a", "#e8bf70", "#e8bf70", "#ee7868", "#ee7868"]);
    }

    #[test]
    fn what_the_window_shows() {
        assert_eq!(
            shown(&Reading::value("on foot", 42, 0.425)),
            ("42%".into(), "on foot".into(), 0xc7df7a, String::new(), 36)
        );
        assert_eq!(
            shown(&Reading::with("Unsupported game version", "d", "PE 1700000000")),
            ("-".into(), "Unsupported game version".into(), 0x85969d, "PE 1700000000".into(), 0)
        );
        assert_eq!(shown(&Reading::value("vehicle", 100, 1.0)).4, 86);
    }

    #[test]
    fn texts_fit_the_window() {
        let (x0, _, x1, _) = BAR;
        assert!(W - 6 - BIG.width("100%", false) >= x1, "the percent never covers the bar");
        assert!(6 + SMALL.width("WORM", true) < x0);
        for hint in ["right-click", "PE 1700000000", "PE 4294967295"] {
            let w = SMALL.width(hint, false);
            assert!((x0 + x1) / 2 - w / 2 >= x0 && (x0 + x1) / 2 - w / 2 + w <= x1, "{hint}");
        }
        for status in
            ["Unsupported game version", "not started by --launch", "on foot, no threshold", "waiting for game"]
        {
            assert!(6 + SMALL.width(status, false) <= W, "{status}");
        }
        assert!(32 - SMALL.ascent / 2 + SMALL.height <= H);
    }

    #[test]
    fn randr_versions() {
        assert!(!randr_has_monitors(1, 4));
        assert!(randr_has_monitors(1, 5));
        assert!(randr_has_monitors(1, 6));
        assert!(randr_has_monitors(2, 0));
        assert!(!randr_has_monitors(0, 9));
    }

    #[test]
    fn menu_rows() {
        let (w, h, rows) = menu_layout();
        let labels: Vec<&str> = rows.iter().map(|r| r.1).collect();
        assert_eq!(labels, ["Window frame", "Reset position", "Help / details", "", "Quit"]);
        assert!(w > SMALL.width("Help / details", false) && h == rows.last().unwrap().3 + MENU_PAD);
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("a b c", 80), ["a b c"]);
        assert_eq!(wrap("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(wrap("  aaa bbb ccc", 9), ["  aaa bbb", "  ccc"]);
        assert_eq!(wrap("x\n\ny", 80), ["x", "", "y"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        for line in wrap(&crate::help::permission_help(Some(1), false, "/a/b c/dehulud"), 80) {
            assert!(line.chars().count() <= 80, "{line}");
        }
    }
}
