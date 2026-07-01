# zellij-cockpit

An all-in-one [Zellij](https://zellij.dev) **top bar**: live system load, coding-agent
usage (Claude Code and/or Codex CLI), and per-tab attention icons — in the single
tab-bar row, so it costs no extra vertical space.

```
 1 edit ●  2 build ◐  3 logs ✓   CPU 12%  MEM 9.4/16G  Claude $4.21·312k  5h ▓▓▓░░ 2h09m left  Codex $0.01·27k  5h ▓░░░ 3h25m left
 └──────── tabs + attention ──────┘   └────────────────────── system + per-agent usage ──────────────────────┘
  legend:  ● needs you    ◐ working    ✓ done    (no icon = idle)
```

## What it shows

- **Tabs** with the active tab highlighted, plus a per-tab **attention icon**:
  - `◐` working — Claude is running in a pane on that tab
  - `●` needs you — Claude is waiting for input/permission
  - `✓` done — Claude finished
  - the icon clears when you focus the tab
- **CPU %** and **Memory** (used/total), color-coded by load
- **Per coding agent** (Claude and Codex, each toggleable — see Config):
  - **today** — estimated list-price cost ($) and tokens since local midnight
  - **window** — a bar plus time until the rate-limit window resets. For Codex this uses its
    real `rate_limits` (actual % used + exact reset); for Claude it's the 5-hour rolling block.

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

## Configuration

Optional keys in the plugin block (see `assets/layout.kdl`):

| Key        | Default                                          | Meaning                              |
|------------|--------------------------------------------------|--------------------------------------|
| `interval` | `3`                                              | seconds between metric refreshes     |
| `helper`   | `$HOME/.config/zellij/plugins/cockpit-helper`    | path to the helper binary            |
| `claude`   | `true`                                           | show Claude usage                    |
| `codex`    | `true`                                           | show Codex usage (when `~/.codex` has logs) |

The layout also keeps zellij's built-in `status-bar` (keybinding hints) at the bottom — see
[`assets/layout.kdl`](assets/layout.kdl).

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
