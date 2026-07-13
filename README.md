# zellij-cockpit

An all-in-one [Zellij](https://zellij.dev) **top bar**: live system load, coding-agent
usage (Claude Code and/or Codex CLI), and per-tab attention icons — in the single
tab-bar row, so it costs no extra vertical space.

```
 1 edit ●  2 build ◐  3 logs ✓   CPU 12%  MEM 9.4/16G  SWAP 0  Claude $4.21·312k  5h ▓▓▓░░ 2h09m left  Codex $0.01·27k  5h ▓░░░ 3h25m left
 └──────── tabs + attention ──────┘   └───────────────────────── system + per-agent usage ─────────────────────────┘
  legend:  ● needs you    ◐ working    ✓ done    (no icon = idle)
```

## What it shows

- **Tabs** with the active tab highlighted, plus a per-tab **attention icon**:
  - `◐` working — Claude is running in a pane on that tab
  - `●` needs you — Claude is waiting for input/permission
  - `✓` done — Claude finished
  - the icon clears when you focus the tab
- **CPU %** and **Memory** (used/total), color-coded by load
- **Swap** used, and on macOS the **memory pressure** that colors the MEM number.
  Used memory is a poor health signal on a Mac: the kernel keeps caches and compressible
  pages resident, so "used" sits near total even when everything is fine.
  Pressure measures the memory the kernel *cannot* reclaim cheaply, which is what
  Activity Monitor graphs, and swap is what tells you the machine is actually paging to disk.
- **Per coding agent** (Claude and Codex, each toggleable — see Config):
  - **today** — estimated list-price cost ($) and tokens since local midnight
  - **window** — a bar plus time until the rate-limit window resets. For Codex this uses its
    real `rate_limits` (actual % used + exact reset); for Claude it's the 5-hour rolling block.
- **Display presets** (`compact`, `balanced`, `full`) plus configurable glyphs,
  ASCII-safe icons, segment toggles, and color thresholds.

A provider is shown only when it's enabled *and* has data, so you get Claude-only, Codex-only,
or both. When the terminal is narrow, the right-hand segments drop one at a time so the tabs
always stay visible.

## How it works

No long-running daemon and no lock files. Two pieces:

```
 ┌─ zellij top bar (default_tab_template, 1 row) ─┐
 │  zellij-cockpit.wasm  (renders the bar)        │
 └──────────────┬─────────────────────────────────┘
   Timer ~3s →  │ run_command("cockpit-helper")        ← system + Claude metrics, as JSON
   pipe      ←  │ "cockpit::attention::<state>::<pane>" ← from Claude Code hooks
```

- **`cockpit-helper`** (native binary) reads CPU/MEM via `sysinfo` and computes per-agent
  usage by scanning each agent's local logs against a built-in price table:
  - Claude — `~/.claude/projects/**/*.jsonl`
  - Codex — `~/.codex/sessions/**/rollout-*.jsonl` (also reads Codex's real `rate_limits`)

  It's short-lived — the plugin runs it on a timer; the log scans are cached for ~30s.
  The today/window aggregation is shared (`src/usage.rs`); each agent just parses its own logs.
- **`zellij-cockpit.wasm`** (the plugin) renders the row, polls the helper, and listens for
  attention pipes sent by Claude Code hooks.

## Build & install

Requires Rust 1.85+ (edition 2024), the `wasm32-wasip1` target
(`rustup target add wasm32-wasip1`), [`just`](https://github.com/casey/just),
and Zellij.

```bash
just install
```

This builds both binaries and copies them to `~/.config/zellij/plugins/`
(`zellij-cockpit.wasm` and `cockpit-helper`). Then:

1. **Add the top bar.** Put the `default_tab_template` block from
   [`assets/layout.kdl`](assets/layout.kdl) into your Zellij config or a layout file.

2. **Enable attention hooks.** Merge [`assets/cockpit-hooks.json`](assets/cockpit-hooks.json)
   into `~/.claude/settings.json` (merge — don't overwrite any hooks you already have). The
   hooks fire `zellij pipe` on `UserPromptSubmit` / `Notification` / `Stop`.

On first run, Zellij prompts to grant the plugin **RunCommands** and
**ReadApplicationState** permissions — accept them, or the helper can't run and the
plugin can't map panes to tabs for attention icons.

### Platform support

Works on macOS and on Linux, including **WSL2** (Debian and friends). The only metric whose
meaning is platform-dependent is memory:

| Platform | What colors MEM | SWAP |
|----------|-----------------|------|
| macOS | **Memory pressure** (`vm_stat`: wired + compressor pages) | shown |
| Linux / WSL2 | **used / total** against `mem_warn` / `mem_crit` | shown when the system has swap |

macOS *needs* pressure because its "used" figure counts reclaimable caches. Linux does not have
that problem: there, used is `MemTotal - MemAvailable`, which already excludes the page cache, so
used/total is a truthful signal on its own and is what colors MEM.

On WSL2, every number describes the **WSL VM**, not Windows: the memory total is what WSL was
given, not your machine's RAM. WSL2 configures a swap file by default, so SWAP shows up; if you
set `swap=0` in `.wslconfig`, the segment hides itself.

Building on Debian additionally needs a C linker for the native helper:

```bash
sudo apt install build-essential
rustup target add wasm32-wasip1
just install
just doctor        # prints exactly what the bar will show for CPU/MEM/SWAP
```

### Reloading after a change

You don't need to restart Zellij or kill your session.

```bash
just reload                          # layout's plugin block has no config keys
just reload preset=full,interval=2   # layout's plugin block sets preset/interval
```

This rebuilds, installs, and hot-swaps the bar in place: same session, same panes, new code.

The `config` argument **must match the plugin block in your layout**. Zellij identifies a running
plugin by its url *and* its configuration, so if the config doesn't match it won't recognize the
bar that's already running and will open the plugin in a **new pane** instead of reloading it.
(If that happens, just close the stray pane.)

Changes to the **helper** alone need no reload at all: the plugin re-spawns it on every tick, so
`just install` is enough and the new numbers appear on the next refresh.

## Configuration

Optional keys in the plugin block (see `assets/layout.kdl`):

| Key        | Default                                       | Meaning                              |
|------------|-----------------------------------------------|--------------------------------------|
| `interval` | `3`                                           | seconds between metric refreshes     |
| `helper`   | `$HOME/.config/zellij/plugins/cockpit-helper` | path to the helper binary            |
| `preset`   | `balanced`                                    | `compact`, `balanced`, or `full`     |
| `claude`   | `true`                                        | show Claude usage                    |
| `codex`    | `true`                                        | show Codex usage (when `~/.codex` has logs) |

Preset defaults:

| Preset     | Behavior |
|------------|----------|
| `compact`  | CPU/MEM/SWAP plus provider rate-limit windows; hides cost and token totals |
| `balanced` | current default: CPU/MEM/SWAP, cost, tokens, window bar, percent, provider labels |
| `full`     | everything `balanced` shows, plus the memory-pressure bar and percentage next to MEM |

Any explicit toggle overrides the preset:

| Toggle | Meaning |
|--------|---------|
| `cpu`, `mem`, `swap` | show or hide system segments |
| `pressure` | show the memory-pressure bar and percentage next to MEM (macOS). Pressure colors the MEM number either way; this only controls whether the number itself is drawn |
| `cost`, `tokens`, `window`, `percent`, `provider_labels` | show or hide provider segment details |

The SWAP segment is hidden on systems with no swap configured.
Where memory pressure is unavailable (everything but macOS), MEM falls back to being colored
by used/total against `mem_warn` / `mem_crit`.

Booleans accept `true/false`, `1/0`, `yes/no`, and `on/off`.

Glyph and threshold keys:

| Key | Meaning |
|-----|---------|
| `ascii` | use ASCII-safe default attention glyphs (`~`, `!`, `+`) |
| `glyph_working`, `glyph_waiting`, `glyph_done` | override attention glyphs |
| `cpu_warn`, `cpu_crit`, `mem_warn`, `mem_crit`, `window_warn`, `window_crit` | warning/critical percentages |
| `pressure_warn`, `pressure_crit` | memory-pressure percentages that color MEM on macOS (default `50` / `75`) |
| `swap_warn_gb`, `swap_crit_gb` | swap thresholds in **gigabytes**, not percent (default `1` / `4`). macOS grows its swap file on demand, so a percentage of total swap means nothing |

The layout also keeps zellij's built-in `status-bar` (keybinding hints) at the bottom — see
[`assets/layout.kdl`](assets/layout.kdl).

Examples:

```kdl
// Minimal and portable.
preset "compact"
ascii "true"
```

```kdl
// Keep the default density but hide money.
preset "balanced"
cost "false"
```

```kdl
// Explicit high-detail layout with custom attention markers.
preset "full"
glyph_working "..."
glyph_waiting "!"
glyph_done "ok"
window_crit "90"
```

## Pricing

Model prices live in the per-agent `pricing.rs` files
([`src/claude/pricing.rs`](src/claude/pricing.rs), [`src/codex/pricing.rs`](src/codex/pricing.rs)),
in USD per 1M tokens with cache multipliers. These are list-price estimates for
display, not a billing authority; subscription plans, credits, regional pricing,
fast modes, and provider price changes can make real account cost differ. Update
them there when prices change.

## Troubleshooting

- **No metrics / blank right side** — run `just helper` (or `cockpit-helper`) directly; it should
  print one JSON line. If not, check the helper path in your plugin config.
- **Install/config uncertainty** — run `just doctor`; it checks the helper, cache
  directory, Zellij on PATH, installed plugin files, Claude settings, and local agent logs.
- **Attention icons never appear** — confirm the hooks are in `~/.claude/settings.json` and that
  `zellij` is on PATH inside Claude Code. Test manually:
  `zellij pipe --name "cockpit::attention::waiting::$ZELLIJ_PANE_ID"`.
- **Permission errors** — reload the plugin and accept the RunCommands and
  ReadApplicationState prompts.

## Background

Started as a fix/rewrite after [zellij-load](https://github.com/Christian-Prather/zellij-load)'s
daemon turned out to be broken on macOS. zellij-cockpit keeps the good idea (system load in the
bar), drops the fragile daemon/lock-file design, moves everything to the top row, and adds Claude
usage + tab attention. Design owes thanks to
[zellaude](https://github.com/ishefi/zellaude) (top-bar via `default_tab_template`, hook bridge)
and [zellij-attention](https://github.com/KiryuuLight/zellij-attention) (broadcast-pipe attention).

## License

MIT — see [LICENSE](LICENSE).
