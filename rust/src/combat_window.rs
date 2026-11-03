//! DeHulud Combat's X11 window: 180 px wide, always on top, frameless (override-redirect, so it is drawn above
//! fullscreen games). A header row with the three section switches, then the sections that are on (worm meter,
//! Damage taken, Damage dealt, see [`crate::sections`]) and the status line; the height follows the sections and
//! the top-left corner stays. Drag with the left button, click a switch to flip it, right-click menu; position,
//! frame flag and switches kept in its config file ([`crate::combat_config`]).
//!
//! Drawn like DeHulud's window ([`crate::window`], whose colours, menu and help-text helpers it uses): the
//! built-in bitmap font into a pixmap copied to the window, no server-side fonts, no toolkit, the display only
//! through its Unix socket ([`crate::display`]).

use crate::combat_config::{self as config, Config, Monitor};
use crate::display::{self, Connected, LocalDisplay};
use crate::font::{BIG, Font, SMALL};
use crate::meter::{POLL, Poll, Reading};
use crate::sections::{
    self, AMBER, DASH, DamageView, FADE, Feature, Features, LABEL, MAX_H, OFF, Section, TEAL, damage_cells,
    header_cells, header_zone, is_click, layout,
};
use crate::window::{
    BAR, BAR_BG, BAR_EDGE, BG, CHECK_W, HINT, Item, MENU_BG, MENU_FG, MENU_HI, PCT, Res, STATUS, hex, menu_layout,
    randr_has_monitors, shown, wrap, x,
};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ButtonPressEvent, ChangeGCAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, GrabMode, Gravity, PropMode, Rectangle, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

pub const W: i32 = sections::W;
/// The window title (DeHulud's is "DeHulud").
pub const TITLE: &str = "DeHulud Combat";

/// The sections shown: switched on in the config file and not switched off by a flag.
pub fn enabled(cfg: &Config, off: Features) -> Features {
    let (c, o) = (cfg.sections, off);
    Features { worm: c.worm && !o.worm, taken: c.taken && !o.taken, dealt: c.dealt && !o.dealt }
}

/// What a click on the switch of `f` changes: the switch is set to the opposite of what is shown (and saved by
/// the caller), a flag's override of it ends, and the section's numbers are forgotten at once (with every
/// section off no poll would clear them).
pub fn flip(cfg: &mut Config, off: &mut Features, view: &mut DamageView, f: Feature) {
    let on = !enabled(cfg, *off).has(f);
    cfg.sections.set(f, on);
    off.set(f, false);
    view.reset(f);
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
    /// The config file; None without a home folder (then the position is not kept).
    path: Option<PathBuf>,
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
    /// Sections switched off by a command-line flag for this run (never saved).
    off: Features,
    view: DamageView,
    /// Where the left button went down (root coordinates), and whether it has become a drag.
    press: (i32, i32),
    moved: bool,
    /// The window's current height.
    h: i32,
}

impl<'m> Window<'m> {
    /// Opens the window on display `d` (a display on this computer, through its Unix socket).
    /// `off`: sections switched off by a command-line flag, for this run only.
    pub fn new(
        d: &LocalDisplay,
        meter: &'m mut dyn Poll,
        path: Option<PathBuf>,
        help_text: &str,
        off: Features,
    ) -> Res<Self> {
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
        // allocated once: the fade steps of the damage numbers too
        let mut palette =
            vec![BG, LABEL, BAR_EDGE, BAR_BG, HINT, PCT, STATUS, MENU_BG, MENU_FG, MENU_HI, DASH, 0xc7df7a, OFF];
        for (_, steps) in FADE {
            for rgb in steps {
                if !palette.contains(&rgb) {
                    palette.push(rgb);
                }
            }
        }
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
        let cfg = path.as_deref().map_or(config::DEFAULTS, config::load_config);
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
            off,
            view: DamageView::default(),
            press: (0, 0),
            moved: false,
            h: 0,
        };
        w.h = layout(w.enabled()).1;
        w.place();
        w.create_main()?;
        x(w.c.conn.flush())?;
        Ok(w)
    }

    /// The sections shown: switched on in the config file and not switched off by a flag.
    pub fn enabled(&self) -> Features {
        enabled(&self.cfg, self.off)
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
        x(conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"dehulud-combat\0DeHulud Combat\0",
        ))?;
        x(conn.change_property32(
            PropMode::REPLACE,
            win,
            self.atoms.wm_protocols,
            AtomEnum::ATOM,
            &[self.atoms.wm_delete_window],
        ))?;
        Ok(())
    }

    /// Always on top: see [`crate::window::keep_above`].
    fn keep_above(&self, win: u32, mapped: bool) -> Res<()> {
        crate::window::keep_above(&self.c, self.root, (self.atoms.net_wm_state, self.atoms.above), win, mapped)
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
            self.h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        ))?;
        self.describe(win, TITLE)?;
        self.size_hints(win, true)?;
        self.keep_above(win, false)?;
        let pixmap = x(conn.generate_id())?;
        // as tall as the tallest window: a resize only changes how much of it is drawn and copied
        x(conn.create_pixmap(self.depth, pixmap, win, W as u16, MAX_H as u16))?;
        x(conn.map_window(win))?;
        self.keep_above(win, true)?;
        self.win = win;
        self.pixmap = pixmap;
        self.draw_main()
    }

    /// Size hints for the current height: not resizable by the user, north-west gravity (the top-left stays).
    fn size_hints(&self, win: u32, with_position: bool) -> Res<()> {
        let mut hints = WmSizeHints::new();
        if with_position {
            hints.position = Some((WmSizeHintsSpecification::UserSpecified, self.pos.0, self.pos.1));
        }
        hints.size = Some((WmSizeHintsSpecification::ProgramSpecified, W, self.h));
        hints.min_size = Some((W, self.h)); // not resizable
        hints.max_size = Some((W, self.h));
        hints.win_gravity = Some(Gravity::NORTH_WEST);
        x(hints.set_normal_hints(&self.c.conn, win))?;
        Ok(())
    }

    /// The height for the sections now on: hints first (a window manager keeps the frame to them), then the
    /// window's own height only, so its top-left corner stays where it is.
    fn resize(&mut self) -> Res<()> {
        self.h = layout(self.enabled()).1;
        self.size_hints(self.win, false)?;
        x(self.c.conn.configure_window(self.win, &ConfigureWindowAux::new().height(self.h as u32)))?;
        self.draw_main()
    }

    /// A click on a header switch: flips that section as shown and saves the switch to the config file (a
    /// flag's override of that switch ends with it). The other switches keep their saved values.
    pub fn toggle(&mut self, f: Feature) -> Res<()> {
        flip(&mut self.cfg, &mut self.off, &mut self.view, f);
        self.meter.reset(f); // the damage readers forget it too
        self.save();
        self.resize()?;
        crate::testhook::trace(|| self.state_json("switched"));
        Ok(())
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
        let on = self.enabled();
        let mid = 12 - SMALL.ascent / 2; // small text centred on y = 12 of a section
        self.fill(pm, BG, &[rect(0, 0, W, self.h)])?;
        for (hx, text, rgb) in header_cells(on) {
            self.text(pm, rgb, &SMALL, &text, hx, 9 - SMALL.ascent / 2, false)?;
        }
        let (shown_sections, _) = layout(on);
        for (section, y) in &shown_sections {
            let y = *y;
            match section {
                Section::Header => {}
                Section::Worm => {
                    let (x0, y0, x1, y1) = (BAR.0, y + BAR.1, BAR.2, y + BAR.3);
                    self.text(pm, LABEL, &SMALL, "WORM", 6, y + mid, true)?;
                    self.fill(pm, BAR_EDGE, &[rect(x0, y0, x1 - x0 + 1, y1 - y0 + 1)])?;
                    self.fill(pm, BAR_BG, &[rect(x0 + 1, y0 + 1, x1 - x0 - 1, y1 - y0 - 1)])?;
                    if fill > 0 {
                        self.fill(pm, color, &[rect(x0, y0, fill, y1 - y0)])?;
                    }
                    if !hint.is_empty() {
                        let w = SMALL.width(&hint, false);
                        self.text(pm, HINT, &SMALL, &hint, (x0 + x1) / 2 - w / 2, y + mid, false)?;
                    }
                    let pct_top = y + 12 - BIG.height / 2;
                    self.text(pm, PCT, &BIG, &pct, W - 6 - BIG.width(&pct, false), pct_top, false)?;
                }
                Section::Taken => self.text(pm, LABEL, &SMALL, "TAKEN", 6, y + mid, true)?,
                Section::Dealt => {
                    self.text(pm, LABEL, &SMALL, "DEALT", 6, y + mid, true)?;
                    self.text(pm, AMBER, &SMALL, "HEALTH", 122 - SMALL.width("HEALTH", false), y + mid, false)?;
                    self.text(pm, TEAL, &SMALL, "SHIELD", 174 - SMALL.width("SHIELD", false), y + mid, false)?;
                }
                Section::Status => self.text(pm, STATUS, &SMALL, &status, 6, y + 8 - SMALL.ascent / 2, false)?,
            }
        }
        for (section, right, top, text, rgb) in damage_cells(&self.view, on) {
            let y = shown_sections.iter().find(|s| s.0 == section).map_or(0, |s| s.1);
            self.text(pm, rgb, &BIG, &text, right - BIG.width(&text, false), y + top, false)?;
        }
        x(self.c.conn.copy_area(pm, self.win, self.gc, 0, 0, 0, 0, W as u16, self.h as u16))?;
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
        if let Some(path) = &self.path {
            config::save_config(path, &self.cfg);
        }
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
        self.describe(win, "DeHulud Combat menu")?;
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
        let m = self.monitor_at(self.pos.0 + W / 2, self.pos.1 + self.h / 2);
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
        self.describe(win, "DeHulud Combat help")?;
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
        let on = self.enabled();
        if !on.any() {
            // every section off: nothing is read
            crate::testhook::trace(|| self.state_json("tick"));
            return Ok(());
        }
        let r = self.meter.poll_enabled(on);
        if !r.detail.is_empty() && self.said.as_deref() != Some(r.detail.as_str()) {
            eprintln!("{}", r.detail);
            self.said = Some(r.detail.clone());
        }
        let before = self.view.clone();
        self.view.step(r.taken, r.dealt, on);
        let changed = r != self.reading || self.view != before; // numbers age (fade) every poll
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
            1 => {
                self.drag = Some((rx - self.pos.0, ry - self.pos.1));
                self.press = (rx, ry);
                // outside the header row a press drags at once, as before
                self.moved = header_zone(i32::from(e.event_x), i32::from(e.event_y)).is_none();
            }
            3 => self.open_menu(rx, ry)?,
            _ => {}
        }
        Ok(())
    }

    /// The pointer of a left-button press is at (root) rx, ry: a press on a header switch stays a click until it
    /// has moved 3 px; from then on, and for any other press, the window follows the pointer.
    fn drag_to(&mut self, rx: i32, ry: i32) -> Res<()> {
        let Some((dx, dy)) = self.drag else { return Ok(()) };
        if !self.moved && is_click(rx - self.press.0, ry - self.press.1) {
            return Ok(());
        }
        self.moved = true;
        self.pos = (rx - dx, ry - dy);
        self.move_to(self.pos)
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
                    x(self.c.conn.copy_area(self.pixmap, self.win, self.gc, 0, 0, 0, 0, W as u16, self.h as u16))?;
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
                } else if e.event == self.win && self.drag.is_some() {
                    self.drag_to(i32::from(e.root_x), i32::from(e.root_y))?;
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
                } else if e.event == self.win && e.detail == 1 && self.drag.is_some() {
                    // moved 3 px or more without a motion event: still a drag
                    self.drag_to(i32::from(e.root_x), i32::from(e.root_y))?;
                    self.drag = None;
                    if self.moved {
                        self.cfg.x = Some(self.pos.0);
                        self.cfg.y = Some(self.pos.1);
                        self.save();
                        crate::testhook::trace(|| self.state_json("moved"));
                    } else if let Some(f) = header_zone(i32::from(e.event_x), i32::from(e.event_y)) {
                        self.toggle(f)?;
                    }
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
        let on: Vec<String> = self.enabled().keys().iter().map(|k| quote(k)).collect();
        let cells: Vec<String> = damage_cells(&self.view, self.enabled())
            .iter()
            .map(|(s, x, y, t, rgb)| format!("[{}, {x}, {y}, {}, {}]", quote(s.name()), quote(t), quote(&hex(*rgb))))
            .collect();
        format!(
            "{{\"event\": {}, \"window\": {}, \"frame\": {}, \"pos\": [{}, {}], \"socket\": {}, \"shown\": [{}, {}, {}, {}], \
             \"fill\": {fill}, \"menu\": {menu}, \"items\": [{}], \"help\": {help}, \"help_text\": {}, \"monitors\": {}, \
             \"size\": [{W}, {}], \"enabled\": [{}], \"cells\": [{}]}}",
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
            self.monitors().len(),
            self.h,
            on.join(", "),
            cells.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // every text stays inside its section
        let (all, h) = layout(Features::ALL);
        let status = all.iter().find(|s| s.0 == Section::Status).unwrap().1;
        assert!(status + 8 - SMALL.ascent / 2 + SMALL.height <= h);
        assert!(9 - SMALL.ascent / 2 >= 0 && 9 - SMALL.ascent / 2 + SMALL.height <= sections::HEADER_H);
        assert!(sections::DEALT_TOPS[3] + BIG.height <= sections::DEALT_H);
        assert!(sections::TAKEN_TOPS[3] + BIG.height <= sections::TAKEN_H);
        assert!(122 - SMALL.width("HEALTH", false) > 6 + SMALL.width("DEALT", true));
    }

    #[test]
    fn flipping_a_switch() {
        let (mut cfg, mut off, mut view) =
            (config::DEFAULTS, Features { worm: true, ..Features::NONE }, DamageView::default());
        let numbers = |v: &mut DamageView| v.step(Some(Some(37.0)), Some((Some(120.0), Some(40.0))), Features::ALL);
        let shown = |v: &DamageView, on| damage_cells(v, on).into_iter().map(|c| c.3).collect::<Vec<_>>();
        numbers(&mut view);
        // off and straight on again, no poll between: the old numbers are gone, '-' until the next poll
        flip(&mut cfg, &mut off, &mut view, Feature::Taken);
        flip(&mut cfg, &mut off, &mut view, Feature::Taken);
        assert_eq!(shown(&view, enabled(&cfg, off)), ["-", "120", "40"]);
        flip(&mut cfg, &mut off, &mut view, Feature::Dealt);
        flip(&mut cfg, &mut off, &mut view, Feature::Dealt);
        assert_eq!(shown(&view, enabled(&cfg, off)), ["-", "-", "-"]);
        // all off (no polls at all), then on again
        numbers(&mut view);
        for f in [Feature::Taken, Feature::Dealt, Feature::Taken, Feature::Dealt] {
            flip(&mut cfg, &mut off, &mut view, f);
        }
        assert_eq!(shown(&view, enabled(&cfg, off)), ["-", "-", "-"]);
        // the worm switch: off by flag (file on); the click switches it on, ends the override, keeps the numbers
        assert!(!enabled(&cfg, off).worm && cfg.sections.worm);
        numbers(&mut view);
        flip(&mut cfg, &mut off, &mut view, Feature::Worm);
        assert!(enabled(&cfg, off).worm && !off.worm && cfg.sections.worm);
        assert_eq!(shown(&view, Features::ALL), ["-37", "120", "40"]);
    }
}
