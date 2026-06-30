# zellij-cockpit

An all-in-one [Zellij](https://zellij.dev) **top bar**: live system load, Claude Code
usage, and per-tab attention icons — all in the single tab-bar row, so it costs no
extra vertical space.

```
 1 edit ●  2 build ◐  3 logs ✓     CPU 12%  MEM 9.4/16G  Claude $4.21·312k  5h ▓▓▓░░ 38%
 └──────── tabs + attention ───────┘   └─────────── system + Claude usage ───────────┘
  legend:  ● needs you    ◐ working    ✓ done    (no icon = idle)
```

## What it shows

- **Tabs** with the active tab highlighted, plus a per-tab **attention icon**:
  - `◐` working — Claude is running in a pane on that tab
  - `●` needs you — Claude is waiting for input/permission
  - `✓` done — Claude finished
  - the icon clears when you focus the tab
- **CPU %** and **Memory** (used/total), color-coded by load
- **Claude today** — total cost ($) and tokens across all projects since local midnight
- **Claude 5-hour block** — how far through the active rate-limit window you are

When the terminal is narrow, the right-hand metrics drop one at a time (5h → Claude → MEM → CPU)
so the tabs always stay visible.

## How it works

No long-running daemon and no lock files. Two pieces:

```
 ┌─ zellij top bar (default_tab_template, 1 row) ─┐
 │  zellij-cockpit.wasm  (renders the bar)        │
 └──────────────┬─────────────────────────────────┘
   Timer ~3s →  │ run_command("cockpit-helper")        ← system + Claude metrics, as JSON
   pipe      ←  │ "cockpit::attention::<state>::<pane>" ← from Claude Code hooks
```

- **`cockpit-helper`** (native binary) reads CPU/MEM via `sysinfo` and computes Claude
  cost/tokens by scanning `~/.claude/projects/**/*.jsonl` against a built-in price table.
  It's short-lived — the plugin runs it on a timer. The Claude scan is cached for ~30s.
- **`zellij-cockpit.wasm`** (the plugin) renders the row, polls the helper, and listens for
  attention pipes sent by Claude Code hooks.

## Build & install

Requires the Rust toolchain, the `wasm32-wasip1` target (`rustup target add wasm32-wasip1`),
[`just`](https://github.com/casey/just), and Zellij.

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

On first run, Zellij prompts to grant the plugin **RunCommands** permission — accept it, or the
helper can't run.

## Configuration

Optional keys in the plugin block (see `assets/layout.kdl`):

| Key        | Default                                          | Meaning                          |
|------------|--------------------------------------------------|----------------------------------|
| `interval` | `3`                                              | seconds between metric refreshes |
| `helper`   | `$HOME/.config/zellij/plugins/cockpit-helper`    | path to the helper binary        |

## Pricing

Model prices live in [`src/claude/pricing.rs`](src/claude/pricing.rs) (USD per 1M tokens, with
the cache-write ×1.25 / cache-read ×0.1 multipliers). Update them there when prices change.

## Troubleshooting

- **No metrics / blank right side** — run `just helper` (or `cockpit-helper`) directly; it should
  print one JSON line. If not, check the helper path in your plugin config.
- **Attention icons never appear** — confirm the hooks are in `~/.claude/settings.json` and that
  `zellij` is on PATH inside Claude Code. Test manually:
  `zellij pipe --name "cockpit::attention::waiting::$ZELLIJ_PANE_ID"`.
- **Permission errors** — reload the plugin and accept the RunCommands prompt.

## Background

Started as a fix/rewrite after [zellij-load](https://github.com/Christian-Prather/zellij-load)'s
daemon turned out to be broken on macOS. zellij-cockpit keeps the good idea (system load in the
bar), drops the fragile daemon/lock-file design, moves everything to the top row, and adds Claude
usage + tab attention. Design owes thanks to
[zellaude](https://github.com/ishefi/zellaude) (top-bar via `default_tab_template`, hook bridge)
and [zellij-attention](https://github.com/KiryuuLight/zellij-attention) (broadcast-pipe attention).

## License

MIT — see [LICENSE](LICENSE).
