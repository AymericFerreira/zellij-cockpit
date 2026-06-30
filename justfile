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

# Build, then copy the wasm plugin and native helper into the zellij plugins dir.
[group("install")]
install: build-all
    mkdir -p "{{plugins_dir}}"
    cp target/wasm32-wasip1/release/zellij-cockpit.wasm "{{plugins_dir}}/"
    cp target/release/cockpit-helper "{{plugins_dir}}/"
    @echo ""
    @echo "Installed plugin + helper to {{plugins_dir}}"
    @echo "Next:"
    @echo "  1. Add the default_tab_template from assets/layout.kdl to your zellij config/layout."
    @echo "  2. Merge assets/cockpit-hooks.json into ~/.claude/settings.json (keep your existing hooks)."

[group("install")]
uninstall:
    rm -f "{{plugins_dir}}/zellij-cockpit.wasm" "{{plugins_dir}}/cockpit-helper"

# Print the metrics line the plugin consumes (for debugging the helper).
[group("run")]
helper:
    cargo run --release --bin cockpit-helper --features native
