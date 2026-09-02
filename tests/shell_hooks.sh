#!/bin/sh
# Tests for assets/cockpit-shell.sh: what the shell hooks actually send.
#
# Every case runs a real interactive shell under a pty, with a stub `zellij` on
# PATH that logs the pipe names it was called with. The pty matters: the hooks
# send their pipe in the background, and a background process that reads the
# terminal is stopped with SIGTTIN. Without a controlling terminal that bug is
# invisible, and it is the one that made the marker never appear at all.
#
# Run with: just test-shell

set -u

hook=$(cd "$(dirname "$0")/.." && pwd)/assets/cockpit-shell.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
log="$work/pipes.log"
failures=0

mkdir -p "$work/bin"
cat > "$work/bin/zellij" <<EOF
#!/bin/sh
echo "\$*" >> "$log"
EOF
chmod +x "$work/bin/zellij"

# Run a script in an interactive shell attached to a pty.
#
# The pty is the point: the hooks send their pipe in the background, and a
# background process that reads the terminal is stopped with SIGTTIN. Without a
# controlling terminal that whole class of bug is invisible.
#
# The shell is told to exit explicitly. On plain EOF `script` tears the pty down
# at once, killing the shell mid-command and any pipe still in flight with it.
# run_shell <shell> <script> [extra env assignments]
run_shell() {
  shell=$1
  script_text=$2
  extra_env=${3:-}
  : > "$log"
  # Feed one line at a time: a shell's line editor discards type-ahead when it
  # takes over the pty, so a script written all at once loses everything past
  # the first line.
  #
  # The session ends by closing stdin, after an idle pause. Nothing else is
  # typed: `exit` is a command like any other, and it would send messages of its
  # own into what the test reads back.
  feed() {
    printf '%s\n' "$script_text" | while IFS= read -r line; do
      printf '%s\n' "$line"
      sleep 0.6
    done
    sleep 3
  }
  # No rc files: the shell under test must run the hook from this repo, not the
  # copy the developer has installed in their own shell.
  case "$shell" in
    zsh) args="-f -is" ;;
    *) args="--norc -is" ;;
  esac
  if [ "$(uname)" = "Darwin" ]; then
    feed | env PATH="$work/bin:$PATH" ZELLIJ=0 ZELLIJ_PANE_ID=7 $extra_env \
      script -q /dev/null "$shell" $args >/dev/null 2>&1 &
  else
    feed | env PATH="$work/bin:$PATH" ZELLIJ=0 ZELLIJ_PANE_ID=7 $extra_env \
      script -qec "$shell $args" /dev/null >/dev/null 2>&1 &
  fi
  # Watchdog: a shell that ignores EOF must not wedge the suite.
  runner=$!
  waited=0
  while kill -0 "$runner" 2>/dev/null && [ "$waited" -lt 25 ]; do
    sleep 1
    waited=$((waited + 1))
  done
  kill -TERM "$runner" 2>/dev/null
  wait "$runner" 2>/dev/null
  sleep 1
}

# Every pipe name the run sent, one per line. Each run starts from an empty log,
# so the whole log is this run's story.
sent() { sed 's/^pipe --name //' "$log" 2>/dev/null; }

ok() { printf '  ok   %s\n' "$1"; }
fail() {
  printf '  FAIL %s\n' "$1"
  printf '       %s\n' "$2"
  failures=$((failures + 1))
}

expect_sent() {
  label=$1
  pattern=$2
  if sent | grep -q "$pattern"; then
    ok "$label"
  else
    fail "$label" "no line matching '$pattern' in: $(sent | tr '\n' ' ')"
  fi
}

expect_not_sent() {
  label=$1
  pattern=$2
  if sent | grep -q "$pattern"; then
    fail "$label" "unexpected line matching '$pattern' in: $(sent | tr '\n' ' ')"
  else
    ok "$label"
  fi
}

# SHELLS=bash tests/shell_hooks.sh runs just one shell.
for shell in ${SHELLS:-zsh bash}; do
  command -v "$shell" >/dev/null 2>&1 || {
    printf '%s: not installed, skipped\n' "$shell"
    continue
  }
  printf '%s\n' "$shell"

  # The core contract, and the SIGTTIN regression: with a terminal on stdin the
  # backgrounded pipe still has to reach the stub.
  run_shell "$shell" ". $hook
echo hello"
  expect_sent "a command sends start" "activity::start::7::"
  expect_sent "a command sends end" "activity::end::7::"

  # Agent sessions are one command that runs for hours; they must not mark the
  # tab. A skipped command still sends end, to clear whatever ran before it.
  run_shell "$shell" ". $hook
codex --resume"
  expect_not_sent "an agent command sends no start" "activity::start::"
  expect_sent "an agent command still sends end" "activity::end::7::"

  run_shell "$shell" ". $hook
sudo claude"
  expect_not_sent "a wrapped agent command sends no start" "activity::start::"

  run_shell "$shell" ". $hook
FOO=1 /usr/bin/claude --x"
  expect_not_sent "an agent behind assignments and a path sends no start" "activity::start::"

  # A command whose name merely starts the same way is not an agent.
  run_shell "$shell" ". $hook
claudette"
  expect_sent "a lookalike command still sends start" "activity::start::7::"

  # The override has to be set before the file is sourced, so it comes from the
  # environment: typing it would itself be a command, and show up in the log.
  run_shell "$shell" ". $hook
hugo build" "COCKPIT_SKIP=hugo"
  expect_not_sent "COCKPIT_SKIP overrides the default list" "activity::start::"

  # An overridden list replaces the default, it does not add to it.
  run_shell "$shell" ". $hook
codex --resume" "COCKPIT_SKIP=hugo"
  expect_sent "an overridden list no longer skips codex" "activity::start::7::"

  # Ordering: the plugin drops an older counter from the same shell, so the
  # counter has to keep rising within one shell.
  run_shell "$shell" ". $hook
echo one
echo two"
  count=$(grep -c 'activity::' "$log")
  actual=$(sed 's/.*:://' "$log" | tr '\n' ' ')
  sorted=$(sed 's/.*:://' "$log" | sort -n | tr '\n' ' ')
  if [ "$count" -ge 4 ] && [ "$sorted" = "$actual" ]; then
    ok "the counter rises with every message"
  else
    fail "the counter rises with every message" "saw: $actual"
  fi

  # Outside zellij the hooks must stay out of the way entirely.
  : > "$log"
  case "$shell" in
    zsh) args="-f -is" ;;
    *) args="--norc -is" ;;
  esac
  printf '%s\n' ". $hook
echo hello" | env -u ZELLIJ -u ZELLIJ_PANE_ID PATH="$work/bin:$PATH" \
    "$shell" $args >/dev/null 2>&1
  sleep 1
  if [ -s "$log" ]; then
    fail "no pipes outside zellij" "sent: $(cat "$log" | tr '\n' ' ')"
  else
    ok "no pipes outside zellij"
  fi
done

if [ "$failures" -eq 0 ]; then
  echo "shell hooks: all good"
  exit 0
fi
printf 'shell hooks: %s failure(s)\n' "$failures"
exit 1
