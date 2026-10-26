# DeHulud - sandworm threat meter for Dune: Awakening on Linux

> [!IMPORTANT]
> **Before you start: permission to read the game**
>
> DeHulud reads one number out of the running game's memory. Linux only allows that if your
> system's *ptrace* setting permits it. Check yours:
>
> ```
> cat /proc/sys/kernel/yama/ptrace_scope
> ```
>
> | value | meaning | what to do |
> |---|---|---|
> | `0` | any program may read your other programs | nothing, DeHulud works as is |
> | `1` | only a program's parent may read it | **recommended:** start the game through DeHulud (Steam launch option, below), or switch to `0` |
> | `2` | only root may read other programs | lower it to `1` (then use the launch option) or to `0` |
> | `3` | nobody may, and it cannot be lowered until a reboot | change the file that sets it (below), then reboot |
> | file missing | your kernel has no Yama module | nothing to change here; another sandbox or security module may still block it |
>
> **Steam launch option (recommended for `1`)** - no system change needed. In Steam: right-click
> Dune: Awakening > Properties > General > Launch Options, and enter
>
> ```
> /path/to/dehulud.py --launch %command%
> ```
>
> (use the real path, and make the file executable once: `chmod +x /path/to/dehulud.py`). Steam then starts
> the game as a child of DeHulud, which `1` allows; DeHulud closes when the game exits. **This mode is not
> yet confirmed with the Steam Linux Runtime container**: if Steam turns out to start the game outside
> DeHulud, the window says `not started by --launch` and tells you to use the change below instead.
>
> **Change the setting until the next reboot:**
>
> ```
> sudo sysctl kernel.yama.ptrace_scope=0
> ```
>
> **Change it permanently:** first look for a file that already sets it:
> `grep -rs ptrace_scope /etc/sysctl.d /run/sysctl.d /usr/lib/sysctl.d /usr/local/lib/sysctl.d /etc/sysctl.conf`.
> If a file in `/etc/` sets it, edit that line; if a file in `/usr/lib/sysctl.d/` sets it, create a file
> with the *same name* in `/etc/sysctl.d/` (it replaces the packaged one). Otherwise:
>
> ```
> echo 'kernel.yama.ptrace_scope = 0' | sudo tee /etc/sysctl.d/10-ptrace.conf
> sudo sysctl --system
> ```
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

## What the meter shows

- **Percent** = your current threat value divided by the sandworm attack threshold, rounded down to a
  whole percent and capped at 0-100 %. The bar is the same ratio.
  100 % means you are at or above the threshold from which the worm *may* target you (the game also
  checks other conditions, so it does not mean "attacked now").
- **Colour:** green below 50 %, amber from 50 % to below 75 %, red from 75 %; grey when there is no value.
- **`-` instead of a number** (never a fake `0%`) whenever there is nothing trustworthy to show: no game,
  not in the world (menus, loading), a type check of the route failed, a value is not a number or negative,
  or the threshold is missing or `<= 0` (for example outside a worm area). A real `0%` is shown only for a
  genuine zero threat with a valid threshold.
- **Status line:**

| status | meaning |
|---|---|
| `on foot` / `vehicle` | reading the character's / the driven vehicle's threat (the vehicle's, like the game's own meter, while you drive) |
| `on foot, no threshold` / `on foot, no value` | in the world, but the threshold is unset (`<= 0`) or the value is invalid: shows `-` |
| `waiting for game` | the game is not running (or not loaded yet) |
| `not in world` | game found, but no player in a world yet (menus, loading screens) |
| `no data` | a type check failed or memory moved during the read; usually only during loading |
| `Unsupported game version` | a game build DeHulud does not know; the bar area shows its PE timestamp |
| `no permission` | see the box at the top; right-click > *Help / details* shows the exact fix for your setting |
| `not started by --launch` | launch-option mode, but Steam started the game outside DeHulud |

Not yet confirmed in a live game: whether the two values are kept up to date on your PC (the server
replicates them) and whether the percent matches the game's own threat indicator. The game's own widget
is drawn by game scripts DeHulud cannot see. If DeHulud stays at `-` (`no threshold`) or at `0%` while the
game shows threat, that is the reason, not a fault of your setup.

## Supported game versions

| game build | PE timestamp | SizeOfImage |
|---|---|---|
| 26083551 | 1792504044 (2026-10-20 13:47 UTC) | 220008448 |

DeHulud identifies the build from the game's own PE header in memory (TimeDateStamp + SizeOfImage).
**After a game update** the offsets move: DeHulud then shows `-` and `Unsupported game version` with the new
build's PE timestamp, never a guess. It needs a new DeHulud release with that build in its builds table
(`BUILDS` in `dehulud.py`).

## Running it

Requirements: Linux, Python 3.9 or newer, and tkinter for the window (without tkinter it falls back to text
mode and tells you which package to install).

```
chmod +x dehulud.py
./dehulud.py                  # window
./dehulud.py --text           # one line in the terminal instead
./dehulud.py --launch CMD...  # start CMD (Steam's %command%) and read the game it starts
```

Start it before or after the game; it waits for the game and notices when the game exits.

**The window** is about 180 x 44 pixels, always on top and frameless. Drag it with the left mouse
button. Right-click opens the menu: *Window frame* (show/hide the title bar), *Reset position*,
*Help / details* (the full message behind `no permission` and similar states) and *Quit*. Its position is
remembered in `$XDG_CONFIG_HOME/dehulud/config.json` (normally `~/.config/dehulud/config.json`); a damaged
config file is ignored.

**Placement:** the first time, it opens at the top-right of your main screen (with several screens it
always picks a real screen); *Reset position* puts it back there. It is meant to stay above games in
*windowed* or *borderless windowed* mode and works on a second screen. In *exclusive fullscreen* the game
may cover it. On Wayland desktops the window runs through XWayland and staying on top is up to your
compositor: checked on KDE Plasma (Wayland) over the running game in fullscreen, with three screens.

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
sandbox may block the read). The native (non-Flatpak) Steam package is the tested target.

## Read-only and no network: what DeHulud does and does not do

- It reads `/proc/<pid>/cmdline`, `/proc/<pid>/maps` and `/proc/<pid>/status` of processes that belong to
  you, `/proc/sys/kernel/yama/ptrace_scope`, `/etc/os-release` (only to name the tkinter package) and its
  own config file.
- It reads game memory only with `process_vm_readv`. It never writes to the game, never attaches a
  debugger (no ptrace attach), never sends signals, injects nothing, simulates no input, and opens no
  network connection of any kind (no update check, no telemetry). `--launch` only starts Steam's command
  as a normal child process and waits for it.
- The test suite enforces this: it fails if `dehulud.py` contains a memory-write, debugger, signal,
  process-kill or network call, or imports a network module.

**How much it reads.** 4 polls per second. While the game is not found it only looks at `/proc` every
2 seconds. Once found: one 1024-byte read of the PE header and 8 reads of class pointers, then per poll
a fixed **15 reads on foot / 14 in a vehicle** (16 on foot in the moment the possessed pawn and the
acknowledged pawn differ) plus one read for every class-hierarchy step the type checks walk (0-4 steps per
check; five checks, six in that moment), i.e.:

| | reads per poll | reads per second |
|---|---|---|
| on foot | 16 - 40 | 64 - 160 |
| in a vehicle | 14 - 34 | 56 - 136 |
| test layout (Blueprint depths like the game's: 1-2 steps) | 20 on foot / 19 vehicle | **80 / 76** |

Each read is 8 bytes (a pointer), 12 bytes (the local-players array) or 24 bytes (threat value and
threshold together); about 180 bytes per poll in the test layout.

**How it finds the value.** From the game's GWorld slot: World > GameInstance > LocalPlayers[0] (the
array size is checked before element 0 is read) > PlayerController > the possessed pawn. If that pawn is a
vehicle (ADuneVehicle) it reads the vehicle's `m_SandwormThreatSourceComponent`, otherwise the player
character's (ADuneCharacter and ADunePlayerCharacter, via AcknowledgedPawn), then the component's
`m_CurrentThreatBlobValue` and `m_CurrentSandwormAttackThreshold`. Every object on the way is type-checked
against the game's own class pointers, so a moved or freed object shows `-`, not garbage. Every offset in
`dehulud.py` is commented with the game member it reads.

## Terms of service and anti-cheat

DeHulud does not change the game, but reading another program's memory may still be against the game's
terms of service, and anti-cheat software may treat it as suspicious. Nobody can promise that it is safe
for your account. **Use at your own risk.**

## Uninstall

1. Delete `dehulud.py` and the config folder `~/.config/dehulud` (or `$XDG_CONFIG_HOME/dehulud`).
2. Remove the Steam launch option if you added one.
3. If you changed the ptrace setting: delete the file you created (for example
   `sudo rm /etc/sysctl.d/10-ptrace.conf`) and reboot (or set your old value with `sudo sysctl`).

## Development

```
python3 -m unittest discover -s tests
```

The tests start a fake game: a child Python process that lays out the whole route (fake PE header, class
objects, pointers and the two floats) in its own memory, and DeHulud reads it with `process_vm_readv`
through an injected `/proc` source that only lists that child. They cover build matching, on foot, vehicle,
`LocalPlayers` empty, every type check, invalid values, permission errors for each `ptrace_scope` value
(simulated), the game exiting during a poll, config handling, text mode and `--launch`. The GUI test builds
the real window only on a private headless compositor (`kwin_wayland --virtual --xwayland` under its own
D-Bus session and runtime directory) and is skipped when that or tkinter is missing; it never uses your
desktop's display. Checked live once (Debian 13, KDE Plasma on Wayland, native Steam). Not tested:
`--launch` under the real Steam Linux Runtime, Flatpak Steam, and the other distributions.
