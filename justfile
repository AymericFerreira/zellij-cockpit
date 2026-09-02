plugins_dir := env_var('HOME') / ".config/zellij/plugins"

[group("build")]
build-all:
    just build-helper
    just build-plugin

[group("build")]
build-helper:
    cargo build --release --bin cockpit-helper --features native

[group("build")]
build-plugin:
    cargo build --target wasm32-wasip1 --release --bin zellij-cockpit --features plugin

[group("test")]
test:
    cargo test --features native

# The shell hooks, in real zsh and bash under a pty. Slower (a few minutes): it
# drives interactive shells with a stub `zellij` and reads back what they sent.
# SHELLS=bash just test-shell runs one shell.
[group("test")]
test-shell:
    sh tests/shell_hooks.sh

[group("test")]
test-all: test test-shell

# Build, then copy the wasm plugin and native helper into the zellij plugins dir.
[group("install")]
install: build-all
    mkdir -p "{{plugins_dir}}"
    cp target/wasm32-wasip1/release/zellij-cockpit.wasm "{{plugins_dir}}/"
    cp target/release/cockpit-helper "{{plugins_dir}}/"
    cp assets/cockpit-shell.sh "{{plugins_dir}}/"
    @echo ""
    @echo "Installed plugin + helper to {{plugins_dir}}"
    @echo "Next:"
    @echo "  1. Add the default_tab_template from assets/layout.kdl to your zellij config/layout."
    @echo "  2. Merge assets/cockpit-hooks.json into ~/.claude/settings.json (keep your existing hooks)."
    @echo "  3. Source cockpit-shell.sh from your ~/.zshrc or ~/.bashrc for the running-command marker."

[group("install")]
uninstall:
    rm -f "{{plugins_dir}}/zellij-cockpit.wasm" "{{plugins_dir}}/cockpit-helper" "{{plugins_dir}}/cockpit-shell.sh"

# Rebuild + install, then hot-reload the bar in the running zellij session (no restart).
#
# `config` MUST match the plugin block in your layout, e.g. `just reload preset=full,interval=2`.
# Zellij keys a plugin instance by url *and* configuration: with a mismatched config it does not
# recognize the running bar, and opens the plugin in a new pane instead of reloading it.
#
# The helper needs no reload - the plugin re-spawns it every tick, so `just install` alone is
# enough for helper-only changes to appear on the next refresh.
[group("run")]
reload config="": install
    zellij action start-or-reload-plugin {{ if config == "" { "" } else { "-c " + config } }} "file:{{plugins_dir}}/zellij-cockpit.wasm"
    @echo ""
    @echo "Reloaded the bar in this session."

# Print the metrics line the plugin consumes (for debugging the helper).
[group("run")]
helper:
    cargo run --release --bin cockpit-helper --features native

# Check local install/config prerequisites without starting zellij.
[group("run")]
doctor:
    cargo run --release --bin cockpit-helper --features native -- doctor
