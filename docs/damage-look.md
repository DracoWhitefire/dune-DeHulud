# DeHulud Combat: how its window looks

This page is about **DeHulud Combat**, the second program in this repo: the window with the header switches, the
**Damage taken** list and the **Damage dealt** columns, in both of its versions (Tk in `dehulud_combat.py`, the drawn
window with the built-in bitmap font in the `dehulud-combat` binary). Its window is titled `DeHulud Combat`, and
wherever the plain program shows its name (the window title, the help box) Combat shows that name; its config file
lives in its own folder, `dehulud-combat`, with the same `x`, `y` and `frame` keys plus the three switches.

**Plain DeHulud is not changed by this page**: its window stays 180 x 44, with no header row and the status line
under the bar, exactly as shipped. Combat's worm meter is the same meter drawn the same way, under the header.

Behaviour (what is read, when a number is emitted) is not repeated here; this page only says what is drawn where, in
which colour and size.

Everything below is in window pixels. The window stays **180 px wide**; only its height changes. Sizes are chosen on
the bitmap font's grid so that both versions can place every item at the same pixel: the small font is 5 x 9 per
character, 6 px per character advance (7 when bold), ascent 7; the big bold font is 11 px tall, 7 px per digit, 5 px
for `-`, 1 px between glyphs.

## Sections, top to bottom

| section | height | shown when |
|---|---|---|
| header (the three switches) | 18 | always |
| worm meter (label, bar, percent) | 24 | `worm` on |
| damage taken | 60 | `damage_taken` on |
| damage dealt | 76 | `damage_dealt` on |
| status line (`on foot`, `waiting for game`, ...) | 20 | at least one section on |

Window height = the sum of the shown sections. The order never changes; a section that is off takes no space.

| worm | taken | dealt | height | the window |
|---|---|---|---|---|
| off | off | off | 18 | header only |
| on | off | off | 62 | Combat's worm-only window: header, meter, status (plain DeHulud stays 180 x 44) |
| off | on | off | 98 | |
| off | off | on | 114 | |
| on | on | off | 122 | |
| on | off | on | 138 | |
| off | on | on | 174 | |
| on | on | on | 198 | |

The window keeps its top-left corner when it grows or shrinks (the saved position is that corner). The height changes
at once on a click in the header, and the window opens at the right height for the config file and flags.

## Colours

| use | hex | note |
|---|---|---|
| background | `#1d1f21` | as in plain DeHulud |
| section labels `WORM`, `TAKEN`, `DEALT`; a switch that is on | `#d8cfb4` | plain DeHulud's label colour |
| a switch that is off | `#6b7174` | the status colour at 55 % over the background |
| column header `HEALTH` and health numbers | `#e8bf70` | the bar's amber |
| column header `SHIELD` and shield numbers | `#8ad4c8` | new: pale teal, as muted as the other three bar colours |
| damage taken numbers | `#ee7868` | the bar's red |
| `-` (nothing trustworthy) in any section | `#85969d` | the bar's grey |
| status line, hint | `#aab4b8`, `#b8c0c4` | as in plain DeHulud |

Fade steps, one per poll (4 polls per second). A number is drawn at full colour for its first two polls, then in three
dimmer steps, and is removed on its fifth poll: about 1.25 s on screen. The dim steps are the colour blended towards
the background at 70 %, 45 % and 20 %; both versions use exactly these values (the drawn window allocates them once):

| age in polls | 0, 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| red | `#ee7868` | `#af5d53` | `#7b4741` | `#47312f` | removed |
| amber | `#e8bf70` | `#ab8f58` | `#786745` | `#463f31` | removed |
| teal | `#8ad4c8` | `#699e96` | `#4e706c` | `#334342` | removed |

Age counts polls, not wall time, so the two versions and the tests agree poll by poll.

## Header: `[x] Worm  [x] Taken  [x] Dealt`

Small font, regular. Text vertically centred at y = 9 (bitmap: top row at y = 6). Three fixed strings at fixed x:

| switch | text (on / off) | x of the first character | click zone (x) |
|---|---|---|---|
| worm | `[x] Worm` / `[ ] Worm` | 6 | 0 to 56 |
| taken | `[x] Taken` / `[ ] Taken` | 60 | 57 to 116 |
| dealt | `[x] Dealt` / `[ ] Dealt` | 120 | 117 to 179 |

The last string ends at x = 172, leaving the same 6 to 7 px margin as on the left. "Off" reads two ways at once: the
box is empty and the whole string is in the off colour; "on" is a filled box in the label colour. Nothing else
changes with the state (no strike-through, no size change).

A click is a left-button press and release inside the header row (y from 0 to 17) with less than 3 px of movement:
on the release, the switch of the zone under the pointer flips. A press that moves 3 px or more is a drag and moves
the window as before; it flips nothing. Right-click in the header opens the menu as everywhere else. The three zones
cover the full row width, so the target is the whole row, not just the characters.

## Worm meter

Plain DeHulud's meter row, moved down by the header: inside its 24 px, `WORM` bold at (6, centre 12), the bar from
(44, 6) to (130, 18), the hint centred in the bar, the percent in the big font right-aligned to x = 174 at centre 12.
In Combat the status line is not part of this section: it is its own section at the bottom (below), so that
`waiting for game` and `Unsupported game version` are still shown when the worm meter is off. (In plain DeHulud the
status line stays under the bar, as shipped.)

## Damage taken

```
y=0   +------------------------------------+
      | TAKEN                         -37  |   row 1, big font, newest
      |                               -12  |   row 2
      |                                 -9 |   row 3  (dim)
      |                                    |   row 4  (empty)
y=59  +------------------------------------+
```

- `TAKEN` bold, label colour, at x = 6, centred on y = 12 (the same place as `WORM` in its section).
- Numbers in the big font, red, right-aligned to x = 174 (the last ink column is 173, like the percent). Text `-N`.
  Four rows, top of the glyphs at y = 7, 20, 33, 46 (13 px pitch: 11 px glyph + 2 px gap). Row 1 is the newest; a
  new number pushes the others down one row and the fifth oldest out.
- `N` is whole points; a number that rounds to 0 is not drawn. The window never draws more than five digits
  (`-99999` is 45 px wide, from x = 129); a larger value is drawn as `-99999`.
- Nothing to show (no hit): the rows are empty, the label stays.

## Damage dealt

```
y=0   +------------------------------------+
      | DEALT          HEALTH      SHIELD  |   column headers, small font
      |                                    |   row 1 (empty)
      |                   240              |   row 2  (dim)
      |                    84          40  |   row 3
      |                   120              |   row 4, newest
y=75  +------------------------------------+
```

- Header row centred on y = 12: `DEALT` bold, label colour, at x = 6; `HEALTH` small regular in amber, right-aligned
  to x = 122; `SHIELD` small regular in teal, right-aligned to x = 174.
- Numbers in the big font, no minus sign: health column right-aligned to x = 122 (ink from x = 83 at most), shield
  column to x = 174 (from 135). Four rows per column, top of the glyphs at y = 22, 35, 48, 61. Row 4 is the newest; a
  new number pushes that column's others up one row and the fifth oldest out. The two columns move independently.
- Five digits at most, as above.

## `-` and `Unsupported game version`

The status line is one for the whole window and keeps plain DeHulud's texts and colour. A section shows a grey `-` in the big
font in its newest slot (taken: row 1; dealt: row 4, in both columns) and clears its list whenever the poll had nothing
trustworthy for it:

| state | worm | taken | dealt | status line |
|---|---|---|---|---|
| no game, not in world, no data, no permission, not started by `--launch` | `-` | `-` | `-` | as in plain DeHulud |
| `Unsupported game version` | `-`, hint `PE ...` in the bar | `-` | `-` | `Unsupported game version` |
| in the world, worm threshold unset or value invalid | `-` | numbers | numbers | `on foot, no threshold` etc. |
| in a vehicle, no own-character route in the constants, or an invalid health pair | numbers or `-` | `-` | numbers | `vehicle` |
| in the world, nothing hit and nothing hitting | numbers | empty rows | empty rows | `on foot` |

A per-target failure in damage dealt (a stale handle, a vehicle target, a failed class check) is silent: that target
simply shows nothing. `-` in the dealt section means the world itself could not be read. With the worm meter off, the
`PE ...` hint of an unsupported build is not shown anywhere in the window; the status line still names the state and
right-click > *Help / details* has the number.

## Mocks

All three on, in the world (198 px tall):

```
+------------------------------------+
| [x] Worm  [x] Taken  [x] Dealt     |  18
| WORM  [########......]        42%  |  24
| TAKEN                         -37  |  60
|                               -12  |
|                                    |
|                                    |
| DEALT          HEALTH      SHIELD  |  76
|                                    |
|                                    |
|                    84          40  |
|                   120              |
| on foot                            |  20
+------------------------------------+
```

Worm off, both damage sections on, game not running (174 px):

```
+------------------------------------+
| [ ] Worm  [x] Taken  [x] Dealt     |
| TAKEN                           -  |
|                                    |
|                                    |
|                                    |
| DEALT          HEALTH      SHIELD  |
|                                    |
|                                    |
|                                    |
|                     -           -  |
| waiting for game                   |
+------------------------------------+
```

All off (18 px): `| [ ] Worm  [ ] Taken  [ ] Dealt     |`, nothing else, no status line.

## Fonts in the Tk version

The Tk version draws the same strings at the same anchors with its default font; glyph shapes and exact text widths
differ from the bitmap font, which is why every number and column header is right-aligned to a fixed x.

| item | Tk font | bitmap font |
|---|---|---|
| header switches, column headers `HEALTH` / `SHIELD`, status line | `TkDefaultFont` 8 | small, regular |
| `WORM`, `TAKEN`, `DEALT` | `TkDefaultFont` 8 bold | small, bold |
| percent, damage numbers, `-` | `TkDefaultFont` 11 bold | big |
| hint in the bar | `TkDefaultFont` 7 | small, regular |

## Identical in both versions / may differ

Identical: the section order and heights, every x and y above, the window width, the colours and fade steps by poll
age, the strings, the click zones and the click-or-drag rule, which section shows `-` when, the five-digit cap, the
config keys (`worm`, `damage_taken`, `damage_dealt`) and flags (`--no-worm`, `--no-taken`, `--no-dealt`), and that
a click in the header saves the clicked switch to Combat's config file (a switch set by a `--no-*` flag keeps the
file's value until it is clicked).

May differ: glyph shapes, anti-aliasing, the exact pixel width of a text (hence the right-aligned anchors), the
menu (Tk's own menu against the drawn one) and the help box, as in plain DeHulud.

Not covered here: Combat's terminal line (`--text`), and anything about plain DeHulud, which this page leaves as it is.
