"""Shared test support: the fake game child and an injectable /proc source that only knows that child."""
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

import dehulud  # noqa: E402

EXE_PATH = "/games/DUNE Awakening/DuneSandbox/Binaries/Win64/DuneSandbox-Win64-Shipping.exe"


class FakeGame:
    """A spawned child process holding the fake route in its own memory (tests/fake_game.py)."""

    def __init__(self, *extra):
        self.proc = subprocess.Popen(
            [sys.executable, "-B", os.path.join(HERE, "fake_game.py"), "DuneSandbox-Win64-Shipping.exe"] + list(extra),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        info = json.loads(self.proc.stdout.readline())
        self.pid, self.base, self.obj, self.size = self.proc.pid, info["base"], info["obj"], info["size"]
        self.timestamp, self.build = info["timestamp"], info["build"]
        self.damage = info.get("damage")

    def cmd(self, line):
        if line.startswith("damage "):
            frame = json.loads(line.split(" ", 1)[1])
            if "fail_reads" in frame:
                raise ValueError("use damage_frame and apply failure metadata to the consumer reader")
            self.damage_frame(frame)
            return
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        assert self.proc.stdout.readline().strip() == "ok", line

    def damage_frame(self, frame, reader=None):
        """Apply a frame and install returned failure ranges in the consumer reader."""
        self.proc.stdin.write("damage " + json.dumps(frame) + "\n")
        self.proc.stdin.flush()
        reply = json.loads(self.proc.stdout.readline())
        if "error" in reply:
            raise ValueError(reply["error"])
        self.damage = reply["damage"]
        self.base = self.damage.get("image_base", self.base)
        if reader is not None:
            reader.failures = self.damage["failure_ranges"]
            reader.log.clear()
        return self.damage

    def exec_image(self, timestamp):
        """The child execs a fresh fake game in place: same pid and (with --fixed) same addresses, but an image
        whose PE header carries `timestamp`."""
        self.proc.stdin.write("exec %d\n" % timestamp)
        self.proc.stdin.flush()
        info = json.loads(self.proc.stdout.readline())
        assert (info["base"], info["obj"]) == (self.base, self.obj), "the new image moved"

    def end_without_reaping(self):
        """Let the child exit and stay a zombie (pid not reusable until close())."""
        self.proc.stdin.write("exit\n")
        self.proc.stdin.flush()
        os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOWAIT)

    def close(self):
        if self.proc.poll() is None:
            try:
                self.proc.stdin.write("exit\n")
                self.proc.stdin.flush()
            except (BrokenPipeError, ValueError):
                pass
        self.proc.wait(timeout=10)
        for f in (self.proc.stdin, self.proc.stdout):
            try:
                f.close()
            except (BrokenPipeError, ValueError):
                pass


class FakeSource(dehulud.ProcSource):
    """Injectable /proc source: lists ONLY the fake game's pid and synthesizes its maps; uid, cmdline and
    status of that (own) child are read from the real /proc. Never enumerates other processes."""

    def __init__(self, game, scope=0, ppid=None):
        self.game, self.scope, self.fake_ppid = game, scope, ppid
        self.alive = True

    def pids(self):
        return [self.game.pid] if self.alive else []

    def cmdline(self, pid):
        if not self.alive:                      # reaped: its pid may belong to someone else now
            raise FileNotFoundError(2, "No such file or directory")
        return dehulud.ProcSource.cmdline(self, pid)

    def maps(self, pid):
        assert pid == self.game.pid
        b = self.game.base
        return ("55d000000000-55d000021000 r--p 00000000 103:02 77 /usr/bin/wine64-preloader\n"
                "%x-%x r--p 00000000 103:02 1234 %s\n"
                "%x-%x rw-p 00000000 00:00 0 \n"
                "7ffc00000000-7ffc00021000 rw-p 00000000 00:00 0 [stack]\n"
                % (b, b + 0x1000, EXE_PATH, b + 0x1000, b + self.game.size))

    def ppid(self, pid):
        return self.fake_ppid if self.fake_ppid is not None else dehulud.ProcSource.ppid(self, pid)

    def ptrace_scope(self):
        return self.scope


class GameTest(unittest.TestCase):
    """Starts one fake game per test and a Meter bound to it."""
    extra = ()

    def setUp(self):
        self.game = FakeGame(*self.extra)
        self.addCleanup(self.game.close)
        self.source = FakeSource(self.game)
        self.meter = dehulud.Meter(source=self.source, scan_interval=0)

    def poll(self, n=1):
        for _ in range(n):
            r = self.meter.poll()
        return r
