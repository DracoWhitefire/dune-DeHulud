"""DeHulud core tests: route against a fake target process, values, permissions, launch, config, text mode."""
import ast
import errno
import io
import json
import os
import re
import stat
import sys
import tempfile
import unittest
from unittest import mock

from support import ROOT, FakeGame, FakeSource, GameTest, dehulud

SOURCE_FILE = os.path.join(ROOT, "dehulud.py")
COMP, BLOB, THR = "char_comp", "0x334", "0x348"


class Build(GameTest):
    def test_build_26083551_is_recognised(self):
        r = self.poll()
        self.assertEqual(r.status, "on foot")
        self.assertEqual(self.meter.game[2]["build"], "26083551")

    def test_builds_table_identity(self):
        self.assertEqual(sorted(dehulud.BUILDS), [(1792504044, 220008448)])


class UnknownBuild(GameTest):
    extra = ("--timestamp", "1700000000")

    def test_unknown_build_is_unsupported_never_guessed(self):
        r = self.poll()
        self.assertEqual(r.status, "Unsupported game version")
        self.assertIsNone(r.percent)
        self.assertIn("1700000000", r.hint)
        self.assertIn("1700000000", r.detail)
        self.assertIn("26083551", r.detail)
        self.assertEqual(self.meter.reads, 1)           # only the PE header was read
        self.assertIsNone(self.meter.game)
        self.assertIn("Unsupported game version", dehulud.format_line(r))
        self.assertIn("1700000000", dehulud.format_line(r))


class Route(GameTest):
    def test_on_foot(self):
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("on foot", 42))
        self.assertAlmostEqual(r.fraction, 0.425)
        self.assertEqual(self.meter.reads, 1 + 8 + 20)  # header + 8 class slots + the walk
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("on foot", 42))
        self.assertEqual(self.meter.reads, 20)

    def test_vehicle(self):
        self.game.cmd("p pc 0x388 veh")                 # AController.Pawn = the sandbike
        r = self.poll(2)
        self.assertEqual((r.status, r.percent), ("vehicle", 80))
        self.assertEqual(self.meter.reads, 19)
        self.game.cmd("p pc 0x388 char")                # back on foot
        self.assertEqual(self.poll().percent, 42)

    def test_vehicle_component_wins_over_character(self):
        self.game.cmd("p pc 0x388 veh")
        self.game.cmd("f char_comp 0x334 10")
        self.assertEqual(self.poll().percent, 80)

    def test_pawn_null_uses_acknowledged_pawn(self):
        self.game.cmd("p pc 0x388 0")
        r = self.poll(2)
        self.assertEqual((r.status, r.percent), ("on foot", 42))
        self.assertEqual(self.meter.reads, 20)

    def test_pawn_and_acknowledged_pawn_differ(self):
        self.game.cmd("p pc 0x388 lp")                  # a non-vehicle, non-character pawn for a moment
        r = self.poll(2)
        self.assertEqual((r.status, r.percent), ("on foot", 42))
        self.assertEqual(self.meter.reads, 22)

    def test_misaligned_pointer_is_rejected(self):
        self.game.cmd("i pc 0x388 12345")                # low half of the pawn pointer: odd address
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("no data", None))

    def test_local_players_num_zero(self):
        self.poll()
        self.game.cmd("i gi 0x40 0")
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("not in world", None))
        self.assertEqual(self.meter.reads, 6)            # stops at the TArray: Data[0] is never read
        self.game.cmd("i gi 0x40 -1")
        self.assertIsNone(self.poll().percent)

    def test_local_players_data_null(self):
        self.game.cmd("p gi 0x38 0")
        self.assertEqual(self.poll().status, "not in world")

    def test_no_pawn(self):
        self.game.cmd("p pc 0x388 0")
        self.game.cmd("p pc 0x3f0 0")
        self.assertEqual((self.poll().status, self.poll().percent), ("not in world", None))

    def test_null_component(self):
        self.game.cmd("p char 0xe88 0")
        self.assertIsNone(self.poll().percent)

    def assertDash(self, *cmds):
        for c in cmds:
            self.game.cmd(c)
        r = self.poll()
        self.assertIsNone(r.percent, cmds)
        self.assertIsNone(r.fraction, cmds)
        self.assertIn(" - ", dehulud.format_line(r))
        return r

    def test_witness_component_class(self):
        self.assertEqual(self.assertDash("p char 0xe88 bogus").status, "no data")

    def test_witness_world_class(self):
        self.assertDash("p world 0x10 c_bogus")

    def test_witness_game_instance_class(self):
        self.assertDash("p gi 0x10 c_bogus")

    def test_witness_local_player_class(self):
        self.assertDash("p lp 0x10 c_bogus")

    def test_witness_controller_class(self):
        self.assertDash("p pc 0x10 c_lp")

    def test_witness_pawn_not_a_player_character(self):
        self.assertDash("p pc 0x388 lp", "p pc 0x3f0 lp")

    def test_witness_pawn_character_without_player_character(self):
        self.assertDash("p char 0x10 c_char")           # ADuneCharacter but not ADunePlayerCharacter

    def test_witness_vehicle_component_class(self):
        self.assertDash("p pc 0x388 veh", "p veh 0xe08 bogus")

    def test_witness_chain_depth_is_four(self):
        # BP_Bike(0) -> Wheeled(1) -> c_bogus(2) -> c_world(3) -> ADuneVehicle(4): accepted
        for c in ("p c_wheeled 0x40 c_bogus", "p c_bogus 0x40 c_world", "p c_world 0x40 c_veh", "p pc 0x388 veh"):
            self.game.cmd(c)
        self.assertEqual(self.poll().percent, 80)
        # one more level (c_world -> c_lp -> ADuneVehicle = depth 5): refused, and the ack pawn is the same bike
        self.assertDash("p c_world 0x40 c_lp", "p c_lp 0x40 c_veh", "p pc 0x3f0 veh")

    def test_values_outside_the_domain_show_dash(self):
        for blob, thr, why in (("nan", "100", "no value"), ("-1", "100", "no value"), ("inf", "100", "no value"),
                               ("2e6", "100", "no value"), ("42", "0", "no threshold"), ("42", "-5", "no threshold"),
                               ("42", "nan", "no threshold"), ("42", "inf", "no threshold")):
            self.game.cmd("f %s %s %s" % (COMP, BLOB, blob))
            self.game.cmd("f %s %s %s" % (COMP, THR, thr))
            r = self.poll()
            self.assertEqual((r.percent, r.fraction), (None, None), (blob, thr))
            self.assertEqual(r.status, "on foot, " + why, (blob, thr))
            self.assertNotIn("0%", dehulud.format_line(r))

    def test_genuine_zero_is_zero(self):
        self.game.cmd("f %s %s 0" % (COMP, BLOB))
        r = self.poll()
        self.assertEqual((r.percent, r.fraction), (0, 0.0))

    def test_percent_is_floored_and_clamped(self):
        for blob, want, frac in (("29", 29, 0.29), ("99.99", 99, 0.9999), ("100", 100, 1.0), ("150", 100, 1.0),
                                 ("1000", 100, 1.0)):
            self.game.cmd("f %s %s %s" % (COMP, BLOB, blob))
            r = self.poll()
            self.assertEqual(r.percent, want, blob)
            self.assertAlmostEqual(r.fraction, frac, places=5)

    def test_game_exit_between_polls(self):
        self.assertEqual(self.poll().percent, 42)
        self.game.end_without_reaping()
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("waiting for game", None))
        self.assertIsNone(self.meter.game)
        self.source.alive = False
        self.assertEqual(self.poll().status, "waiting for game")

    def test_game_exit_and_reaped(self):
        self.assertEqual(self.poll().percent, 42)
        self.source.alive = False
        self.game.close()
        r = self.poll()
        self.assertEqual((r.status, r.percent), ("waiting for game", None))
        self.assertIsNone(self.meter.game)

    def test_game_exit_mid_poll(self):
        game = self.game

        class Dying(dehulud.Reader):
            def read(self, addr, size):
                if self.count == 12:
                    game.end_without_reaping()
                return dehulud.Reader.read(self, addr, size)

        meter = dehulud.Meter(source=self.source, reader_factory=Dying, scan_interval=0)
        r = meter.poll()
        self.assertEqual((r.status, r.percent), ("waiting for game", None))
        self.assertIsNone(meter.game)

    def test_read_only(self):
        before = dehulud.Reader(self.game.pid).read(self.game.obj["char_comp"], 0x400)
        self.poll(5)
        self.game.cmd("p pc 0x388 veh")
        self.poll(5)
        self.assertEqual(dehulud.Reader(self.game.pid).read(self.game.obj["char_comp"], 0x400), before)


class Denied(dehulud.Reader):
    def read(self, addr, size):
        self.count += 1
        raise OSError(errno.EPERM, "Operation not permitted")


class Permission(GameTest):
    def check(self, scope, launched, *needles):
        self.source.scope = scope
        meter = dehulud.Meter(source=self.source, reader_factory=Denied, launched=launched, scan_interval=0)
        r = meter.poll()
        self.assertEqual((r.status, r.percent), ("no permission", None))
        for n in needles:
            self.assertIn(n, r.detail, (scope, launched))
        self.assertIn("ptrace_scope", r.detail)
        self.assertTrue(r.hint)
        return r

    def test_scope_0(self):
        self.check(0, False, "same user")

    def test_scope_1(self):
        r = self.check(1, False, "--launch %command%", "sudo sysctl kernel.yama.ptrace_scope=0")
        self.assertIn(os.path.realpath(SOURCE_FILE), r.detail)

    def test_scope_1_launched(self):
        self.check(1, True, "sudo sysctl kernel.yama.ptrace_scope=0", "wrapper")

    def test_scope_2(self):
        self.check(2, False, "sudo sysctl kernel.yama.ptrace_scope=1")

    def test_scope_3(self):
        self.check(3, False, "reboot", "/etc/sysctl.d/")

    def test_no_yama(self):
        self.check(None, False, "Yama")

    def test_help_texts_differ_per_scope(self):
        texts = {s: dehulud.permission_help(s, False) for s in (0, 1, 2, 3, None)}
        self.assertEqual(len(set(texts.values())), 5)


class Launch(unittest.TestCase):
    def test_parse_plain(self):
        self.assertEqual(dehulud.parse_args([]), (False, None))
        self.assertEqual(dehulud.parse_args(["--text"]), (True, None))

    def test_parse_launch_keeps_the_command_verbatim(self):
        cmd = ["/steam/reaper", "SteamLaunch", "AppId=1172710", "--", "proton", "waitforexitandrun",
               "DuneSandbox.exe", "--text", "--launch"]
        self.assertEqual(dehulud.parse_args(["--launch"] + cmd), (False, cmd))
        self.assertEqual(dehulud.parse_args(["--text", "--launch"] + cmd), (True, cmd))

    def test_parse_launch_without_command(self):
        with self.assertRaises(SystemExit) as cm:
            dehulud.parse_args(["--launch"])
        self.assertIn("%command%", str(cm.exception.code))

    def test_parse_unknown_option(self):
        with self.assertRaises(SystemExit):
            dehulud.parse_args(["--bogus"])
        with mock.patch("sys.stderr", io.StringIO()):     # never blocks the game launch
            self.assertEqual(dehulud.parse_args(["--bogus", "--launch", "x"]), (False, ["x"]))

    def run_main(self, argv):
        polls = []

        class StubMeter:
            def __init__(self, launched):
                self.launched = launched

            def poll(self):
                polls.append(self.launched)
                return dehulud.Reading("waiting for game")

        out = io.StringIO()
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", io.StringIO()), \
                mock.patch.object(dehulud, "POLL_MS", 20):
            rc = dehulud.main(argv, meter_factory=StubMeter)
        return rc, polls, out.getvalue()

    def test_launch_runs_the_command_and_exits_with_it(self):
        rc, polls, out = self.run_main(["--text", "--launch", sys.executable, "-c",
                                        "import time, sys; time.sleep(0.3); sys.exit(3)"])
        self.assertEqual(rc, 3)
        self.assertTrue(polls and all(polls))           # the meter ran in launched mode until the child ended
        self.assertIn("waiting for game", out)

    def test_launch_of_a_missing_command(self):
        rc, polls, out = self.run_main(["--text", "--launch", "/nonexistent/dune-game"])
        self.assertEqual(rc, 127)

    def test_launched_reads_its_descendant(self):
        game = FakeGame()
        self.addCleanup(game.close)
        meter = dehulud.Meter(source=FakeSource(game), launched=True, scan_interval=0)
        self.assertEqual(meter.poll().percent, 42)

    def test_launched_refuses_a_game_that_is_not_a_descendant(self):
        game = FakeGame()
        self.addCleanup(game.close)
        meter = dehulud.Meter(source=FakeSource(game, scope=1, ppid=1), launched=True, scan_interval=0)
        r = meter.poll()
        self.assertEqual((r.status, r.percent), ("not started by --launch", None))
        self.assertIn(str(game.pid), r.detail)
        self.assertIn("kernel.yama.ptrace_scope=0", r.detail)
        self.assertEqual(meter.reads, 0)                # nothing was read from it


class Text(unittest.TestCase):
    def test_format_line(self):
        self.assertEqual(dehulud.format_line(dehulud.Reading("on foot", 42, 0.425)),
                         "DeHulud  42% [########............] on foot")
        self.assertEqual(dehulud.format_line(dehulud.Reading("waiting for game")),
                         "DeHulud   -  [....................] waiting for game")
        self.assertEqual(dehulud.format_line(dehulud.Reading("vehicle", 100, 1.0)),
                         "DeHulud 100% [####################] vehicle")
        self.assertEqual(dehulud.format_line(dehulud.Reading("no permission", hint=dehulud.MENU_HINT)),
                         "DeHulud   -  [....................] no permission")

    def scripted(self, readings):
        it = iter(readings)

        class M:
            def poll(self):
                r = next(it, None)
                if r is None:
                    raise KeyboardInterrupt
                return r
        return M()

    def test_run_text_pipe_prints_changes_only(self):
        out, err = io.StringIO(), io.StringIO()
        rs = [dehulud.Reading("waiting for game")] * 3 + [dehulud.Reading("on foot", 42, 0.425)] * 2 + \
            [dehulud.Reading("no permission", detail="FIX IT", hint="see help")] * 2
        dehulud.run_text(self.scripted(rs), out=out, err=err, sleep=lambda s: None)
        lines = out.getvalue().splitlines()
        self.assertEqual(len(lines), 3)
        self.assertIn("42%", lines[1])
        self.assertEqual(err.getvalue().count("FIX IT"), 1)

    def test_run_text_tty_redraws_one_line(self):
        class Tty(io.StringIO):
            def isatty(self):
                return True
        out = Tty()
        dehulud.run_text(self.scripted([dehulud.Reading("on foot", 42, 0.425)] * 2), out=out, err=io.StringIO(),
                         sleep=lambda s: None)
        self.assertEqual(out.getvalue().count("\r"), 2)
        self.assertTrue(out.getvalue().endswith("\n"))

    def test_without_tkinter_falls_back_to_text(self):
        out, err = io.StringIO(), io.StringIO()
        meter = self.scripted([dehulud.Reading("on foot", 42, 0.425)])
        with mock.patch.object(dehulud, "_import_tk", side_effect=ImportError("no tkinter")), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err), \
                mock.patch.object(dehulud, "tk_install_hint", return_value="HINT"):
            self.assertEqual(dehulud.main([], meter_factory=lambda launched: meter), 0)
        self.assertIn("42%", out.getvalue())
        self.assertIn("HINT", err.getvalue())

    def test_without_display_falls_back_to_text(self):
        class FakeTk:
            class TclError(Exception):
                pass

            def Tk(self):
                raise self.TclError("no display name and no $DISPLAY environment variable")

        out, err = io.StringIO(), io.StringIO()
        meter = self.scripted([dehulud.Reading("vehicle", 80, 0.8)])
        with mock.patch.object(dehulud, "_import_tk", return_value=FakeTk()), \
                mock.patch("sys.stdout", out), mock.patch("sys.stderr", err):
            rc = dehulud.main([], meter_factory=lambda launched: meter,
                              config_file=os.path.join(tempfile.mkdtemp(), "c.json"))
        self.assertEqual(rc, 0)
        self.assertIn(" 80% ", out.getvalue())
        self.assertIn("no display", err.getvalue())

    def test_tk_install_hints(self):
        cases = {'ID=ubuntu\nID_LIKE=debian': "apt install python3-tk",
                 'ID=linuxmint\nID_LIKE="ubuntu debian"': "apt install python3-tk",
                 'ID=pop\nID_LIKE="ubuntu debian"': "apt install python3-tk",
                 'ID=debian': "apt install python3-tk",
                 'ID=fedora': "dnf install python3-tkinter",
                 'ID=arch': "pacman -S tk",
                 'ID=manjaro\nID_LIKE=arch': "pacman -S tk",
                 'ID=endeavouros\nID_LIKE=arch': "pacman -S tk",
                 'ID="opensuse-tumbleweed"\nID_LIKE="opensuse suse"': "zypper install python3-tk",
                 'ID=steamos\nID_LIKE=arch': "--text",
                 '': "tkinter"}
        for text, want in cases.items():
            self.assertIn(want, dehulud.tk_install_hint(text), text)


class Config(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.path = os.path.join(self.dir, "dehulud", "config.json")

    def write(self, data):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        with open(self.path, "w") as f:
            f.write(data)

    def test_path(self):
        self.assertEqual(dehulud.config_path({"XDG_CONFIG_HOME": "/x/cfg"}), "/x/cfg/dehulud/config.json")
        home = os.path.expanduser("~")
        self.assertEqual(dehulud.config_path({}), os.path.join(home, ".config", "dehulud", "config.json"))
        self.assertEqual(dehulud.config_path({"XDG_CONFIG_HOME": "relative"}),
                         os.path.join(home, ".config", "dehulud", "config.json"))

    def test_missing_and_bad_configs_give_defaults(self):
        defaults = {"x": None, "y": None, "frame": False}
        self.assertEqual(dehulud.load_config(self.path), defaults)
        for bad in ("{", "[]", "null", '{"x": "10", "y": true, "frame": 1}', '{"x": 1e309}', "\xff\xfe",
                    "[" * 100000, '{"x": 99999999999}'):
            self.write(bad)
            self.assertEqual(dehulud.load_config(self.path), defaults, bad[:20])
        self.write('{"x": 10, "y": "a", "frame": true, "extra": 1}')
        self.assertEqual(dehulud.load_config(self.path), {"x": 10, "y": None, "frame": True})

    def test_save_and_load(self):
        self.assertTrue(dehulud.save_config(self.path, {"x": 12, "y": 34, "frame": True}))
        self.assertEqual(dehulud.load_config(self.path), {"x": 12, "y": 34, "frame": True})
        self.assertEqual(stat.S_IMODE(os.stat(os.path.dirname(self.path)).st_mode), 0o700)
        with open(self.path) as f:
            self.assertEqual(json.load(f), {"x": 12, "y": 34, "frame": True})

    def test_save_never_crashes(self):
        blocker = os.path.join(self.dir, "file")
        open(blocker, "w").close()
        self.assertFalse(dehulud.save_config(os.path.join(blocker, "dehulud", "config.json"), {"x": 1}))


class Parsing(unittest.TestCase):
    def test_image_base(self):
        maps = ("100000-200000 r--p 00000000 08:01 5 /x/DuneSandbox-Win64-Shipping.exe.bak\n"
                "140001000-140100000 r-xp 00001000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n"
                "140000000-140001000 r--p 00000000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n"
                "7f0000000000-7f0000001000 r--p 00000000 08:01 9 /g/x/DuneSandbox-Win64-Shipping.exe (deleted)\n"
                "garbage line\n")
        self.assertEqual(dehulud.image_base(maps),
                         (0x140000000, "/g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe"))
        self.assertIsNone(dehulud.image_base("140000000-140001000 r--p 00001000 08:01 9 /g/DuneSandbox-Win64-Shipping.exe\n"))

    def test_pe_identity(self):
        head = bytearray(0x400)
        head[0:2] = b"MZ"
        head[0x3C:0x40] = (0x80).to_bytes(4, "little")
        head[0x80:0x84] = b"PE\0\0"
        head[0x84:0x86] = (0x8664).to_bytes(2, "little")
        head[0x88:0x8C] = (1792504044).to_bytes(4, "little")
        head[0x98:0x9A] = (0x20B).to_bytes(2, "little")
        head[0xD0:0xD4] = (220008448).to_bytes(4, "little")
        self.assertEqual(dehulud.pe_identity(bytes(head)), (1792504044, 220008448))
        for patch in ((0, b"ZM"), (0x80, b"PX"), (0x84, b"\x4c\x01"), (0x98, b"\x0b\x01"), (0x3C, b"\xf0\x03")):
            bad = bytearray(head)
            bad[patch[0]:patch[0] + len(patch[1])] = patch[1]
            self.assertIsNone(dehulud.pe_identity(bytes(bad)), patch)

    def test_bar_colours(self):
        self.assertEqual([dehulud.bar_color(f) for f in (None, 0.0, 0.49, 0.5, 0.74, 0.75, 1.0)],
                         ["#85969d", "#c7df7a", "#c7df7a", "#e8bf70", "#e8bf70", "#ee7868", "#ee7868"])

    def test_reader_errors(self):
        r = dehulud.Reader(os.getpid())
        with self.assertRaises(OSError) as cm:
            r.read(8, 8)                                 # unmapped page 0 of our own process
        self.assertEqual(cm.exception.errno, errno.EFAULT)


class Placement(unittest.TestCase):
    """The window opens on a real monitor, also when the X screen's corner lies on none (mixed layouts)."""
    # Three monitors: a portrait one at the left, the primary 4K one lower in the middle, a 1440p one lower right.
    THREE = [(0, 0, 1440, 2560, False), (1440, 400, 3840, 2160, True), (5280, 1120, 2560, 1440, False)]

    def test_default_is_top_right_of_the_primary_monitor(self):
        self.assertEqual(dehulud.choose_position(None, None, self.THREE, 180), (1440 + 3840 - 180 - 24, 400 + 48))

    def test_old_default_in_the_dead_zone_is_moved_onto_a_monitor(self):
        # 7840 - 180 - 24 = 7636, y 48: inside the X screen but on no monitor of this layout
        self.assertEqual(dehulud.choose_position(7636, 48, self.THREE, 180), (5076, 448))

    def test_saved_position_on_any_monitor_is_kept(self):
        for xy in ((5076, 448), (10, 10), (6000, 1500)):
            self.assertEqual(dehulud.choose_position(*xy, self.THREE, 180), xy)

    def test_without_a_primary_the_first_monitor_is_used(self):
        mons = [(0, 0, 1920, 1080, False), (1920, 0, 1920, 1080, False)]
        self.assertEqual(dehulud.choose_position(None, None, mons, 180), (1920 - 180 - 24, 48))

    def test_single_screen_fallback_matches_the_old_default(self):
        self.assertEqual(dehulud.choose_position(None, None, [(0, 0, 2560, 1440, True)], 180), (2560 - 204, 48))

    def test_monitors_without_a_display_is_empty_not_an_error(self):
        self.assertEqual(dehulud.monitors(":987654"), [])


class SourceSafety(unittest.TestCase):
    """The product file may only READ: no write/attach/signal/inject/network calls anywhere in it."""
    NAMES = {"process_vm_writev", "ptrace", "kill", "killpg", "send_signal", "terminate", "pidfd_send_signal",
             "tgkill", "tkill", "socket", "create_connection", "urlopen", "pwrite", "pwritev", "process_madvise",
             "PTRACE_ATTACH", "PTRACE_SEIZE", "PTRACE_POKEDATA", "PR_SET_PTRACER", "XTestFakeKeyEvent", "system",
             "popen", "execv", "execvp", "fork"}
    MODULES = {"socket", "urllib", "http", "ssl", "requests", "ftplib", "smtplib", "asyncio", "signal", "xmlrpc",
               "telnetlib", "multiprocessing", "webbrowser", "selectors"}
    STRINGS = re.compile(r"process_vm_writev|/mem\b|pagemap|/environ\b|\bptrace\(|https?://|PTRACE_")

    def test_forbidden_calls_absent(self):
        with open(SOURCE_FILE, encoding="utf-8") as f:
            tree = ast.parse(f.read())
        found = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Name) and node.id in self.NAMES:
                found.append(node.id)
            elif isinstance(node, ast.Attribute) and node.attr in self.NAMES:
                found.append(node.attr)
            elif isinstance(node, ast.Import):
                found += [a.name for a in node.names if a.name.split(".")[0] in self.MODULES]
            elif isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[0] in self.MODULES:
                found.append(node.module)
            elif isinstance(node, ast.Constant) and isinstance(node.value, str) and self.STRINGS.search(node.value):
                found.append(node.value[:40])
        self.assertEqual(found, [])

    def test_the_only_memory_call_is_process_vm_readv(self):
        with open(SOURCE_FILE, encoding="utf-8") as f:
            text = f.read()
        self.assertEqual(re.findall(r"process_vm_\w+", text), ["process_vm_readv"] * len(re.findall(r"process_vm_\w+", text)))
        self.assertIn("process_vm_readv", text)


if __name__ == "__main__":
    unittest.main()
