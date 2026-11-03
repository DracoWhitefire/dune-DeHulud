#!/usr/bin/env python3
"""DeHulud Combat - sandworm threat meter, damage taken and damage dealt, for Dune: Awakening on Linux.

Shows how close you are to the sandworm attack threshold (a percent and a bar), the damage you take
and the damage you deal, in a small always-on-top window with a switch for each of the three
sections (or the worm meter as one terminal line). It finds the running game (Proton) by itself.
Needs dehulud.py next to it.

  dehulud_combat.py                    window (falls back to text without tkinter or a local X display)
  dehulud_combat.py --text             one-line worm meter in the terminal
  dehulud_combat.py --launch %command% Steam launch options: starts the game as a child of DeHulud
                                       Combat, so a ptrace_scope of 1 (descendants only) allows the read
  --no-worm, --no-taken, --no-dealt
                                       start with that section of the window switched off, for this run
                                       only (a click on its label in the window's header row switches it
                                       and is saved); text mode shows only the worm meter, so --text
                                       refuses --no-worm

Read-only: it reads /proc/<pid>/{cmdline,maps,status} of your own processes, the game's memory with
process_vm_readv, the ptrace setting, /etc/os-release and its own config file. It never writes to the
game, never attaches a debugger, never signals or injects anything, sends no input and opens no network
connection (the window only opens a local X display, through its Unix socket).
Python 3.9+, standard library only.
"""
import collections
import math
import os
import struct
import subprocess
import sys
import time

import dehulud
from dehulud import (BUILDS, EXE, MAX_DEPTH, MENU_HINT, POLL_MS, ProcSource, Reader, _Soft, _import_tk,  # noqa: F401
                     bar_color, choose_position, format_line, local_display, monitors, pe_identity, tk_install_hint)

APP = "DeHulud Combat"

Reading = collections.namedtuple("Reading", "status percent fraction detail hint taken dealt",
                                 defaults=(None, None, "", "", None, None))
Reading.__doc__ = """One poll: percent/fraction are None whenever the meter must show '-'.

taken and dealt are what the poll hands the two damage sections of the window (None for a section the poll did
not read, and whenever nothing trustworthy was read: the section then shows '-'):
  taken: None, or a tuple of the health drops emitted in this poll (at most one non-negative number)
  dealt: None, or (health, shield): two tuples of the drops emitted in this poll (at most one number each)
When a number is emitted is the reader's business; ages, history, fading and capping are the window's."""

# The window's three switchable sections: config keys in header order, and their command-line flags.
FEATURES = ("worm", "damage_taken", "damage_dealt")
ALL = frozenset(FEATURES)
FLAGS = {"--no-worm": "worm", "--no-taken": "damage_taken", "--no-dealt": "damage_dealt"}
TEXT = frozenset(("worm",))   # what the terminal line shows (the section flags are for the window)
TEXT_NO_WORM = "dehulud-combat: text mode shows only the worm meter; drop --no-worm"


# Damage routes by (PE TimeDateStamp, SizeOfImage); absent builds have no damage route.
DAMAGE_BUILDS = {
    (1793384724, 220024832): {
        "attribute_set": 0x1400,       # ADuneCharacter.m_DuneCharacterAttributeSet
        "health": 0x98,                # UDuneCharacterAttributeSet.CurrentHealth
        "maximum": 0xC0,               # UDuneCharacterAttributeSet.MaxHealth
        "power": 0x320,                # UDuneCharacterAttributeSet.CurrentPower
        "payload": 0x18,               # FglGameplayAttributeData.CurrentValuePtr
        "current": 0xC,                # FGameplayAttributeData.CurrentValue
        "cls_attributes": 0xC146A88,   # UDuneCharacterAttributeSet class slot RVA
        "cls_controller": 0xC175948,   # ADunePlayerController class slot RVA
        "coordinator": 0x1048,         # ADunePlayerController.m_InventoryCoordinator
        "owner": 0x268,                # UInventoryCoordinator.m_OwnerCharacter
        "cls_coordinator": 0xC1CD1D8,  # UInventoryCoordinator class slot RVA
        "aim": 0x1E58,                 # ADunePlayerCharacter.m_CachedAimTarget
        "soft_lock": 0x1D80,           # ADunePlayerCharacter.m_SoftLockTarget
        "weak_index": 0,               # FWeakObjectPtr.ObjectIndex
        "weak_serial": 4,              # FWeakObjectPtr.ObjectSerialNumber
        "chunks": 0xC337DC0,           # GUObjectArray.ObjObjects.Objects
        "count": 0xC337DD4,            # GUObjectArray.ObjObjects.NumElements
        "shift": 16,                   # FChunkedFixedUObjectArray chunk index shift
        "stride": 24,                  # FUObjectItem stride
        "item_object": 0,              # FUObjectItem.Object
        "item_flags": 8,               # FUObjectItem.Flags
        "item_serial": 16,             # FUObjectItem.SerialNumber
        "reject_flags": 0x30200000,    # FUObjectItem.Flags weak reject mask
    },
}


def damage_layout(build):
    """Damage offsets for an identified entry of the shared builds table, or None."""
    return next((DAMAGE_BUILDS.get(key) for key, entry in BUILDS.items() if entry is build), None)


Attached = collections.namedtuple("Attached", "base build reader")
Attached.__doc__ = "The attached game as the damage readers get it: image base, builds table entry, memory reader."


class NoDamage:
    """The damage readers of the meter: what one poll reads for each damage section of the window.

    The meter calls `taken` only while the Damage taken section is on, `dealt` only while Damage dealt is on, and
    `mode` (the status line, e.g. 'on foot') only while the worm meter, which otherwise names the state, is off
    and a damage section is on. Each gets the Attached game once per poll and returns the shape documented on
    Reading (None: nothing trustworthy, '-'); an OSError counts as None. These stand-ins read nothing: every
    damage section shows '-' until readers for the game's values exist.

    Two hooks, no-ops here (readers subclass this class and override what they need), neither with a memory
    handle (they cannot read):
      begin(now, enabled, image): once per poll, before mode/taken/dealt, only while the game is attached and at
        least one damage section is on. now: monotonic seconds (the meter's clock); enabled: the poll's set of
        feature keys; image: the attachment serial, a number that changes with every new attachment (a restarted
        game, or an exec under the same pid into an image with another PE identity) and stays the same while the
        meter stays attached.
      reset(feature): on every click on a header switch (feature: 'worm', 'damage_taken' or 'damage_dealt'),
        also when a section goes off and on again with no poll between; never for the flags at startup."""

    def begin(self, now, enabled, image):
        pass

    def reset(self, feature):
        pass

    def taken(self, game):
        return None

    def dealt(self, game):
        return None

    def mode(self, game):
        return None


class DamageHistory:
    """Net drops between adjacent samples, keyed by object identity rather than targeting role."""

    def __init__(self):
        self.context = None
        self.own = None
        self.targets = {}

    @staticmethod
    def channel(previous, value, maximum, now):
        valid = value is not None and math.isfinite(value) and value >= 0
        if maximum is not None:
            valid = valid and math.isfinite(maximum) and maximum > 0 and value <= maximum
        if not valid:
            return None, None
        emitted = None
        if previous is not None:
            old, old_max, at = previous
            if 0 <= now - at < 0.5 and maximum == old_max and old > 0 and value < old:
                emitted = old - value
        return ((value, maximum, now) if value > 0 else None), emitted

    def reset(self, feature=None):
        if feature in (None, "damage_taken"):
            self.own = None
        if feature in (None, "damage_dealt"):
            self.targets.clear()

    def begin(self, context):
        if context != self.context:
            self.reset()
            self.context = context

    def taken(self, sample, now):
        if sample is None:
            self.own = None
            return None
        health, maximum = sample
        self.own, drop = self.channel(self.own, health, maximum, now)
        if not (math.isfinite(health) and math.isfinite(maximum) and 0 <= health <= maximum and maximum > 0):
            return None
        return () if drop is None else (drop,)

    def dealt(self, samples, now):
        """None sample clears both channels; invalid leaves clear only that channel."""
        totals = [0.0, 0.0]
        for key in list(self.targets):
            if key not in samples:
                del self.targets[key]
        for key, sample in samples.items():
            old = self.targets.get(key, (None, None))
            if sample is None:
                self.targets.pop(key, None)
                continue
            health, maximum, shield = sample
            h, hd = self.channel(old[0], health, maximum, now) if maximum is not None else (None, None)
            s, sd = self.channel(old[1], shield, None, now)
            self.targets[key] = (h, s)
            totals[0] += hd or 0.0
            totals[1] += sd or 0.0
        return tuple((total,) if total > 0 else () for total in totals)


class DamageReader:
    """Validated character vitals and bounded weak targets; no threat-component reads."""

    def __init__(self, clock=time.monotonic):
        self.clock = clock
        self.history = DamageHistory()
        self.recent = []
        self.path = None
        self.now = 0.0
        self.image = None

    def reset(self, feature=None):
        self.history.reset(feature)
        if feature in (None, "damage_dealt"):
            self.recent.clear()
        self.path = None

    def begin(self, now, enabled, image):
        self.now, self.path = now, None
        if image != self.image:
            self.reset()
            self.image = image
        for feature in ("damage_taken", "damage_dealt"):
            if feature not in enabled:
                self.reset(feature)

    @staticmethod
    def pointer(game, address, required=True, alignment=8):
        value, = struct.unpack("<Q", game.reader.read(address, 8))
        if value % alignment or value >= 1 << 47 or (required and not value):
            raise _Soft("no data")
        return value

    @staticmethod
    def scalar(game, address):
        return struct.unpack("<f", game.reader.read(address, 4))[0]

    def is_class(self, game, obj, slot, exact=False):
        ptr, b = self.pointer, game.build
        wanted = ptr(game, game.base + slot)
        cls = ptr(game, obj + b["class_private"], False)
        for depth in range(1 if exact else MAX_DEPTH + 1):
            if not cls:
                return False
            if cls == wanted:
                return True
            if depth < MAX_DEPTH and not exact:
                cls = ptr(game, cls + b["super_struct"], False)
        return False

    def require_class(self, game, obj, slot, exact=False):
        if not self.is_class(game, obj, slot, exact):
            raise _Soft("no data")

    def character(self, game):
        b, ptr = game.build, self.pointer
        d = damage_layout(b)
        if d is None:
            raise _Soft("no data")
        if self.path is not None:
            return self.path
        world = ptr(game, game.base + b["gworld"])
        self.require_class(game, world, b["cls_world"], True)
        gi = ptr(game, world + b["world_game_instance"])
        self.require_class(game, gi, b["cls_game_instance"])
        raw = game.reader.read(gi + b["local_players"], 12)
        data, num = struct.unpack("<Qi", raw)
        if num < 1 or not data or data % 8 or data >= 1 << 47:
            raise _Soft("not in world")
        lp = ptr(game, data)
        self.require_class(game, lp, b["cls_local_player"])
        pc = ptr(game, lp + b["player_controller"])
        self.require_class(game, pc, b["cls_player_controller"])
        pawn = ptr(game, pc + b["pawn"])
        if self.is_class(game, pawn, b["cls_dune_vehicle"]):
            mode = "vehicle"
            self.require_class(game, pc, d["cls_controller"])
            coordinator = ptr(game, pc + d["coordinator"])
            self.require_class(game, coordinator, d["cls_coordinator"])
            own = ptr(game, coordinator + d["owner"])
        else:
            mode, own = "on foot", pawn
            if ptr(game, pc + b["acknowledged_pawn"]) != own:
                raise _Soft("no data")
        self.require_class(game, own, b["cls_dune_character"])
        self.require_class(game, own, b["cls_dune_player_character"])
        context = (self.image, world, own)
        if context != self.history.context:
            self.recent.clear()
        self.history.begin(context)
        self.path = (world, own, mode)
        return self.path

    def vitals(self, game, obj, shield=False):
        d = damage_layout(game.build)
        attributes = self.pointer(game, obj + d["attribute_set"])
        self.require_class(game, attributes, d["cls_attributes"], True)
        payload = self.pointer(game, attributes + d["health"] + d["payload"], alignment=4)
        health = self.scalar(game, payload)
        maximum = self.scalar(game, attributes + d["maximum"] + d["current"])
        power = self.scalar(game, attributes + d["power"] + d["current"]) if shield else None
        return (health, maximum, power) if shield else (health, maximum)

    def mode(self, game):
        try:
            return self.character(game)[2]
        except (OSError, _Soft):
            self.reset()
            return None

    def taken(self, game):
        try:
            own = self.character(game)[1]
        except (OSError, _Soft):
            self.reset()
            return None
        try:
            sample = self.vitals(game, own)
        except (OSError, _Soft):
            sample = None
        return self.history.taken(sample, self.now)

    def handle(self, game, address):
        d, raw = damage_layout(game.build), game.reader.read(address, 8)
        index, = struct.unpack_from("<i", raw, d["weak_index"])
        serial, = struct.unpack_from("<i", raw, d["weak_serial"])
        return (index, serial) if index >= 0 and serial != 0 else None

    def target(self, game, key):
        d = damage_layout(game.build)
        index, serial = key
        count, = struct.unpack("<i", game.reader.read(game.base + d["count"], 4))
        if not 0 <= index < count:
            return None
        chunks = self.pointer(game, game.base + d["chunks"])
        chunk = self.pointer(game, chunks + (index >> d["shift"]) * 8)
        item = chunk + (index & ((1 << d["shift"]) - 1)) * d["stride"]
        def fields(raw):
            return (struct.unpack_from("<Q", raw, d["item_object"])[0],
                    struct.unpack_from("<I", raw, d["item_flags"])[0],
                    struct.unpack_from("<i", raw, d["item_serial"])[0])
        obj, flags, item_serial = fields(game.reader.read(item, d["stride"]))
        if item_serial != serial or flags & d["reject_flags"] or not obj or obj % 8 or obj >= 1 << 47:
            return None
        self.require_class(game, obj, game.build["cls_dune_character"])
        values = self.vitals(game, obj, True)
        after = fields(game.reader.read(item, d["stride"]))
        return values if after == (obj, flags, item_serial) else None

    def dealt(self, game):
        try:
            own = self.character(game)[1]
        except (OSError, _Soft):
            self.reset()
            return None
        try:
            d = damage_layout(game.build)
            aim = self.handle(game, own + d["aim"])
            soft = self.handle(game, own + d["soft_lock"])
        except (OSError, _Soft):
            self.reset("damage_dealt")
            return ((), ())
        self.recent = [(key, at) for key, at in self.recent if 0 <= self.now - at < 18 and key != aim]
        if aim is not None:
            self.recent.insert(0, (aim, self.now))
        self.recent = self.recent[:5]
        keys = list(dict.fromkeys(key for key in (aim, soft) if key is not None))
        keys += [key for key, _ in self.recent if key not in keys][:4]
        samples = {}
        for key in keys[:6]:
            try:
                samples[key] = self.target(game, key)
            except (OSError, _Soft):
                samples[key] = None
        return self.history.dealt(samples, self.now)


class Meter(dehulud.Meter):
    """DeHulud's meter, with sections: once per poll(enabled) it walks the route of each enabled section, the
    threat route for the worm meter (DeHulud's, unchanged), the damage readers for the damage sections. A section
    that is not in `enabled` is not read at all; with nothing enabled the poll reads nothing."""
    program = os.path.realpath(__file__)       # the help texts name this file

    def __init__(self, source=None, reader_factory=dehulud.Reader, launched=False, scan_interval=dehulud.SCAN_S,
                 clock=time.monotonic, damage=None):
        super().__init__(source, reader_factory, launched, scan_interval, clock)
        self.damage = damage or NoDamage()
        self.image = 0          # attachment serial, handed to the damage readers' begin()
        self.enabled = ALL      # the sections of the current poll

    def poll(self, enabled=ALL):
        self.enabled = enabled
        if not enabled & ALL:
            self.reads = 0
            return Reading("")                   # every section off: no reads at all
        return as_reading(super().poll())

    def _attach(self):
        r = super()._attach()
        if self.game is not None:
            self.image += 1
        return r

    def _read(self, base, b, r, slots):
        enabled = self.enabled
        worm = as_reading(super()._read(base, b, r, slots)) if "worm" in enabled else None
        if self.game is None:                    # refused or gone during the worm route: nothing more to read
            return worm
        game = Attached(base, b, r)
        if enabled & {"damage_taken", "damage_dealt"}:
            self.damage.begin(self.clock(), enabled, self.image)
        if worm is None:
            mode = self._damage("mode", game) if enabled & {"damage_taken", "damage_dealt"} else None
            worm = Reading(mode or "no data")
        return worm._replace(taken=self._damage("taken", game) if "damage_taken" in enabled else None,
                             dealt=self._damage("dealt", game) if "damage_dealt" in enabled else None)

    def reset(self, feature):
        """A header switch was clicked: the damage readers forget what they kept for it."""
        self.damage.reset(feature)

    def _damage(self, name, game):
        try:
            return getattr(self.damage, name)(game)
        except OSError:
            return None


def as_reading(r):
    """DeHulud's 5-field reading as DeHulud Combat's (no damage values: '-')."""
    return r if r is None or isinstance(r, Reading) else Reading(*r)


def run_text(meter, out=None, err=None, should_stop=lambda: False, sleep=time.sleep):
    """DeHulud's text meter. The line is the worm meter's, so only the worm route is read."""
    class WormOnly:
        def poll(self):
            return meter.poll(TEXT)
    dehulud.run_text(WormOnly(), out, err, should_stop, sleep)


SECTIONS = (("header", 18), ("worm", 24), ("damage_taken", 60), ("damage_dealt", 76), ("status", 20))
HEADER_ZONES = (("worm", 0, 56, 6, "Worm"), ("damage_taken", 57, 116, 60, "Taken"),
                ("damage_dealt", 117, 179, 120, "Dealt"))   # feature, click zone x0..x1, text x, label
CLICK_PX = 3            # a press that moves this far (in x or y) is a drag, not a click
LIFE = 5                # a number is removed on its fifth poll (shown at ages 0..4)
ROWS = 4                # numbers per column
CAP = 99999             # at most five digits
LABEL, OFF, DASH = "#d8cfb4", "#6b7174", "#85969d"
RED, AMBER, TEAL = "#ee7868", "#e8bf70", "#8ad4c8"
# Colour by age in polls: full at ages 0 and 1, then blended towards the background at 70, 45 and 20 %.
FADE = {RED: (RED, RED, "#af5d53", "#7b4741", "#47312f"),
        AMBER: (AMBER, AMBER, "#ab8f58", "#786745", "#463f31"),
        TEAL: (TEAL, TEAL, "#699e96", "#4e706c", "#334342")}
TAKEN_TOPS = (7, 20, 33, 46)      # top of the big glyphs per row in the Damage taken section; row 1 = newest
DEALT_TOPS = (22, 35, 48, 61)     # ... in the Damage dealt section; row 4 = newest


def layout(enabled):
    """([(section, y), ...], height) of the window for the set of enabled features."""
    out, y = [], 0
    for name, h in SECTIONS:
        if name == "header" or name in enabled or (name == "status" and enabled & ALL):
            out.append((name, y))
            y += h
    return out, y


def header_zone(x, y):
    """The feature whose header switch is at window pixel (x, y), or None outside the header row."""
    if 0 <= y < 18:
        for feature, x0, x1, _, _ in HEADER_ZONES:
            if x0 <= x <= x1:
                return feature
    return None


def is_click(dx, dy):
    """Whether a press that moved (dx, dy) by its release is a click (else it is a drag)."""
    return abs(dx) < CLICK_PX and abs(dy) < CLICK_PX


def header_cells(enabled):
    """The header row: [(x, text, colour)] of the three switches."""
    return [(x, ("[x] " if f in enabled else "[ ] ") + label, LABEL if f in enabled else OFF)
            for f, _, _, x, label in HEADER_ZONES]


def shown_number(v):
    """The whole points the window shows for an emitted drop (capped at five digits), or None for nothing."""
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0:
        return None
    n = math.floor(v + 0.5)
    return min(n, CAP) if n > 0 else None


class Column:
    """One column of damage numbers, newest first, each with its age in polls; or '-' after an unreadable poll."""

    def __init__(self):
        self.items, self.dash = [], True     # nothing read yet: '-'

    def step(self, emitted):
        """One poll: None = nothing trustworthy (clear, '-'), else a tuple of the numbers emitted in this poll."""
        if emitted is None:
            self.items, self.dash = [], True
            return
        self.dash = False
        self.items = [(n, age + 1) for n, age in self.items if age + 1 < LIFE]
        for v in emitted:
            n = shown_number(v)
            if n is not None:
                self.items.insert(0, (n, 0))
        del self.items[ROWS:]


class DamageView:
    """What the two damage sections show: the Damage taken column and the Damage dealt health/shield columns."""

    def __init__(self):
        self.taken, self.health, self.shield = Column(), Column(), Column()

    def reset(self, feature):
        """Forgets a damage section's numbers (its switch was flipped): it starts again with '-'."""
        if feature == "damage_taken":
            self.taken = Column()
        elif feature == "damage_dealt":
            self.health, self.shield = Column(), Column()

    def step(self, r, enabled):
        """One poll's reading. A section that is off forgets its numbers (it starts with '-' when switched on)."""
        if "damage_taken" in enabled:
            self.taken.step(r.taken)
        else:
            self.taken = Column()
        if "damage_dealt" in enabled:
            health, shield = r.dealt if r.dealt is not None else (None, None)
            self.health.step(health)
            self.shield.step(shield)
        else:
            self.health, self.shield = Column(), Column()


def damage_cells(view, enabled):
    """The numbers to draw: [(section, right x, top y in the section, text, colour)], big font, right-aligned."""
    out = []

    def column(section, col, right, tops, colour, sign):
        if col.dash:
            out.append((section, right, tops[0] if section == "damage_taken" else tops[-1], "-", DASH))
            return
        for i, (n, age) in enumerate(col.items):
            top = tops[i] if section == "damage_taken" else tops[ROWS - 1 - i]
            out.append((section, right, top, sign + str(n), FADE[colour][age]))

    if "damage_taken" in enabled:
        column("damage_taken", view.taken, 174, TAKEN_TOPS, RED, "-")
    if "damage_dealt" in enabled:
        column("damage_dealt", view.health, 122, DEALT_TOPS, AMBER, "")
        column("damage_dealt", view.shield, 174, DEALT_TOPS, TEAL, "")
    return out


DEFAULTS = {"x": None, "y": None, "frame": False, "worm": True, "damage_taken": True, "damage_dealt": True}


def config_path(env=None):
    """$XDG_CONFIG_HOME/dehulud-combat/config.json, by DeHulud's rules (see dehulud.config_path)."""
    return dehulud.config_path(env, "dehulud-combat")


def load_config(path):
    """Window position, frame flag and the three section switches; anything missing or malformed falls back to
    the defaults (each key on its own)."""
    return dehulud.load_config(path, DEFAULTS)


def save_config(path, cfg):
    return dehulud.save_config(path, cfg, DEFAULTS)


class Window:
    """Always-on-top window, 180 px wide: a header row with the three section switches, then the sections that
    are on (worm meter, Damage taken, Damage dealt) and the status line. Drag to move, click a switch to flip
    it, right-click for the menu. The height follows the sections; the top-left corner stays where it is."""
    W, BAR = 180, (44, 6, 130, 18)

    def __init__(self, tk, meter, config_file, should_stop=lambda: False, display=None, off=()):
        """`display` defaults to $DISPLAY; only a local display is opened (see local_display). `off`: features
        switched off by a command-line flag, for this run only (never saved)."""
        self.tk, self.meter, self.path, self.should_stop = tk, meter, config_file, should_stop
        self.cfg, self.drag, self.after_id, self.said = load_config(config_file), None, None, None
        self.off, self.press, self.moved = set(off) & ALL, None, False
        self.reading, self.view = Reading("waiting for game"), DamageView()
        self.pct = self.status = self.fill = self.hint = None     # the worm meter's items while it is shown
        name = os.environ.get("DISPLAY", "") if display is None else display
        screen = local_display(name)
        if screen is None:
            raise tk.TclError("not a display on this computer: %r" % name)
        root = self.root = tk.Tk(screenName=screen)
        root.title(APP)
        root.resizable(False, False)
        c = self.canvas = tk.Canvas(root, width=self.W, height=self.H, bg="#1d1f21", highlightthickness=0)
        c.pack()
        self.frame_var = tk.BooleanVar(value=self.cfg["frame"])
        m = self.menu = tk.Menu(root, tearoff=0)
        m.add_checkbutton(label="Window frame", variable=self.frame_var, command=self.toggle_frame)
        m.add_command(label="Reset position", command=self.reset_position)
        m.add_command(label="Help / details", command=self.show_help)
        m.add_separator()
        m.add_command(label="Quit", command=self.quit)
        for seq, fn in (("<ButtonPress-1>", self._press), ("<B1-Motion>", self._drag),
                        ("<ButtonRelease-1>", self._release), ("<Button-3>", self._menu)):
            c.bind(seq, fn)
        root.protocol("WM_DELETE_WINDOW", self.quit)
        self.place()
        self._apply_frame()
        self.draw()

    @property
    def enabled(self):
        """The sections shown: switched on in the config file and not switched off by a flag."""
        return frozenset(f for f in FEATURES if self.cfg[f] and f not in self.off)

    @property
    def H(self):
        return layout(self.enabled)[1]

    def place(self):
        mons = monitors(self.root.winfo_screen()) or [
            (0, 0, self.root.winfo_screenwidth(), self.root.winfo_screenheight(), True)]
        x, y = choose_position(self.cfg["x"], self.cfg["y"], mons, self.W)
        self.pos = (x, y)
        self.root.geometry("%dx%d+%d+%d" % (self.W, self.H, x, y))

    def _apply_frame(self):
        self.root.withdraw()
        self.root.overrideredirect(not self.cfg["frame"])
        self.root.deiconify()
        self.root.attributes("-topmost", True)

    def toggle_frame(self):
        self.cfg["frame"] = bool(self.frame_var.get())
        self._apply_frame()
        save_config(self.path, self.cfg)

    def reset_position(self):
        self.cfg["x"] = self.cfg["y"] = None
        self.place()
        save_config(self.path, self.cfg)

    def toggle(self, feature):
        """A click on a header switch: flips that section as shown and saves the switch to the config file (a
        flag's override of that switch ends with it). The other switches keep their saved values."""
        self.cfg[feature] = feature not in self.enabled
        self.off.discard(feature)
        self.view.reset(feature)                 # at once: with every section off no poll would clear it
        reset = getattr(self.meter, "reset", None)   # the readers forget it too (a stand-in meter may lack it)
        if reset is not None:
            reset(feature)
        save_config(self.path, self.cfg)
        h = self.H
        self.canvas.config(height=h)
        self.root.geometry("%dx%d" % (self.W, h))   # size only: the top-left corner stays
        self.draw()

    def show_help(self):
        from tkinter import messagebox
        messagebox.showinfo(APP, self.reading.detail or __doc__, parent=self.root)

    def _menu(self, e):
        self.menu.tk_popup(e.x_root, e.y_root)

    def _press(self, e):
        self.drag = (e.x_root - self.pos[0], e.y_root - self.pos[1])
        self.press = (e.x_root, e.y_root)
        self.moved = header_zone(e.x, e.y) is None   # outside the header row a press drags at once, as before

    def _drag(self, e):
        if self.drag:
            if not self.moved and is_click(e.x_root - self.press[0], e.y_root - self.press[1]):
                return                               # still a click on a header switch
            self.moved = True
            self.pos = (e.x_root - self.drag[0], e.y_root - self.drag[1])
            self.root.geometry("+%d+%d" % self.pos)

    def _release(self, e):
        if self.drag:
            if not self.moved and not is_click(e.x_root - self.press[0], e.y_root - self.press[1]):
                self._drag(e)                        # moved 3 px or more without a motion event: a drag
            self.drag = None
            if self.moved:
                self.cfg["x"], self.cfg["y"] = self.pos
                save_config(self.path, self.cfg)
            elif header_zone(e.x, e.y):
                self.toggle(header_zone(e.x, e.y))

    def draw(self):
        """Draws the whole window for the current reading and damage numbers."""
        c, r, en = self.canvas, self.reading, self.enabled
        small, bold, big = ("TkDefaultFont", 8), ("TkDefaultFont", 8, "bold"), ("TkDefaultFont", 11, "bold")
        c.delete("all")
        self.pct = self.status = self.fill = self.hint = None
        for x, text, colour in header_cells(en):
            c.create_text(x, 9, anchor="w", text=text, fill=colour, font=small)
        sections, _ = layout(en)
        for name, y in sections:
            if name == "worm":
                x0, y0, x1, y1 = self.BAR
                frac = r.fraction if r.percent is not None else None
                c.create_text(6, y + 12, anchor="w", text="WORM", fill=LABEL, font=bold)
                c.create_rectangle(x0, y + y0, x1, y + y1, outline="#4a4d50", fill="#2a2c2e")
                self.fill = c.create_rectangle(x0, y + y0, x0 + int((x1 - x0) * (frac or 0.0)), y + y1, width=0,
                                               fill=bar_color(frac))
                self.hint = c.create_text((x0 + x1) // 2, y + 12, text=r.hint if r.percent is None else "",
                                          fill="#b8c0c4", font=("TkDefaultFont", 7))
                self.pct = c.create_text(self.W - 6, y + 12, anchor="e", fill="#f0f0f0", font=big,
                                         text="-" if r.percent is None else "%d%%" % r.percent)
            elif name == "damage_taken":
                c.create_text(6, y + 12, anchor="w", text="TAKEN", fill=LABEL, font=bold)
            elif name == "damage_dealt":
                c.create_text(6, y + 12, anchor="w", text="DEALT", fill=LABEL, font=bold)
                c.create_text(122, y + 12, anchor="e", text="HEALTH", fill=AMBER, font=small)
                c.create_text(174, y + 12, anchor="e", text="SHIELD", fill=TEAL, font=small)
            elif name == "status":
                self.status = c.create_text(6, y + 8, anchor="w", text=r.status, fill="#aab4b8", font=small)
        tops = dict(sections)
        for section, right, top, text, colour in damage_cells(self.view, en):
            # the big font's glyph top at `top`: Tk centres the text, so the anchor is 5 px lower (as the percent)
            c.create_text(right, tops[section] + top + 5, anchor="e", text=text, fill=colour, font=big)

    def show(self, r):
        self.reading = r
        self.view.step(r, self.enabled)
        self.draw()
        if r.detail and r.detail != self.said:
            print(r.detail, file=sys.stderr)
            self.said = r.detail

    def tick(self):
        if self.should_stop():
            return self.quit()
        if self.enabled:                         # every section off: nothing is read
            try:
                r = self.meter.poll(self.enabled)
            except Exception as e:               # keep the window alive; say what happened
                r = Reading("error: %s" % type(e).__name__, detail=repr(e))
            self.show(r)
        self.after_id = self.root.after(POLL_MS, self.tick)

    def quit(self):
        if self.cfg["frame"]:                    # the window manager may have moved the framed window
            try:
                geo = self.root.geometry().split("+")
                self.cfg["x"], self.cfg["y"] = int(geo[1]), int(geo[2])
            except (ValueError, IndexError):
                pass
        elif self.cfg["x"] is not None:
            self.cfg["x"], self.cfg["y"] = self.pos
        save_config(self.path, self.cfg)
        self.root.destroy()

    def run(self):
        self.after_id = self.root.after(0, self.tick)
        self.root.mainloop()


def parse_args(argv):
    """(text_mode, launch_command, features switched off by a flag). Everything after --launch is the game's
    command, untouched."""
    own, cmd = argv, None
    if "--launch" in argv:
        i = argv.index("--launch")
        own, cmd = argv[:i], argv[i + 1:]
        if not cmd:
            raise SystemExit("dehulud-combat: --launch needs the game command; Steam launch options: "
                             + dehulud.launch_option(Meter.program))
    for a in own:
        if a in ("-h", "--help"):
            print(__doc__)
            raise SystemExit(0)
        if a != "--text" and a not in FLAGS:
            if cmd is None:
                raise SystemExit("dehulud-combat: unknown option %r (try --help)" % a)
            print("dehulud-combat: ignoring unknown option %r" % a, file=sys.stderr)   # never block the game
    off = frozenset(FLAGS[a] for a in own if a in FLAGS)
    if "--text" in own and "worm" in off:        # nothing to show, and the worm route must not be read
        if cmd is None:
            raise SystemExit(TEXT_NO_WORM)
        print(TEXT_NO_WORM + " (the game starts without the meter)", file=sys.stderr)   # never block the game
    return "--text" in own, cmd, off


def real_meter(launched=False):
    """The production meter, including the damage readers; tests may inject other readers."""
    return Meter(launched=launched, damage=DamageReader())


def main(argv=None, meter_factory=real_meter, config_file=None):
    text, cmd, off = parse_args(sys.argv[1:] if argv is None else argv)
    child = None
    if cmd:
        try:
            child = subprocess.Popen(cmd)        # the game first: the meter must never stand in its way
        except OSError as e:
            print("dehulud-combat: cannot start %s: %s" % (cmd[0], e), file=sys.stderr)
            return 127
    stop = (lambda: child.poll() is not None) if child else (lambda: False)
    try:
        meter = meter_factory(launched=child is not None)
        tk, display = None, os.environ.get("DISPLAY", "")
        if not text and local_display(display) is None:      # decided before tkinter or Xlib is even loaded
            print("dehulud-combat: %s, using text mode." % (
                  "DISPLAY=%r is not a display on this computer (DeHulud Combat opens no network connection)" % display
                  if display else "no X display (DISPLAY is empty or not set)"), file=sys.stderr)
        elif not text:
            try:
                tk = _import_tk()
            except ImportError:
                print("dehulud-combat: tkinter is missing, using text mode. " + tk_install_hint(), file=sys.stderr)
        window = None
        if tk is not None:
            try:
                window = Window(tk, meter, config_file or config_path(), stop, display, off)
            except tk.TclError as e:
                print("dehulud-combat: no window possible (%s), using text mode." % e, file=sys.stderr)
        if window is not None:
            window.run()
        elif "worm" in off:                      # text mode shows only the worm meter, and it is off: no reads
            if not text:
                print(TEXT_NO_WORM, file=sys.stderr)
            if child is None:
                return 1
        else:
            run_text(meter, should_stop=stop)
    except Exception as e:
        if child is None:
            raise
        print("dehulud-combat: the meter stopped (%r); the game keeps running." % e, file=sys.stderr)
    if child is not None:
        while True:
            try:
                rc = child.wait()
            except KeyboardInterrupt:
                continue
            return 128 - rc if rc < 0 else rc      # killed by signal N: exit status 128 + N, as a shell reports it
    return 0


if __name__ == "__main__":
    sys.exit(main())
