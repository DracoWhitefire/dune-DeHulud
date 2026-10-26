"""Runs INSIDE the owned compositor started by test_gui.py. Builds the real window with a scripted meter.

argv: <result.json> <comma-separated X sockets that existed before the compositor started, or '-'>
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))


def main(result_path, before):
    display = os.environ.get("DISPLAY", "")
    number = display[1:].split(".")[0] if display.startswith(":") else ""
    if not number or "X" + number in before.split(","):     # never the session's own display
        with open(result_path, "w") as f:
            json.dump({"refused": display}, f)
        return 3
    import tkinter
    from tkinter import messagebox
    import dehulud

    readings = iter([dehulud.Reading("on foot", 42, 0.425),
                     dehulud.Reading("Unsupported game version", detail="PE timestamp 1700000000", hint="PE 1700000000"),
                     dehulud.Reading("vehicle", 100, 1.0),
                     dehulud.Reading("no permission", detail="FIX: sudo sysctl", hint=dehulud.MENU_HINT)])

    class Meter:
        def poll(self):
            return next(readings)

    cfg_path = os.path.join(os.environ["XDG_CONFIG_HOME"], "dehulud", "config.json")
    w = dehulud.Window(tkinter, Meter(), cfg_path)
    w.root.update()
    out = {"title": w.root.title(), "screen": w.root.winfo_screen(), "topmost": int(w.root.attributes("-topmost")),
           "frameless": bool(w.root.overrideredirect()), "size": [w.root.winfo_width(), w.root.winfo_height()],
           "shown": []}
    for _ in range(4):
        w.tick()
        w.root.update()
        c = w.canvas
        out["shown"].append([c.itemcget(w.pct, "text"), c.itemcget(w.status, "text"), c.itemcget(w.fill, "fill"),
                             c.itemcget(w.hint, "text")])
        out.setdefault("boxes", []).append({k: c.bbox(getattr(w, k)) for k in ("pct", "status", "hint")})
    shown = []
    messagebox.showinfo = lambda title, message, **kw: shown.append(message)
    w.show_help()
    out["help"] = shown[0] if shown else None

    class Ev:
        def __init__(self, x, y):
            self.x_root, self.y_root = x, y
    x0, y0 = w.pos
    w._press(Ev(x0 + 4, y0 + 2))
    w._drag(Ev(100, 100))
    w._drag(Ev(134, 72))
    w._release(Ev(134, 72))
    out["after_drag"] = dehulud.load_config(cfg_path)
    w.frame_var.set(True)
    w.toggle_frame()
    w.root.update()
    out["after_frame"] = dehulud.load_config(cfg_path)
    w.reset_position()
    out["after_reset"] = dehulud.load_config(cfg_path)
    w.quit()
    try:
        out["destroyed"] = not w.root.winfo_exists()
    except tkinter.TclError:
        out["destroyed"] = True
    with open(result_path, "w") as f:
        json.dump(out, f)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
