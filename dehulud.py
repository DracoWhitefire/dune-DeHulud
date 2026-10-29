#!/usr/bin/env python3
"""DeHulud - sandworm threat meter for Dune: Awakening on Linux.

Shows how close you are to the sandworm attack threshold, as a percent and a bar, in a tiny
always-on-top window (or one terminal line). It finds the running game (Proton) by itself.

  dehulud.py                    window (falls back to text without tkinter or a local X display)
  dehulud.py --text             one-line meter in the terminal
  dehulud.py --launch %command% Steam launch options: starts the game as a child of DeHulud, so a
                                ptrace_scope of 1 (descendants only) allows the read

Read-only: it reads /proc/<pid>/{cmdline,maps,status} of your own processes, the game's memory with
process_vm_readv, the ptrace setting, /etc/os-release and its own config file. It never writes to the
game, never attaches a debugger, never signals or injects anything, sends no input and opens no network
connection (the window only opens a local X display, through its Unix socket).
Python 3.9+, standard library only.
"""
import collections
import ctypes
import errno
import json
import math
import os
import re
import shlex
import struct
import subprocess
import sys
import time

APP = "DeHulud"
EXE = "DuneSandbox-Win64-Shipping.exe"
POLL_MS = 250          # 4 polls per second
SCAN_S = 2.0           # while no game is attached, look for it every 2 seconds
MAX_DEPTH = 4          # type checks: ClassPrivate, then at most 4 SuperStruct steps
VALUE_MAX = 1000000.0  # accepted range of the two threat values: [0, 1e6]
HEADER = 0x400         # the PE header is identified from the first 0x400 bytes of the image when the game
                       # is found; each poll re-reads its start up to SizeOfImage (see Meter._walk)
MENU_HINT = "right-click"  # window hint: the full message is in the right-click menu (and on stderr)

# Builds table. Key = (PE TimeDateStamp, SizeOfImage) as read from the game's mapped PE header.
# Build 26224975: pe_timestamp 1793030166, size_of_image 219996160 (executable 214886832 bytes).
# Build 26083551: pe_timestamp 1792504044, size_of_image 220008448 (executable 214900656 bytes).
# Offsets come from static analysis of each build's executable; each is commented with the game member it
# reads. "rva" values are relative to the image base; the rest are member offsets (the same in both builds).
BUILDS = {
    (1793030166, 219996160): {
        "build": "26224975",
        "gworld": 0xC4CC568,                     # GWorld slot RVA (holds the UWorld*)
        "cls_world": 0xC4CBF40,                  # UWorld class slot RVA (holds the UClass*)
        "cls_game_instance": 0xC49ABF0,          # UGameInstance class slot RVA
        "cls_local_player": 0xC4AA770,           # ULocalPlayer class slot RVA
        "cls_player_controller": 0xC4B8128,      # APlayerController class slot RVA
        "cls_dune_character": 0xC163440,         # ADuneCharacter class slot RVA
        "cls_dune_player_character": 0xC16C2A0,  # ADunePlayerCharacter class slot RVA
        "cls_dune_vehicle": 0xC1E0D78,           # ADuneVehicle class slot RVA
        "cls_threat_component": 0xC1D5BE8,       # USandwormThreatSourceComponent class slot RVA
        "class_private": 0x10,                   # UObject.ClassPrivate
        "super_struct": 0x40,                    # UStruct.SuperStruct
        "world_game_instance": 0x3C8,            # UWorld.OwningGameInstance
        "local_players": 0x38,                   # UGameInstance.LocalPlayers (TArray)
        "array_data": 0x0,                       # TArray.Data (element stride 8)
        "array_num": 0x8,                        # TArray.Num (i32)
        "player_controller": 0x30,               # ULocalPlayer.PlayerController
        "pawn": 0x388,                           # AController.Pawn (the possessed pawn: decides the route)
        "acknowledged_pawn": 0x3F0,              # APlayerController.AcknowledgedPawn (on foot: must equal Pawn)
        "character_threat": 0xE88,               # ADuneCharacter.m_SandwormThreatSourceComponent
        "vehicle_threat": 0xE08,                 # ADuneVehicle.m_SandwormThreatSourceComponent
        "threat_blob": 0x334,                    # m_CurrentThreatBlobValue (f32)
        "threat_threshold": 0x348,               # m_CurrentSandwormAttackThreshold (f32)
    },
    (1792504044, 220008448): {
        "build": "26083551",
        "gworld": 0xC4CF528,                     # GWorld slot RVA (holds the UWorld*)
        "cls_world": 0xC4CEF00,                  # UWorld class slot RVA (holds the UClass*)
        "cls_game_instance": 0xC49DBB0,          # UGameInstance class slot RVA
        "cls_local_player": 0xC4AD730,           # ULocalPlayer class slot RVA
        "cls_player_controller": 0xC4BB0E8,      # APlayerController class slot RVA
        "cls_dune_character": 0xC166430,         # ADuneCharacter class slot RVA
        "cls_dune_player_character": 0xC16F290,  # ADunePlayerCharacter class slot RVA
        "cls_dune_vehicle": 0xC1E3D68,           # ADuneVehicle class slot RVA
        "cls_threat_component": 0xC1D8BD8,       # USandwormThreatSourceComponent class slot RVA
        "class_private": 0x10,                   # UObject.ClassPrivate
        "super_struct": 0x40,                    # UStruct.SuperStruct
        "world_game_instance": 0x3C8,            # UWorld.OwningGameInstance
        "local_players": 0x38,                   # UGameInstance.LocalPlayers (TArray)
        "array_data": 0x0,                       # TArray.Data (element stride 8)
        "array_num": 0x8,                        # TArray.Num (i32)
        "player_controller": 0x30,               # ULocalPlayer.PlayerController
        "pawn": 0x388,                           # AController.Pawn (the possessed pawn: decides the route)
        "acknowledged_pawn": 0x3F0,              # APlayerController.AcknowledgedPawn (on foot: must equal Pawn)
        "character_threat": 0xE88,               # ADuneCharacter.m_SandwormThreatSourceComponent
        "vehicle_threat": 0xE08,                 # ADuneVehicle.m_SandwormThreatSourceComponent
        "threat_blob": 0x334,                    # m_CurrentThreatBlobValue (f32)
        "threat_threshold": 0x348,               # m_CurrentSandwormAttackThreshold (f32)
    },
}
CLASS_SLOTS = ("cls_world", "cls_game_instance", "cls_local_player", "cls_player_controller", "cls_dune_character",
               "cls_dune_player_character", "cls_dune_vehicle", "cls_threat_component")
CHARACTER = {"cls_dune_character", "cls_dune_player_character"}   # on foot: the pawn must be BOTH

Reading = collections.namedtuple("Reading", "status percent fraction detail hint", defaults=(None, None, "", ""))
Reading.__doc__ = "One poll: percent/fraction are None whenever the meter must show '-'."


class _IoVec(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("len", ctypes.c_size_t)]


_readv = None


def _process_vm_readv():
    global _readv
    if _readv is None:
        f = ctypes.CDLL(None, use_errno=True).process_vm_readv
        f.argtypes = [ctypes.c_int, ctypes.POINTER(_IoVec), ctypes.c_ulong, ctypes.POINTER(_IoVec),
                      ctypes.c_ulong, ctypes.c_ulong]
        f.restype = ctypes.c_ssize_t
        _readv = f
    return _readv


class Reader:
    """Copies bytes out of another process with process_vm_readv: one call per read, never a write."""

    def __init__(self, pid):
        self.pid, self.count = pid, 0

    def read(self, addr, size):
        if not 0 < addr < (1 << 47) - size:
            raise OSError(errno.EFAULT, "address outside user space")
        self.count += 1
        buf = ctypes.create_string_buffer(size)
        local, remote = _IoVec(ctypes.addressof(buf), size), _IoVec(addr, size)
        n = _process_vm_readv()(self.pid, ctypes.byref(local), 1, ctypes.byref(remote), 1, 0)
        if n != size:
            code = ctypes.get_errno() if n < 0 else errno.EFAULT
            raise OSError(code, os.strerror(code))
        return buf.raw


class ProcSource:
    """Everything DeHulud looks at outside the game's memory (tests inject a subclass)."""

    def pids(self):
        return sorted(int(n) for n in os.listdir("/proc") if n.isdigit())

    def uid(self, pid):
        return os.stat("/proc/%d" % pid).st_uid

    def _read(self, pid, name):                  # name is one of cmdline, maps, status
        with open("/proc/%d/%s" % (pid, name), "rb") as f:
            return f.read()

    def cmdline(self, pid):
        return self._read(pid, "cmdline")

    def maps(self, pid):
        return self._read(pid, "maps").decode("utf-8", "replace")

    def ppid(self, pid):
        for line in self._read(pid, "status").decode("utf-8", "replace").splitlines():
            if line.startswith("PPid:"):
                return int(line.split()[1])
        return 0

    def ptrace_scope(self):
        try:
            with open("/proc/sys/kernel/yama/ptrace_scope") as f:
                return int(f.read().strip())
        except (OSError, ValueError):
            return None


def image_base(maps_text):
    """(address, path) of the lowest mapping of a file named EXE at file offset 0, or None."""
    best = None
    for line in maps_text.splitlines():
        parts = line.split(None, 5)
        if len(parts) < 6:
            continue
        path = parts[5][:-10] if parts[5].endswith(" (deleted)") else parts[5]
        try:
            start, offset = int(parts[0].split("-")[0], 16), int(parts[2], 16)
        except ValueError:
            continue
        if offset == 0 and path.rsplit("/", 1)[-1] == EXE and (best is None or start < best[0]):
            best = (start, path)
    return best


def pe_identity(head):
    """(TimeDateStamp, SizeOfImage) of a PE32+ x64 header, or None if it is not one."""
    if len(head) < 0x40 or head[:2] != b"MZ":
        return None
    pe = struct.unpack_from("<I", head, 0x3C)[0]
    if pe + 0x58 > len(head) or head[pe:pe + 4] != b"PE\0\0":
        return None
    machine, = struct.unpack_from("<H", head, pe + 4)
    magic, = struct.unpack_from("<H", head, pe + 24)
    if machine != 0x8664 or magic != 0x20B:
        return None
    return struct.unpack_from("<I", head, pe + 8)[0], struct.unpack_from("<I", head, pe + 24 + 56)[0]


def reading_for(mode, blob, threshold):
    """meter = clamp(blob / threshold, 0, 1); '-' for anything outside the accepted range."""
    if not (math.isfinite(blob) and 0.0 <= blob <= VALUE_MAX):
        return Reading(mode + ", no value")
    if not (math.isfinite(threshold) and 0.0 < threshold <= VALUE_MAX):
        return Reading(mode + ", no threshold")
    percent = min(100, max(0, math.floor(blob * 100.0 / threshold)))  # whole percent, floored, clamped
    return Reading(mode, percent, min(1.0, max(0.0, blob / threshold)))


def launch_option():
    """The Steam launch option for this file. The path is quoted for the shell: a path with a space or a quote
    in it would otherwise be split into several words and the game would not start."""
    return shlex.quote(os.path.realpath(__file__)) + " --launch %command%"


def permission_help(scope, launched):
    """The exact fix for the user's ptrace_scope value (kernel.yama.ptrace_scope). Yama's values: 0 adds no
    restriction of its own (the normal same-user rules still apply), 1 allows a program to read only its own
    descendants, 2 only programs with CAP_SYS_PTRACE, 3 nobody, and 3 cannot be lowered until a reboot."""
    head = "DeHulud was refused permission to read the game's memory (ptrace_scope is %s).\n" % (
        "not readable" if scope is None else scope)
    if scope == 0:
        fix = ("ptrace_scope 0 adds no restriction of its own, but the normal rules still apply: the game must run as "
               "the same user as DeHulud and not in a sandbox (Flatpak Steam), must not have made itself unreadable, "
               "and no security module may block it (SELinux: 'getsebool deny_ptrace' must be off).")
    elif scope == 1 and launched:
        fix = ("With 1 a program may read its own descendants, and DeHulud started the game through the --launch "
               "wrapper, but it is still not readable: your Steam setup probably runs it in a container or sandbox "
               "that prevents it. Use instead (until reboot):\n  sudo sysctl kernel.yama.ptrace_scope=0\n"
               "and start DeHulud by itself, without the launch option.")
    elif scope == 1:
        fix = ("With 1 a program may read only its own descendants (the programs it started, and theirs). "
               "Fix, pick one:\n"
               "  1. Steam > Dune: Awakening > Properties > Launch Options:\n"
               "       %s\n"
               "  2. Allow it until the next reboot:  sudo sysctl kernel.yama.ptrace_scope=0\n"
               "     (trade-off: then every program running as your user can read your other programs)"
               % launch_option())
    elif scope == 2:
        fix = ("With 2 only a program with the CAP_SYS_PTRACE capability (normally only root) may read another "
               "program. Lower it to 1 and use the Steam launch option (%s), or to 0:\n"
               "  sudo sysctl kernel.yama.ptrace_scope=1" % launch_option())
    elif scope == 3:
        fix = ("With 3 reading other programs is switched off, and 3 cannot be lowered while the system runs. "
               "Change kernel.yama.ptrace_scope in the file under /etc/sysctl.d/ that sets 3, then reboot.")
    else:
        fix = ("The Yama setting /proc/sys/kernel/yama/ptrace_scope could not be read: Yama is not active, or the "
               "setting is hidden (a container). Something else refuses the read: another user, a sandbox (Flatpak) "
               "or a security module (SELinux 'deny_ptrace', AppArmor).")
    return head + fix + "\nDetails: README.md, 'Before you start: permission to read the game'."


def outside_launch_help(pid, scope):
    """--launch mode, but the game is not a descendant of DeHulud: what works for this ptrace_scope value."""
    head = ("Steam started the game outside DeHulud (pid %d is not a descendant of this --launch DeHulud), so the "
            "launch option cannot help, and ptrace_scope is %s.\n" % (pid, scope))
    trade = ("Trade-off: with 0, every program running as your user can read the memory of your other programs "
             "(browser, password manager).")
    if scope == 3:
        return head + ("3 cannot be lowered while the system runs: change kernel.yama.ptrace_scope to 0 in the "
                       "file under /etc/sysctl.d/ that sets 3 (README, 'Change it permanently'), reboot, remove "
                       "the launch option in Steam and start DeHulud by itself. " + trade)
    why = ("With 2, only a program with the CAP_SYS_PTRACE capability (normally only root) may read another "
           "program, even one it started.\n" if scope == 2 else
           "With 1, a program may read only its own descendants (the programs it started, and theirs).\n")
    return head + why + ("What works: allow reading with\n"
                         "  sudo sysctl kernel.yama.ptrace_scope=0      (until the next reboot)\n"
                         "then remove the launch option in Steam and start DeHulud by itself:  %s\n%s"
                         % (shlex.quote(os.path.realpath(__file__)), trade))


OUTSIDE = "DeHulud found the game outside --launch; reading it anyway (pid %d, ptrace_scope %s)."
UNSUPPORTED = ("Unsupported game version: PE timestamp %d (%s UTC), SizeOfImage %d.\nThis DeHulud knows: %s.\n"
               "After a game update DeHulud shows '-' until a release with the new build's offsets exists.")


class _Soft(Exception):
    """This poll shows '-' with the given status; the game stays attached."""


class Meter:
    """Finds the game, identifies its build and walks the threat route once per poll()."""

    def __init__(self, source=None, reader_factory=Reader, launched=False, scan_interval=SCAN_S,
                 clock=time.monotonic):
        self.source, self.reader_factory, self.launched = source or ProcSource(), reader_factory, launched
        self.scan_interval, self.clock = scan_interval, clock
        self.game = None        # [pid, image base, build entry, reader, class slot values or None,
                                #  PE identity, header bytes re-read each poll]
        self.note = ""          # said with every reading of this attachment (a game found outside --launch)
        self.next_scan, self.reads, self.last = 0.0, 0, Reading("waiting for game")

    def poll(self):
        r = self._poll()
        return r._replace(detail=self.note) if self.note and self.game is not None and not r.detail else r

    def _poll(self):
        self.reads = 0
        if self.game is None and self.clock() < self.next_scan:
            return self.last
        for _ in range(2):           # a second round only when the attached process now runs another image
            if self.game is None:
                self.next_scan = self.clock() + self.scan_interval
                self.last = self._attach()
                if self.game is None:
                    return self.last
            reader = self.game[3]
            start = reader.count
            try:
                self.last = self._walk()
            finally:
                self.reads += reader.count - start
            if self.last is not None:
                return self.last
        self.last = Reading("no data")
        return self.last

    def _descends(self, pid):
        """Whether pid descends from DeHulud. Only the user's own processes are read: the walk stops at the first
        ancestor that belongs to another user, before its status is opened."""
        me, uid = os.getpid(), os.getuid()
        for _ in range(64):
            try:
                if self.source.uid(pid) != uid:
                    return False
                pid = self.source.ppid(pid)
            except (OSError, ValueError):
                return False
            if pid == me:
                return True
            if pid <= 1:
                return False
        return False

    def _attach(self):
        src, found, foreign = self.source, [], []
        for pid in src.pids():
            try:
                if pid == os.getpid() or src.uid(pid) != os.getuid() or EXE.encode() not in src.cmdline(pid):
                    continue
                hit = image_base(src.maps(pid))
            except (OSError, ValueError):
                continue
            if hit and self.launched and not self._descends(pid):
                foreign.append((pid, hit[0]))
            elif hit:
                found.append((pid, hit[0]))
        self.note = ""
        if not found and foreign:
            # Steam started the game outside this --launch DeHulud. With ptrace_scope 0 the kernel allows the
            # read anyway (so it is tried; without a readable setting too: a refusal then says why). With 1-3 a
            # read would be refused, so nothing is read and the message says what works for that value.
            scope = src.ptrace_scope()
            if scope not in (0, None):
                return Reading("not started by --launch", detail=outside_launch_help(foreign[0][0], scope),
                               hint=MENU_HINT)
            found, self.note = foreign, OUTSIDE % (foreign[0][0], "0" if scope == 0 else "not readable")
        if not found:
            return Reading("waiting for game")
        pid, base = found[0]
        reader = self.reader_factory(pid)
        try:
            head = reader.read(base, HEADER)
            ident = pe_identity(head)
        except OSError as e:
            return self._failed(e)
        finally:
            self.reads += reader.count
        if ident is None:
            return Reading("waiting for game")
        if ident not in BUILDS:
            known = ", ".join("build %s (PE timestamp %d)" % (b["build"], k[0]) for k, b in BUILDS.items())
            when = time.strftime("%Y-%m-%d %H:%M", time.gmtime(ident[0]))
            return Reading("Unsupported game version", detail=UNSUPPORTED % (ident[0], when, ident[1], known),
                           hint="PE %d" % ident[0])
        span = struct.unpack_from("<I", head, 0x3C)[0] + 0x58      # e_lfanew + what pe_identity checks
        self.game = [pid, base, BUILDS[ident], reader, None, ident, span]
        return None

    def _failed(self, e):
        if e.errno in (errno.EPERM, errno.EACCES):
            self.game = None
            return Reading("no permission", detail=permission_help(self.source.ptrace_scope(), self.launched),
                           hint=MENU_HINT)
        if e.errno == errno.ESRCH:
            self.game, self.next_scan = None, 0.0
            return Reading("waiting for game")
        return Reading("no data")                # EFAULT/EIO: a pointer went stale during this poll

    def _walk(self):
        """One poll of the attached game; None when the process now runs another image (identify it again)."""
        pid, base, b, r, slots, ident, span = self.game
        try:
            alive = EXE.encode() in self.source.cmdline(pid)     # exited, reaped or pid reused: detach
        except OSError:
            alive = False
        if not alive:
            return self._failed(OSError(errno.ESRCH, "game exited"))
        # The build and the cached class pointers belong to the image identified when the game was found. One
        # read per poll re-checks that image's PE TimeDateStamp and SizeOfImage, so that an exec into another
        # image under the same pid (at the same addresses) is never read with the old build's offsets.
        try:
            same = pe_identity(r.read(base, span)) == ident
        except OSError as e:
            if e.errno in (errno.EPERM, errno.EACCES, errno.ESRCH):
                return self._failed(e)
            same = False                                          # the header is gone: not that image
        if not same:
            self.game, self.next_scan = None, 0.0
            return None
        try:
            return self._route(base, b, r, slots)
        except _Soft as s:
            return Reading(str(s))
        except OSError as e:
            return self._failed(e)

    def _route(self, base, b, r, slots):
        def ptr(addr):
            value, = struct.unpack("<Q", r.read(addr, 8))
            if value % 8 or value >= 1 << 47:
                raise _Soft("no data")                          # not an object pointer
            return value

        def obj(addr):
            value = ptr(addr)
            if not value:
                raise _Soft("not in world")
            return value

        def chain(o, wanted, done):
            """Class slots hit by ClassPrivate and up to MAX_DEPTH SuperStruct steps."""
            hits, cls = set(), ptr(o + b["class_private"])
            for depth in range(MAX_DEPTH + 1):
                if not cls:
                    break
                hits.update(k for k in wanted if slots[k] == cls)
                if done(hits) or depth == MAX_DEPTH:
                    break
                cls = ptr(cls + b["super_struct"])
            return hits

        def witness(o, key):
            if key not in chain(o, (key,), bool):
                raise _Soft("no data")

        if slots is None:            # UClass* slots are written once at startup; read until all are set
            slots = {k: ptr(base + b[k]) for k in CLASS_SLOTS}
            if not all(slots.values()):
                raise _Soft("waiting for game")
            self.game[4] = slots
        world = obj(base + b["gworld"])
        if ptr(world + b["class_private"]) != slots["cls_world"]:             # UWorld: exact class
            raise _Soft("no data")
        gi = obj(world + b["world_game_instance"])
        witness(gi, "cls_game_instance")                                      # type check: UGameInstance
        arr = r.read(gi + b["local_players"], 12)                             # LocalPlayers: Data + Num, one read
        data, = struct.unpack_from("<Q", arr, b["array_data"])
        num, = struct.unpack_from("<i", arr, b["array_num"])
        if num < 1 or not data:                                               # Num checked before Data[0]
            raise _Soft("not in world")
        if data % 8:
            raise _Soft("no data")
        lp = obj(data)                                                        # element 0 (stride 8)
        witness(lp, "cls_local_player")                                       # type check: ULocalPlayer
        pc = obj(lp + b["player_controller"])
        witness(pc, "cls_player_controller")                                  # type check: APlayerController
        # The possessed Pawn alone decides whose threat is shown: the vehicle's when it passes the ADuneVehicle
        # check, the character's when it passes both character checks and is also the AcknowledgedPawn.
        # Anything else shows '-'; there is no fallback to another object.
        pawn = obj(pc + b["pawn"])                                            # AController.Pawn
        hits = chain(pawn, ("cls_dune_vehicle",) + tuple(CHARACTER),
                     lambda h: "cls_dune_vehicle" in h or CHARACTER <= h)
        if "cls_dune_vehicle" in hits:                                        # type check: ADuneVehicle
            field, mode = b["vehicle_threat"], "vehicle"
        elif CHARACTER <= hits and ptr(pc + b["acknowledged_pawn"]) == pawn:  # both character checks, same object
            field, mode = b["character_threat"], "on foot"
        else:
            raise _Soft("no data")
        comp = obj(pawn + field)
        # Type check: USandwormThreatSourceComponent or a subclass within 4 SuperStruct steps. One read when
        # the class is the native one, and the two value offsets hold for any subclass.
        witness(comp, "cls_threat_component")
        gap = b["threat_threshold"] - b["threat_blob"]
        raw = r.read(comp + b["threat_blob"], gap + 4)                        # both f32 leaves, one read
        blob, = struct.unpack_from("<f", raw, 0)
        threshold, = struct.unpack_from("<f", raw, gap)
        return reading_for(mode, blob, threshold)


def format_line(r, width=20):
    """One terminal line: name, percent (or '-'), bar, status."""
    n = int(r.fraction * width) if r.percent is not None else 0
    pct = "%3d%%" % r.percent if r.percent is not None else "  - "
    tail = " (%s)" % r.hint if r.hint and r.hint != MENU_HINT and r.percent is None else ""
    return "%s %s [%s%s] %s%s" % (APP, pct, "#" * n, "." * (width - n), r.status, tail)


def run_text(meter, out=None, err=None, should_stop=lambda: False, sleep=time.sleep):
    """Text meter: redraws one line on a terminal, prints changed lines otherwise. Ctrl-C ends it."""
    out, err = out or sys.stdout, err or sys.stderr
    tty, last, said = out.isatty(), None, None
    try:
        while not should_stop():
            r = meter.poll()
            if r.detail and r.detail != said:
                err.write("\n" + r.detail + "\n" if tty else r.detail + "\n")
                said = r.detail
            line = format_line(r)
            if tty:
                out.write("\r" + line.ljust(78))
            elif line != last:
                out.write(line + "\n")
            out.flush()
            last = line
            sleep(POLL_MS / 1000.0)
    except KeyboardInterrupt:
        pass
    if tty:
        out.write("\n")


def bar_color(fraction):
    """Green below 50 %, amber below 75 %, red from 75 %; grey without a value."""
    if fraction is None:
        return "#85969d"
    return "#c7df7a" if fraction < 0.5 else "#e8bf70" if fraction < 0.75 else "#ee7868"


DEFAULTS = {"x": None, "y": None, "frame": False}


def config_path(env=None):
    """$XDG_CONFIG_HOME/dehulud/config.json; $HOME/.config when XDG_CONFIG_HOME is unset or not absolute. None
    when neither is known: the window then keeps no position. There is deliberately no password-database
    fallback (os.path.expanduser does one without HOME): that lookup can ask a local name service over a
    socket (nscd, systemd-userdb), and the meter opens no socket other than its X display."""
    env = os.environ if env is None else env
    base = env.get("XDG_CONFIG_HOME", "")
    if not os.path.isabs(base):
        home = env.get("HOME")
        if home is None:
            return None
        base = os.path.join(home.rstrip("/") or "/", ".config")
    return os.path.join(base, "dehulud", "config.json")


def load_config(path):
    """Window position and frame flag; anything missing or malformed falls back to the defaults."""
    cfg = dict(DEFAULTS)
    if path is None:
        return cfg
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError, RecursionError):
        return cfg
    if isinstance(data, dict):
        for k in ("x", "y"):
            v = data.get(k)
            if isinstance(v, int) and not isinstance(v, bool) and -100000 < v < 100000:
                cfg[k] = v
        if isinstance(data.get("frame"), bool):
            cfg["frame"] = data["frame"]
    return cfg


def save_config(path, cfg):
    if path is None:
        return False
    try:
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        with open(path + ".tmp", "w", encoding="utf-8") as f:
            json.dump({k: cfg.get(k, DEFAULTS[k]) for k in DEFAULTS}, f)
        os.replace(path + ".tmp", path)
        return True
    except (OSError, TypeError, ValueError):
        return False


def local_display(name):
    """The X display `name` as 'unix/:N[.S]' when it is a display on this computer, else None.

    Accepted: ':N', ':N.S', 'unix:N', 'unix:N.S' (and 'unix/:N[.S]'). Everything with a host part is refused
    ('192.0.2.1:12', 'localhost:10' as set by ssh X forwarding, 'host/unix:0'), as are socket paths: the window
    must never open a network connection. Xlib (libxcb) is handed the 'unix/' protocol form, with which it
    uses only the Unix socket of display N: for a plain ':N' libxcb falls back to TCP on localhost port 6000+N
    when that socket cannot be reached, and libxcb 1.16 and later read 'unix:N' as the path of a socket file
    named 'N'."""
    m = re.fullmatch(r"(?:unix/?)?:([0-9]+)(\.[0-9]+)?", name or "")
    return "unix/:%s%s" % (m.group(1), m.group(2) or "") if m else None


class _MonitorInfo(ctypes.Structure):
    """XRRMonitorInfo (Xrandr 1.5)."""
    _fields_ = [("name", ctypes.c_ulong), ("primary", ctypes.c_int), ("automatic", ctypes.c_int),
                ("noutput", ctypes.c_int), ("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int),
                ("height", ctypes.c_int), ("mwidth", ctypes.c_int), ("mheight", ctypes.c_int),
                ("outputs", ctypes.c_void_p)]


def monitors(display_name):
    """The X screen's monitors as [(x, y, width, height, primary)], or [] when XRandR is unavailable or the
    display is not a local one (see local_display).

    The X screen of a multi-monitor desktop is the bounding box of all monitors, so a corner of it can lie on
    no monitor at all; the window is placed on a real monitor instead."""
    display_name = local_display(display_name)
    if display_name is None:
        return []
    try:
        x11, xrr = ctypes.CDLL("libX11.so.6"), ctypes.CDLL("libXrandr.so.2")
        x11.XOpenDisplay.restype, x11.XOpenDisplay.argtypes = ctypes.c_void_p, [ctypes.c_char_p]
        x11.XDefaultRootWindow.restype, x11.XDefaultRootWindow.argtypes = ctypes.c_ulong, [ctypes.c_void_p]
        x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        xrr.XRRGetMonitors.restype = ctypes.POINTER(_MonitorInfo)
        xrr.XRRGetMonitors.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.POINTER(ctypes.c_int)]
        xrr.XRRFreeMonitors.argtypes = [ctypes.POINTER(_MonitorInfo)]
    except (OSError, AttributeError):        # no library, or a libXrandr older than 1.5 (no monitor list)
        return []
    dpy = x11.XOpenDisplay(display_name.encode())
    if not dpy:
        return []
    try:
        n = ctypes.c_int(0)
        info = xrr.XRRGetMonitors(dpy, x11.XDefaultRootWindow(dpy), 1, ctypes.byref(n))
        if not info:
            return []
        try:
            return [(m.x, m.y, m.width, m.height, bool(m.primary)) for m in info[:n.value]
                    if m.width > 0 and m.height > 0]
        finally:
            xrr.XRRFreeMonitors(info)
    finally:
        x11.XCloseDisplay(dpy)


def choose_position(x, y, mons, width):
    """Keep a saved position whose corner lies on a monitor; otherwise the top-right of the primary monitor."""
    if x is not None and y is not None:
        for mx, my, mw, mh, _ in mons:
            if mx <= x <= mx + mw - 20 and my <= y <= my + mh - 20:
                return x, y
    mx, my, mw, mh, _ = next((m for m in mons if m[4]), mons[0])
    return mx + mw - width - 24, my + 48


class Window:
    """180 x 44 always-on-top window: drag to move, right-click for the menu."""
    W, H, BAR = 180, 44, (44, 6, 130, 18)

    def __init__(self, tk, meter, config_file, should_stop=lambda: False, display=None):
        """`display` defaults to $DISPLAY; only a local display is opened (see local_display)."""
        self.tk, self.meter, self.path, self.should_stop = tk, meter, config_file, should_stop
        self.cfg, self.drag, self.after_id, self.said = load_config(config_file), None, None, None
        self.reading = Reading("waiting for game")
        name = os.environ.get("DISPLAY", "") if display is None else display
        screen = local_display(name)
        if screen is None:
            raise tk.TclError("not a display on this computer: %r" % name)
        root = self.root = tk.Tk(screenName=screen)
        root.title(APP)
        root.resizable(False, False)
        c = self.canvas = tk.Canvas(root, width=self.W, height=self.H, bg="#1d1f21", highlightthickness=0)
        c.pack()
        x0, y0, x1, y1 = self.BAR
        c.create_text(6, 12, anchor="w", text="WORM", fill="#d8cfb4", font=("TkDefaultFont", 8, "bold"))
        c.create_rectangle(x0, y0, x1, y1, outline="#4a4d50", fill="#2a2c2e")
        self.fill = c.create_rectangle(x0, y0, x0, y1, width=0, fill=bar_color(None))
        self.hint = c.create_text((x0 + x1) // 2, 12, text="", fill="#b8c0c4", font=("TkDefaultFont", 7))
        self.pct = c.create_text(self.W - 6, 12, anchor="e", text="-", fill="#f0f0f0",
                                 font=("TkDefaultFont", 11, "bold"))
        self.status = c.create_text(6, 32, anchor="w", text=self.reading.status, fill="#aab4b8",
                                    font=("TkDefaultFont", 8))
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

    def show_help(self):
        from tkinter import messagebox
        messagebox.showinfo(APP, self.reading.detail or __doc__, parent=self.root)

    def _menu(self, e):
        self.menu.tk_popup(e.x_root, e.y_root)

    def _press(self, e):
        self.drag = (e.x_root - self.pos[0], e.y_root - self.pos[1])

    def _drag(self, e):
        if self.drag:
            self.pos = (e.x_root - self.drag[0], e.y_root - self.drag[1])
            self.root.geometry("+%d+%d" % self.pos)

    def _release(self, e):
        if self.drag:
            self.drag = None
            self.cfg["x"], self.cfg["y"] = self.pos
            save_config(self.path, self.cfg)

    def show(self, r):
        self.reading, c = r, self.canvas
        x0, y0, x1, y1 = self.BAR
        frac = r.fraction if r.percent is not None else None
        c.coords(self.fill, x0, y0, x0 + int((x1 - x0) * (frac or 0.0)), y1)
        c.itemconfigure(self.fill, fill=bar_color(frac))
        c.itemconfigure(self.pct, text="-" if r.percent is None else "%d%%" % r.percent)
        c.itemconfigure(self.hint, text=r.hint if r.percent is None else "")
        c.itemconfigure(self.status, text=r.status)
        if r.detail and r.detail != self.said:
            print(r.detail, file=sys.stderr)
            self.said = r.detail

    def tick(self):
        if self.should_stop():
            return self.quit()
        try:
            r = self.meter.poll()
        except Exception as e:                   # keep the window alive; say what happened
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


def tk_install_hint(os_release=None):
    """How to install tkinter on the user's distribution (from /etc/os-release ID / ID_LIKE)."""
    if os_release is None:
        try:
            with open("/etc/os-release", encoding="utf-8") as f:
                os_release = f.read()
        except OSError:
            os_release = ""
    ids = set()
    for line in os_release.splitlines():
        key, _, value = line.partition("=")
        if key.strip() in ("ID", "ID_LIKE"):
            ids.update(value.strip().strip('"').lower().split())
    if "steamos" in ids:
        return "SteamOS: the system is read-only, so run DeHulud with --text (text mode) instead."
    for names, cmd in ((("debian", "ubuntu", "linuxmint", "pop"), "sudo apt install python3-tk"),
                       (("fedora", "rhel"), "sudo dnf install python3-tkinter"),
                       (("arch", "manjaro", "endeavouros"), "sudo pacman -S tk"),
                       (("opensuse", "suse", "opensuse-tumbleweed", "opensuse-leap"), "sudo zypper install python3-tk")):
        if ids & set(names):
            return "To get the window, install tkinter:  " + cmd
    return "To get the window, install your distribution's tkinter package for Python 3."


def _import_tk():
    import tkinter
    return tkinter


def parse_args(argv):
    """(text_mode, launch_command). Everything after --launch is the game's command, untouched."""
    own, cmd = argv, None
    if "--launch" in argv:
        i = argv.index("--launch")
        own, cmd = argv[:i], argv[i + 1:]
        if not cmd:
            raise SystemExit("dehulud: --launch needs the game command; Steam launch options: " + launch_option())
    for a in own:
        if a in ("-h", "--help"):
            print(__doc__)
            raise SystemExit(0)
        if a != "--text":
            if cmd is None:
                raise SystemExit("dehulud: unknown option %r (try --help)" % a)
            print("dehulud: ignoring unknown option %r" % a, file=sys.stderr)   # never block the game
    return "--text" in own, cmd


def main(argv=None, meter_factory=Meter, config_file=None):
    text, cmd = parse_args(sys.argv[1:] if argv is None else argv)
    child = None
    if cmd:
        try:
            child = subprocess.Popen(cmd)        # the game first: the meter must never stand in its way
        except OSError as e:
            print("dehulud: cannot start %s: %s" % (cmd[0], e), file=sys.stderr)
            return 127
    stop = (lambda: child.poll() is not None) if child else (lambda: False)
    try:
        meter = meter_factory(launched=child is not None)
        tk, display = None, os.environ.get("DISPLAY", "")
        if not text and local_display(display) is None:      # decided before tkinter or Xlib is even loaded
            print("dehulud: %s, using text mode." % ("DISPLAY=%r is not a display on this computer (DeHulud opens "
                  "no network connection)" % display if display else "no X display (DISPLAY is empty or not set)"),
                  file=sys.stderr)
        elif not text:
            try:
                tk = _import_tk()
            except ImportError:
                print("dehulud: tkinter is missing, using text mode. " + tk_install_hint(), file=sys.stderr)
        window = None
        if tk is not None:
            try:
                window = Window(tk, meter, config_file or config_path(), stop, display)
            except tk.TclError as e:
                print("dehulud: no window possible (%s), using text mode." % e, file=sys.stderr)
        if window is not None:
            window.run()
        else:
            run_text(meter, should_stop=stop)
    except Exception as e:
        if child is None:
            raise
        print("dehulud: the meter stopped (%r); the game keeps running." % e, file=sys.stderr)
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
