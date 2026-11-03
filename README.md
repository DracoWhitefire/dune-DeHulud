# DeHulud - sandworm threat meter for Dune: Awakening on Linux

> [!IMPORTANT]
> **Before you start: permission to read the game**
>
> DeHulud reads the threat value and its threshold out of the running game's memory. Linux only allows
> that if your system's *ptrace* setting permits it. Check yours:
>
> ```
> cat /proc/sys/kernel/yama/ptrace_scope
> ```
>
> | value | meaning | what to do |
> |---|---|---|
> | `0` | no extra restriction: a program may read your other programs under the normal rules (same user, the game not sandboxed or made unreadable, no security module refusing) | nothing, DeHulud should work as is |
> | `1` | a program may read only its own descendants (the programs it started, and theirs) | **recommended:** start the game through DeHulud (Steam launch option, below), or switch to `0` |
> | `2` | only programs with the `CAP_SYS_PTRACE` capability (normally only root) may read other programs | lower it to `1` (then use the launch option) or to `0` |
> | `3` | no program may, and the value cannot be lowered until a reboot | change the file that sets it (below), then reboot |
> | file missing | the Yama module is not active, or the setting is hidden (for example inside a container); a missing file does not prove that nothing restricts the read | nothing to change here; another sandbox or security module may still block it |
>
> **Steam launch option (recommended for `1`)** - no system change needed. In Steam: right-click
> Dune: Awakening > Properties > General > Launch Options, and enter
>
> ```
> '/path/to/dehulud.py' --launch %command%
> ```
>
> with the real path of the file, inside the single quotes: Steam runs this line as a shell command, so
> a path with a space in it (a folder named `My Games`, for example) would otherwise be split into two
> words and the game would not start. Make the file executable once (`chmod +x dehulud.py` in its
> folder); then `./dehulud.py --launch` there prints this exact line with your path, already quoted (that
> also works for a path that contains a `'`). Steam then starts the game as a child of DeHulud, which `1`
> allows; DeHulud closes when the game exits. **This mode is not yet confirmed with the Steam
> Linux Runtime container**: if Steam turns out to start the game outside DeHulud, the launch option cannot
> work. The window then says `not started by --launch`, and right-click > *Help / details* says what does
> for your setting: with `1` or `2`, switch to `0` (below) and start DeHulud by itself, without the launch
> option; with `3`, the same after a reboot. With `0` a `--launch` DeHulud reads such a game anyway.
>
> **Change the setting until the next reboot:**
>
> ```
> sudo sysctl kernel.yama.ptrace_scope=0
> ```
>
> **Change it permanently:** first look for a file that already sets it:
> `grep -rs ptrace_scope /etc/sysctl.d /run/sysctl.d /usr/lib/sysctl.d /usr/local/lib/sysctl.d /etc/sysctl.conf`.
>
> - If a file under `/etc/` sets it: **write down the old value**, change that line to `0`, then run
>   `sudo sysctl --system` (or reboot, if the old value was `3`).
> - Otherwise (nothing sets it, or only a packaged file under `/usr/lib/`, `/usr/local/lib/` or `/run/`):
>   add a file of your own. Of several files that set the same value, the one whose name sorts last wins,
>   so this file overrides the packaged one without touching it or its other settings:
>
>   ```
>   echo 'kernel.yama.ptrace_scope = 0' | sudo tee /etc/sysctl.d/99-dehulud-ptrace.conf
>   sudo sysctl --system
>   cat /proc/sys/kernel/yama/ptrace_scope
>   ```
>
>   The last line should print `0` (with `3` it cannot change before a reboot). Do not create a file with
>   the *same name* as a packaged one: it replaces the packaged file completely, with all of its other
>   settings.
>
> **Trade-off:** with `0`, any program running as your user can read the memory of your other programs
> (browser, password manager), which is exactly what `1` to `3` prevent, so prefer the launch option.
>
> Per-distribution notes (tkinter, where the file goes, known defaults) are [below](#per-distribution-notes).

DeHulud is one small Python file. It finds a running Dune: Awakening (Steam + Proton) by itself, checks
that it is a game version it knows, and shows how close you are to the sandworm attack threshold in a tiny
always-on-top window - or as one line in a terminal. No sound, no account, no network.

```
WORM [########......]  42%
on foot
```

The same meter also exists as one static binary that needs no Python: see [Rust version](#rust-version).

## What the meter shows

- **Percent** = your current threat value divided by the sandworm attack threshold, rounded down to a
  whole percent and capped at 0-100 %. The bar is the same ratio.
  100 % means you are at or above the threshold from which the worm *may* target you (the game also
  checks other conditions, so it does not mean "attacked now").
- **Colour:** green below 50 %, amber from 50 % to below 75 %, red from 75 %; grey when there is no value.
- **`-` instead of a number** (never a fake `0%`) whenever there is nothing trustworthy to show: no game,
  not in the world (menus, loading), a type check of the route failed, a value is not a number or negative,
  or the threshold is missing or `<= 0` (for example outside a worm area). `0%` is a real reading: because
  the percent is rounded down, it means a valid threat below 1 % of the threshold (a small non-zero threat
  shows `0%` too), never a missing value.
- **Status line:**

| status | meaning |
|---|---|
| `on foot` / `vehicle` | reading the character's / the driven vehicle's threat (the vehicle's, like the game's own meter, while you drive) |
| `on foot, no threshold` / `on foot, no value` | in the world, but the threshold is unset (`<= 0`) or the value is invalid: shows `-` |
| `waiting for game` | the game is not running (or not loaded yet) |
| `not in world` | game found, but no player in a world yet (menus, loading screens), or the player controls no pawn |
| `no data` | a type check failed or memory moved during the read; usually only during loading, or for a moment when the controlled pawn changes (getting on or off a vehicle) |
| `Unsupported game version` | a game build DeHulud does not know; the bar area shows its PE timestamp |
| `no permission` | see the box at the top; right-click > *Help / details* shows the exact fix for your setting |
| `not started by --launch` | launch-option mode, but Steam started the game outside DeHulud and your ptrace setting (`1` to `3`) does not allow reading it; *Help / details* says what works |

Not yet confirmed in a live game: whether the two values are kept up to date on your PC (the server
replicates them) and whether the percent matches the game's own threat indicator. The game's own widget
is drawn by game scripts DeHulud cannot see. If DeHulud stays at `-` (`no threshold`) or at `0%` while the
game shows threat, that is the reason, not a fault of your setup.

## Supported game versions

| game build | PE timestamp | SizeOfImage |
|---|---|---|
| 26333948 | 1793384724 (2026-10-30 18:25 UTC) | 220024832 |
| 26224975 | 1793030166 (2026-10-26 15:56 UTC) | 219996160 |
| 26083551 | 1792504044 (2026-10-20 13:47 UTC) | 220008448 |

DeHulud identifies the build from the game's own PE header in memory (TimeDateStamp + SizeOfImage) and
re-checks those two numbers on every poll. That is a quick check of two header fields, not a checksum of
the game file.
**After a game update** the offsets move: DeHulud then shows `-` and `Unsupported game version` with the new
build's PE timestamp, never a guess. It needs a new DeHulud release with that build in its builds table
(`BUILDS` in `dehulud.py`).

## Running it

Requirements: Linux, Python 3.9 or newer (tested with 3.11, 3.12 and 3.13; `python3 --version` shows
yours), and tkinter for the window (without tkinter it falls back to text mode and tells you which package
to install).

**The first time, step by step:**

1. Download `dehulud.py` from the project page (or the whole project as a ZIP file, then unzip it:
   right-click the ZIP > *Extract*, or `unzip` in a terminal).
2. Put `dehulud.py` in a folder of your own and leave it there, for example a new folder `dehulud` in your
   home folder. The Steam launch option, if you use it, points to this place.
3. Open a terminal in that folder: in the file manager, right-click inside the folder > *Open Terminal
   Here* (Dolphin: *Open Terminal*, or F4; GNOME Files: *Open in Terminal*). Or open any terminal and type
   `cd ~/dehulud`.
4. Make the file executable, once: `chmod +x dehulud.py`
5. Check the permission box at the top of this page, then start it: `./dehulud.py`

```
./dehulud.py                  # window
./dehulud.py --text           # one line in the terminal instead
./dehulud.py --launch CMD...  # start CMD (Steam's %command%) and read the game it starts
python3 dehulud.py            # the same without chmod
```

Start it before or after the game; it waits for the game and notices when the game exits.

**The window** is about 180 x 44 pixels, always on top and frameless. Drag it with the left mouse
button. Right-click opens the menu: *Window frame* (show/hide the title bar), *Reset position*,
*Help / details* (the full message behind `no permission` and similar states) and *Quit*. Its position is
remembered in `$XDG_CONFIG_HOME/dehulud/config.json` (normally `~/.config/dehulud/config.json`); a damaged
config file is ignored. Without `HOME` (and without an absolute `XDG_CONFIG_HOME`) no position is kept: DeHulud
does not look the home folder up in the user database, because that can involve a local name service. (With
`HOME` unset, the Python interpreter's own start-up may still do that lookup; the Rust version does not.)

**Placement:** the first time, it opens at the top-right of your main screen (with several screens it
picks a real screen when the display reports its monitors through XRandR); *Reset position* puts it back
there. It is meant to stay above games in *windowed* or *borderless windowed* mode and works on a second
screen. In *exclusive fullscreen* the game may cover it. On Wayland desktops the window runs through XWayland and staying on top is up to your
compositor: checked on KDE Plasma (Wayland) over the running game in fullscreen, with three screens.

## Rust version

The folder `rust/` holds the same meter written in Rust: **one static binary, no Python and no Tk needed**,
nothing else to install. It finds the game the same way, checks the same game build (and re-checks it on every
poll), reads the same two values with the same type checks and owner rules, and shows the same window, menu,
statuses and terminal line. It uses the same config file (`$XDG_CONFIG_HOME/dehulud/config.json`), so a
position saved by one version is used by the other. (One small difference: the Rust version reads a config
file nested at most 500 levels deep and uses the defaults for a deeper one. DeHulud itself only ever writes a
flat file, so this matters only for a hand-edited file.)

**Run the prebuilt binary** (when you have one): put the file `dehulud` in a folder of your own, open a
terminal in that folder and run

```
chmod +x dehulud              # once
./dehulud                     # window
./dehulud --text              # one line in the terminal
./dehulud --launch CMD...     # Steam launch option, as for the Python version
```

For the Steam launch option enter `'/path/to/dehulud' --launch %command%`; `./dehulud --launch` prints that
exact line with your path, already quoted.

**Build it from source:** install Rust with [rustup](https://rustup.rs), then

```
rustup target add x86_64-unknown-linux-musl
cd rust
RUSTFLAGS="--remap-path-prefix=$HOME=~" cargo build --release --target x86_64-unknown-linux-musl
```

The binary is `rust/target/x86_64-unknown-linux-musl/release/dehulud`: one static executable of about 0.7 MB
for x86-64 Linux. The `RUSTFLAGS` part is optional; it keeps your home folder's path out of the
binary (Rust stores source file paths for its error messages), which matters if you pass the file on. It uses
two libraries (crates): `libc` (system calls such as `process_vm_readv`) and `x11rb` (the X11 protocol, written in
Rust, with the RandR extension for the monitor list); no system library is needed.

**Permission:** the box at the top of this page applies unchanged: the same `ptrace_scope` rules, the same
launch option, the same messages (*Help / details*, or the terminal).

**Reads:** the same as the Python version (see *How much it reads* below): 4 polls per second, 16 fixed reads
on foot / 15 in a vehicle plus one per class-hierarchy step, so 84 / 80 reads per second in the test layout,
and only a look at `/proc` every 2 seconds while the game is not found.

**Differences from the Python version:**

- The window, the right-click menu and the *Help / details* box are drawn by DeHulud itself, with a bitmap font
  built into the binary (no toolkit, no fonts from your system). The text therefore looks pixel-style instead
  of your desktop's font; positions, sizes and colours are the same. *Help / details* opens a small window
  titled `DeHulud help`; a click or a key closes it.
- The window opens the X display itself, through the display's Unix socket: first the socket file
  `/tmp/.X11-unix/XN` (what sandboxes and containers pass on to the programs inside them), then the abstract
  socket `@/tmp/.X11-unix/XN` (for a program with its own private `/tmp`). It never tries TCP. `DISPLAY` must
  be a display on this computer (`:N`, `:N.S`, `unix:N`), as for the Python version; anything else gives the
  text meter and a one-line reason.
- Without a local display the message says that the window needs an X11 or XWayland display (there is no
  tkinter to install), and it does not read `/etc/os-release`.
- Ctrl-C in the terminal that started it also closes the window cleanly and keeps its position (the Python
  version prints an error there). With the window frame on, the saved position is the top-left corner of the frame,
  taken from the window manager's frame size (`_NET_FRAME_EXTENTS`).
- Messages from the operating system are worded the Rust way (for example `No such file or directory` without
  Python's `[Errno 2]`).

**Its tests** (in `rust/`):

```
cargo test                                      # unit tests, source lint, comparison with dehulud.py,
                                                # and the binary as built against the fake game in a
                                                # private PID namespace (unshare, no root needed;
                                                # skipped where that is not available)
cargo test --release --features test-source     # also: the fake-game scenarios of the Python tests,
                                                # and the window on a private headless compositor
```

`test-source` is a test-only build option: it lets the tests point the meter at their own fake game and record
each poll. The normal build commands above never enable it (the code is not compiled in); only a build that
names `--features test-source` - such as the test command above, even with `--release` - contains it, and a
binary built with it refuses to run without the test's control file. Share only binaries from the normal build. The Python tests (`python3 -m unittest discover -s tests`) are unchanged
and cover the Python version.

## Per-distribution notes

The sysctl commands from the box at the top are the same everywhere (`/etc/sysctl.d/` is read at boot by
`systemd-sysctl` on all of the systems below). Defaults below are linked to their source and
can change with a release, so check yours with `cat /proc/sys/kernel/yama/ptrace_scope` and follow the table at the top.

| distribution | install tkinter (for the window) | ptrace setting |
|---|---|---|
| Ubuntu, Linux Mint, Pop!_OS | `sudo apt install python3-tk` | default `1`, set by the file `/etc/sysctl.d/10-ptrace.conf` that ships with the system ([Ask Ubuntu](https://askubuntu.com/questions/146160/what-is-the-ptrace-scope-workaround-for-wine-programs-and-are-there-any-risks)): use the Steam launch option, or edit the value in that file to `0` and run `sudo sysctl --system` |
| Debian | `sudo apt install python3-tk` | Debian's kernel ships the patch "yama: Disable by default" (named in the Debian kernel changelog, `/usr/share/doc/linux-image-*/changelog.gz`), so the value is `0` unless something sets it |
| Fedora | `sudo dnf install python3-tkinter` | Fedora 45 and later: default `1` unless a debugger such as `gdb` is installed; `sudo dnf install yama-ptrace-enable` switches it to `0` the Fedora way. Fedora 44 and earlier: `0` ([Fedora change page](https://fedoraproject.org/wiki/Changes/Restrict_ptrace_by_default)). Check your value either way. Fedora also has the SELinux switch `deny_ptrace`: `getsebool deny_ptrace` must say `off` (`sudo setsebool deny_ptrace off`) |
| Arch, Manjaro, EndeavourOS | `sudo pacman -S tk` | Arch: default `1` ([ArchWiki, Security](https://wiki.archlinux.org/title/Security)); Manjaro and EndeavourOS use their own kernels, so check your value |
| openSUSE (Tumbleweed, Leap) | `sudo zypper install python3-tk` | check your value |
| SteamOS (Steam Deck) | the system is read-only: if `python3 -c "import tkinter"` fails, use `--text` in Desktop Mode (Konsole). Unlocking the system to install packages is undone by updates and not recommended | check your value. `sudo` needs a password: set one with `passwd` in Desktop Mode. `sudo sysctl ...` works until reboot; whether a file in `/etc/sysctl.d/` survives SteamOS updates is not verified |

**Steam Deck Game Mode:** untested. Game Mode shows one window at a time, so DeHulud's window is not
expected to be visible there and could confuse which window is shown; use Desktop Mode.

**Flatpak Steam:** the game then runs inside the Flatpak sandbox. What is known: DeHulud finds the game by
its process and file name, which the sandbox does not hide from your desktop session, and the kernel
permission rules above still apply. What is not known (not tested): whether reading a sandboxed game works
with `ptrace_scope` `0`, and whether `--launch` works at all inside the Flatpak (the launch option runs
inside the sandbox, which must be able to see `dehulud.py`, run Python 3 and open a window, and the
sandbox may block the read). The one live check so far used the native (non-Flatpak) Steam package (see
the end of this page).

## Read-only and no network: what DeHulud does and does not do

- It reads `/proc/<pid>/cmdline`, `/proc/<pid>/maps` and `/proc/<pid>/status` of processes that belong to
  you, `/proc/sys/kernel/yama/ptrace_scope`, `/etc/os-release` (only to name the tkinter package) and its
  own config file.
- It reads game memory only with `process_vm_readv`. It never writes to the game, never attaches a
  debugger (no ptrace attach), never sends signals, injects nothing, simulates no input, and opens no
  network connection of any kind (no update check, no telemetry). `--launch` only starts Steam's command
  as a normal child process and waits for it.
- The window only uses an X display on this computer: `DISPLAY` must be `:N` or `unix:N` (optionally with a
  screen, `:N.S`). A display on another machine (`host:N`, or `localhost:N` as set by `ssh -X`) is refused
  before anything graphical is loaded, and DeHulud runs as the text meter with a one-line explanation. It
  hands Tk the display as `unix/:N`, the form with which Xlib uses only the display's Unix socket: with a
  plain `:N` Xlib falls back to a TCP connection on `localhost` when that socket cannot be reached.
- A source lint in the test suite checks the code of `dehulud.py`: it fails if the file contains a
  memory-write, debugger, signal, process-kill or network call, or imports a network module. That is a
  check of this file's text, not complete enforcement: it cannot see what libraries such as Tk and Xlib do
  at run time (the display check above covers the window's connection), and it is not a sandbox.

**How much it reads.** 4 polls per second. While the game is not found it only looks at `/proc` every
2 seconds. Once found: one 1024-byte read of the PE header and 8 reads of class pointers, then per poll
a fixed **16 reads on foot / 15 in a vehicle** plus one read for every class-hierarchy step the type checks
walk (0-4 steps per check, five checks), i.e.:

| | reads per poll | reads per second |
|---|---|---|
| on foot | 17 - 36 | 68 - 144 |
| in a vehicle | 15 - 35 | 60 - 140 |
| test layout (the fake game's class chains: 1-2 steps per check) | 21 on foot / 20 vehicle | **84 / 80** |

One of the fixed reads re-reads the start of the game's PE header (up to SizeOfImage: the header offset
`e_lfanew` + 88 bytes, 216 bytes in the test layout), so that a different program image under the same
process id is noticed and identified again instead of being read with the old build's offsets. The other
reads are 8 bytes (a pointer), 12 bytes (the local-players array) or 24 bytes (threat value and threshold
together); about 400 bytes per poll in the test layout.

**How it finds the value.** From the game's GWorld slot: World > GameInstance > LocalPlayers[0] (the
array size is checked before element 0 is read) > PlayerController > the possessed pawn (`Pawn`). If that
pawn is a vehicle (ADuneVehicle) it reads the vehicle's `m_SandwormThreatSourceComponent`; if it is the
player character (ADuneCharacter and ADunePlayerCharacter) and also the controller's `AcknowledgedPawn`, the
character's. Any other pawn, or none, shows `-`: DeHulud never falls back to another object's value. Then it
reads the component's `m_CurrentThreatBlobValue` and `m_CurrentSandwormAttackThreshold`. Every object on the
way is type-checked against the game's own class pointers, so a moved or freed object normally fails a
check and shows `-` instead of garbage. A type check cannot catch everything: memory that was freed and
then reused by another object of the same type passes it. Every offset in `dehulud.py` is commented with
the game member it reads.

## Terms of service and anti-cheat

DeHulud does not change the game, but reading another program's memory may still be against the game's
terms of service, and anti-cheat software may treat it as suspicious. Nobody can promise that it is safe
for your account. **Use at your own risk.**

## Uninstall

1. Delete `dehulud.py` and/or the Rust binary `dehulud`, and the config folder `~/.config/dehulud` (or
   `$XDG_CONFIG_HOME/dehulud`).
2. Remove the Steam launch option if you added one (Steam > Dune: Awakening > Properties > General >
   Launch Options: delete the line).
3. If you changed the ptrace setting, put back what was there before:
   - Changed only with `sudo sysctl kernel.yama.ptrace_scope=0`: a reboot restores it, or set your old
     value now, for example `sudo sysctl kernel.yama.ptrace_scope=1`.
   - Created `/etc/sysctl.d/99-dehulud-ptrace.conf`: `sudo rm /etc/sysctl.d/99-dehulud-ptrace.conf`, then
     `sudo sysctl --system`. That applies a packaged file's value again; if no file sets the value, the
     kernel's default only returns with a reboot (or set the old value with `sudo sysctl` as above).
   - Edited a line in an existing file: set that line back to the old value you wrote down, then run
     `sudo sysctl --system`.
   - Fedora, if you used them: `sudo dnf remove yama-ptrace-enable`, and `sudo setsebool deny_ptrace on`
     if you switched SELinux's `deny_ptrace` off (add `-P` if you used `-P`).

## Development

```
python3 -m unittest discover -s tests
```

The tests start a fake game: a child Python process that lays out the whole route (fake PE header, class
objects, pointers and the two floats) in its own memory, and DeHulud reads it with `process_vm_readv`
through an injected `/proc` source that only lists that child. They cover build matching, on foot, vehicle,
owner selection (a pawn that fails its check is `-`), the same process exec'ing into another image,
`LocalPlayers` empty, every type check, invalid values, permission errors for each `ptrace_scope` value
(simulated), `--launch` for each value with and without the game descending from DeHulud, the game exiting
during a poll, the local-display check, quoting of printed commands, config handling and text mode. The
GUI test builds the real window only on a private headless compositor (`kwin_wayland --virtual --xwayland` under its own
D-Bus session and runtime directory) and is skipped when that or tkinter is missing; it never uses your
desktop's display. Checked live once, on one PC: KDE Plasma (Wayland), native Steam, the game in
fullscreen. Not tested: `--launch` under the real Steam Linux Runtime, Flatpak Steam, other desktops and
other distributions.
