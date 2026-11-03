"""GUI smoke test of DeHulud Combat: builds its real Tk window, but ONLY on an owned, private display.

It never uses $DISPLAY / $WAYLAND_DISPLAY of the session running the tests. It starts its own headless
compositor (kwin_wayland --virtual --xwayland) under a private D-Bus session with private HOME and XDG
directories, runs tests/gui_probe.py inside it, and reads the probe's JSON result. The probe refuses to open
a window on any X display that existed before the compositor started. Skipped when kwin_wayland, Xwayland,
dbus-run-session or tkinter is missing (an Xvfb path is not implemented).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


class CombatGui(unittest.TestCase):
    def test_window_on_owned_display(self):
        missing = [t for t in ("kwin_wayland", "Xwayland", "dbus-run-session") if not shutil.which(t)]
        if missing:
            self.skipTest("no owned display server available (missing: %s)" % ", ".join(missing))
        try:
            import tkinter  # noqa: F401
        except ImportError:
            self.skipTest("this Python has no tkinter")
        tmp = tempfile.mkdtemp(prefix="dh-combat-gui-")
        self.addCleanup(shutil.rmtree, tmp, True)
        dirs = {k: os.path.join(tmp, k) for k in ("home", "run", "cfg", "data", "cache", "state")}
        for d in dirs.values():
            os.mkdir(d, 0o700)
        before = sorted(os.listdir("/tmp/.X11-unix")) if os.path.isdir("/tmp/.X11-unix") else []
        result = os.path.join(tmp, "result.json")
        app = os.path.join(tmp, "app.sh")
        with open(app, "w") as f:
            f.write("#!/bin/sh\nexec %s -B %s %s %s >%s 2>&1\n" % (
                sys.executable, os.path.join(HERE, "combat_gui_probe.py"), result, ",".join(before) or "-",
                os.path.join(tmp, "probe.log")))
        os.chmod(app, 0o700)
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C.UTF-8", "HOME": dirs["home"],
               "XDG_RUNTIME_DIR": dirs["run"], "XDG_CONFIG_HOME": dirs["cfg"], "XDG_DATA_HOME": dirs["data"],
               "XDG_CACHE_HOME": dirs["cache"], "XDG_STATE_HOME": dirs["state"], "TMPDIR": tmp}
        with open(os.path.join(tmp, "kwin.log"), "w") as log:
            subprocess.run(["dbus-run-session", "--", "kwin_wayland", "--virtual", "--xwayland", "--no-lockscreen",
                            "--no-global-shortcuts", "--no-kactivities", "--socket", "dehulud-combat-test", "--width", "1280",
                            "--height", "800", "--exit-with-session", app],
                           env=env, stdout=log, stderr=log, timeout=120, check=False)
        if not os.path.exists(result):
            with open(os.path.join(tmp, "kwin.log")) as k, open(os.path.join(tmp, "probe.log")) as p:
                self.fail("probe produced no result\n" + p.read()[-2000:] + k.read()[-2000:])
        with open(result) as f:
            r = json.load(f)
        self.assertNotIn("refused", r)
        self.assertEqual(r["title"], "DeHulud Combat")
        self.assertTrue(r["screen"].startswith("unix/:"), r["screen"])  # Tk got the Unix-socket-only name
        self.assertEqual(r["topmost"], 1)
        self.assertTrue(r["frameless"])
        self.assertEqual(r["size"], [180, 198])        # all three sections on by default
        self.assertEqual(r["shown"][0], ["42%", "on foot", "#c7df7a", ""])
        self.assertEqual(r["shown"][1], ["-", "Unsupported game version", "#85969d", "PE 1700000000"])
        self.assertEqual(r["shown"][2], ["100%", "vehicle", "#ee7868", ""])
        self.assertEqual(r["shown"][3], ["-", "no permission", "#85969d", "right-click"])
        for boxes in r["boxes"]:                        # the worm section is y 18..42, the status line 178..198
            for name, box in boxes.items():
                if box:
                    lo, hi = (178, 198) if name == "status" else (18, 42)
                    self.assertTrue(0 <= box[0] and box[2] <= 180 and lo <= box[1] and box[3] <= hi, (name, box))
            self.assertGreaterEqual(boxes["pct"][0], 130)  # the percent never covers the bar (x 44..130)
            if boxes["hint"]:
                self.assertTrue(44 <= boxes["hint"][0] and boxes["hint"][2] <= 130, boxes["hint"])
        self.assertEqual(r["help"], "FIX: sudo sysctl")
        click = r["after_click"]                         # Taken switched off: saved, 60 px less, corner kept
        self.assertEqual(click["config"], {"x": None, "y": None, "frame": False, "worm": True, "damage_taken": False, "damage_dealt": True})
        self.assertEqual(click["size"], [180, 138])
        self.assertEqual(click["pos"], [1280 - 180 - 24, 48])
        self.assertEqual(r["after_drag"], {"x": 130, "y": 70, "frame": False, "worm": True, "damage_taken": False, "damage_dealt": True})
        self.assertEqual(r["after_frame"]["frame"], True)
        self.assertEqual(r["after_reset"], {"x": None, "y": None, "frame": True, "worm": True, "damage_taken": False, "damage_dealt": True})
        self.assertTrue(r["destroyed"])


if __name__ == "__main__":
    unittest.main()
