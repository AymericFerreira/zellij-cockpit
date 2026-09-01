# zellij-cockpit

An all-in-one [Zellij](https://zellij.dev) **top bar**: live system load, coding-agent
usage (Claude Code and/or Codex CLI), and per-tab attention icons — in the single
tab-bar row, so it costs no extra vertical space.

```
 1 edit ●  2 build ▶ ◐  3 logs ✓   CPU 12%  MEM 9.4/16G  SWAP 0  Claude $4.21·312k  5h ▓▓▓░░ 2h09m left  Codex $0.01·27k  5h ▓░░░ 3h25m left
 └──────── tabs + attention ──────┘   └───────────────────────── system + per-agent usage ─────────────────────────┘
  legend:  ● needs you    ◐ working    ✓ done    ▶ command running    (no icon = idle)
```

## What it shows

- **Tabs** with the active tab highlighted, plus a per-tab **attention icon**:
  - `◐` working — Claude is running in a pane on that tab
  - `●` needs you — Claude is waiting for input/permission
  - `✓` done — Claude finished
  - the icon clears when you focus the tab
  - `▶` a shell command is running in a pane on that tab.
    This one stays on the tab you are looking at: it tells you the command has not finished yet.
- **CPU %** and **Memory** (used/total), color-coded by load
- **Swap** used, and on macOS the **memory pressure** that colors the MEM number.
  Used memory is a poor health signal on a Mac: the kernel keeps caches and compressible
  pages resident, so "used" sits near total even when everything is fine.
  Pressure measures the memory the kernel *cannot* reclaim cheaply, which is what
  Activity Monitor graphs, and swap is what tells you the machine is actually paging to disk.
- **Per coding agent** (Claude and Codex, each toggleable — see Config):
  - **today** — estimated list-price cost ($) and tokens since local midnight
  - **window** — the real rate-limit quota: percent actually used and the exact reset time,
    the same numbers Claude Code's `/usage` and Codex's own limits report. If the real quota
    can't be fetched, the bar shows the window **without a percentage** rather than guess (see
    [Rate-limit window](#rate-limit-window)).
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
   pipe      ←  │ "cockpit::activity::<start|end>::…"   ← from shell preexec/precmd
```

- **`cockpit-helper`** (native binary) reads CPU/MEM via `sysinfo` and computes per-agent
  usage by scanning each agent's local logs against a built-in price table:
  - Claude — `~/.claude/projects/**/*.jsonl`
  - Codex — `~/.codex/sessions/**/rollout-*.jsonl` (also reads Codex's real `rate_limits`)

  It's short-lived — the plugin runs it on a timer; the log scans are cached for ~30s.
  The today/window aggregation is shared (`src/usage.rs`); each agent just parses its own logs.
- **`zellij-cockpit.wasm`** (the plugin) renders the row, polls the helper, and listens for
  attention and activity pipes sent by Claude Code hooks and by your shell.

### Running-command marker

Zellij's plugin API does not expose what a pane is running, so the shell has to say it.
`assets/cockpit-shell.sh` hooks `preexec` and `precmd` (zsh) or `DEBUG` and `PROMPT_COMMAND`
(bash) and fires one `zellij pipe` around each foreground command. The pipe runs in the
background, so your prompt never waits on it.

The pipe is sent with stdin on `/dev/null`. `zellij pipe` reads stdin for its payload, and a
background process that reads the terminal is stopped with `SIGTTIN`, so without that redirect
the message never arrives.

Two details keep the marker honest:

- **No flicker.** A command only draws the marker if it is still running at the next refresh
  (~3s). `ls` and `cd` never show up.
- **No stuck marker.** Background pipes can arrive out of order, so each message carries the
  shell's start time (its *era*) and a command counter. Within one era the bar ignores an older
  counter. A *different* era just means a new shell in that pane, and the bar believes it
  whatever the value - eras are never compared. That matters: if the bar ranked eras, one bad
  value (a clock jump, a stray message) would make every later message look old and silence the
  pane for good. If a pane closes mid-command, its tab drops the marker on the next pane update.

If a marker ever gets stuck anyway, clear every one of them by hand:

```sh
zellij pipe --name "cockpit::activity::reset" </dev/null
```

The marker only covers commands the shell itself starts. Something launched inside a full-screen
program - a build from inside your editor, for example - is invisible to it.

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

3. **Enable the running-command marker.** Source the shell integration from your
   `~/.zshrc` (zsh) or `~/.bashrc` (bash):

   ```sh
   [ -f ~/.config/zellij/plugins/cockpit-shell.sh ] && . ~/.config/zellij/plugins/cockpit-shell.sh
   ```

   `just install` copies that file next to the plugin. See
   [Running-command marker](#running-command-marker).

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

Building on Debian additionally needs a C toolchain — to link the native helper, and to build the
TLS backend behind the live rate-limit lookup:

```bash
sudo apt install build-essential
rustup target add wasm32-wasip1
just install
just doctor        # prints exactly what the bar will show, including CPU/MEM/SWAP and the quota
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
| `activity` | show the running-command marker on tabs (default `true`) |
| `live` | fetch the real rate-limit quota (default `true`). `false` keeps the helper offline: no credentials read, no network, window falls back to an estimate. See [Rate-limit window](#rate-limit-window) |

The SWAP segment is hidden on systems with no swap configured.
Where memory pressure is unavailable (everything but macOS), MEM falls back to being colored
by used/total against `mem_warn` / `mem_crit`.

Booleans accept `true/false`, `1/0`, `yes/no`, and `on/off`.

Glyph and threshold keys:

| Key | Meaning |
|-----|---------|
| `ascii` | use ASCII-safe default glyphs (`~`, `!`, `+`, `>`) |
| `glyph_working`, `glyph_waiting`, `glyph_done` | override attention glyphs |
| `glyph_running` | override the running-command marker (default `▶`, `>` with `ascii`) |
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

## Rate-limit window

The percentage in the `5h` segment is **quota actually used**, fetched from the provider - not a
guess. For Claude it comes from `GET /api/oauth/usage`, the same endpoint behind Claude Code's
`/usage` command; for Codex it comes from the `rate_limits` its session logs already record.

This matters because the obvious local substitute is wrong. From the logs alone you can only tell
how much of the 5-hour window has *elapsed*, and elapsed time is not spent quota: a window can be
72% elapsed while you have used 12% of it. Showing that as "72%" reads like you are nearly out
when you are barely started.

So when the real quota is unavailable, the bar **shows no percentage at all** and dims the bar:

```
Claude · $4.21 · 312k | 5h ▓░░░░ 14% 4h05m left   ← real quota, colored
Claude · $4.21 · 312k | 5h ▓▓▓▓░ 4h05m left       ← estimate: bar dimmed, no percentage
```

The reset countdown is shown either way, because that part of the estimate is sound.

**What this costs you.** The helper reads the OAuth token Claude Code already stored (macOS
Keychain; `~/.claude/.credentials.json` elsewhere), sends it to Anthropic, and drops it - it is
never logged or written anywhere.

The endpoint is undocumented and **rate limits hard**: a handful of requests in a few minutes is
enough to trip it, and it then stays tripped for a long time. Being locked out is worse than being
slightly stale, since a lockout costs the percentage entirely. So the helper polls it slowly and
gives up quickly:

- a successful read is cached for **10 minutes** (six requests an hour, however fast the bar ticks)
- a `429` waits at least **30 minutes** before trying again
- any other failure backs off from 5 minutes, doubling to at most an hour
- a window older than **30 minutes** is discarded rather than presented as current

The countdown stays exact between fetches regardless, because what gets cached is the absolute
reset time, not a duration. Staleness therefore only ever affects the percentage, and only by
minutes.

Set `live "false"` to switch all of this off: no credentials are read, no request is made, and the
window falls back to the estimate. `just doctor` reports which one you are getting.

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
