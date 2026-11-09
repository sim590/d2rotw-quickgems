# d2rotw-quickgems

A small, deliberately narrow tool that reads the gem table in Diablo II:
Resurrected (Reign of the Warlock) and automates gem transmutations.

Status: early prototype, one developer, tested on one machine.

## Demo

<p align="center">
  <img src="assets/demo.gif" alt="QuickGems converting gems" width="320"><br>
  <em>Live demo</em>
</p>

One conversion at 2x speed: three source gems go into the cube, are
transmuted, and the result is returned to the stash.

## What it does

- Reads all 35 gem stack counts from the stash gem tab (7 gem types by
  5 tiers) using template matching against the game's Exocet font. No OCR
  and no per-user calibration.
- Plans transmutations bottom-up while respecting a minimum to keep per
  type and tier, a starting tier, and the game's 99-per-stack limit.
- Executes the Ctrl+Shift clicks through XTest, verifies every action by
  re-reading the table, and audits all 35 cells every N actions.
- Does not silently resynchronize after an error. It stops, keeps a
  screenshot, and empties the cube when it safely can.

`run` simulates by default. Clicks only happen with `--execute`.

## Requirements

- Linux with X11 or XWayland.
- One measured profile: 2560x1440 (`profiles/2560x1440.toml`).
- In game: the stash open on the gem tab, the Horadric Cube open and empty.
- Rust (stable) to build.

Other resolutions are not supported yet. The command-line output is currently
in French; there is no i18n.

## Build

```sh
cargo build --release
```

## Usage

Perform transmutations until no valid action remains:

```sh
cargo run --release -- run --execute --until-done
```

`run` simulates by default; without `--execute`, nothing is clicked. The
`inspect`, `plan`, and `capture` subcommands are read-only helpers.

Useful options:

- `--start-tier N` (`0` to `3`) restricts which source tiers may be
  converted; lower tiers are left untouched.
- `--max-transmutations N` limits the run to N conversions. It is
  incompatible with `--until-done`.
- `--profile` and `--config` select another layout or keep prescription
  (see Configuration).

## Configuration

`quickgems.toml` sets the minimum to keep for each gem type and tier. The
planner never goes below it. Keys follow the in-game French names, in tier
order: `abime`, `imparfait`, `normal`, `sans_defaut`, `parfait`.

Overrides are optional; omitted fields fall back to `[keep.default]`.

`--config` is optional. The configuration is resolved in this order:

1. the path given with `--config`;
2. the platform user configuration file, if present. On Linux this is
   `$XDG_CONFIG_HOME/d2rotw-quickgems/quickgems.toml`, or
   `~/.config/d2rotw-quickgems/quickgems.toml` when `XDG_CONFIG_HOME` is
   unset;
3. the built-in default configuration.

If the user configuration file does not exist, it is created on first use
from the built-in default, so you can find and edit it directly. It is never
overwritten. An existing but invalid user configuration is an error; it is
never silently ignored.

`--profile` is optional; the built-in 2560x1440 profile is used when omitted.

## Safety

- `F8` pauses and resumes. On resume, the game is re-read before continuing.
- `F9` or `Ctrl+C` stops. Moving the mouse also stops the run.
- After a safety stop, the program does not click again and does not try to
  recover. Gems may be left inside the cube.
- Logs and error screenshots are written to
  `$TMPDIR/d2rotw-quickgems-<random>/`, never to the current directory. The
  journal is `quickgems.log`; an error screenshot is `error-<epoch>.png`.

## Limitations

- Transmutation is lossy: 3 source gems become 1 gem of the next tier. Every
  automated conversion consumes real resources and cannot be undone.
- Only 2560x1440 is calibrated. The reader relies on measured pixel
  positions; UI scale, language, or layout changes will break it. For another
  resolution, open an issue with a full-window screenshot of the stash gem
  tab.
- A stack holds at most 99 gems per cell. The planner accounts for this, but
  a full stack may still need manual attention.
- This is a personal prototype. Use at your own risk; no warranty.

## Feedback

The tool is tuned to my setup. If your resolution, UI language, or layout
differs, open an issue and I can look into it; adjustments can be made on
request.

## Tests

```sh
cargo test --release
```

Unit tests cover the digit reader (with PNG fixtures), planner invariants,
config parsing, and action verification rules.

## Layout

- `src/vision/`: digit templates, gem table, cube grid, tab check.
- `src/planner.rs`: bottom-up planner and its invariants.
- `src/config.rs`: default `keep` plus per-type overrides.
- `src/platform/x11.rs`: capture, focus, XTest, hotkeys, pointer tracking.
- `src/executor.rs`: run loop, action verification, audit, recovery, logging.
- `src/logging.rs`: console and file journal in the per-run temporary directory.
- `profiles/2560x1440.toml`: measured layout, tabs, and click points.
- `assets/exocet-blizzard-medium.ttf`: the game font used for templates.

## Platform backends

All OS-specific code lives behind four traits in `src/platform/`: window
discovery and focus, screen capture, input injection, and control signals.
The current backend targets X11/XWayland. To add another platform, see
[docs/platform.md](docs/platform.md).

## License

The code is licensed under the GNU General Public License, version 3
(GPL-3.0). See [LICENSE](LICENSE).

`assets/exocet-blizzard-medium.ttf` is not covered by this license. Its
embedded metadata identifies it as Exocet, copyright Emigre Inc., designed by
Jonathan Barnbrook, all rights reserved.
