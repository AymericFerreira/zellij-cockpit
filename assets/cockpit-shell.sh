# zellij-cockpit shell integration: tell the bar when a command is running.
#
# Source this from ~/.zshrc (zsh) or ~/.bashrc (bash):
#
#     [ -f ~/.config/zellij/plugins/cockpit-shell.sh ] && . ~/.config/zellij/plugins/cockpit-shell.sh
#
# Before every command the shell sends "start" to the bar, and after it sends
# "end". The bar marks the tab only once the command is still running at the
# next refresh, so short commands never make it blink.

# Only inside a zellij pane, and only for interactive shells ("i" in $-).
case "$-" in
  *i*) _cockpit_interactive=1 ;;
  *) _cockpit_interactive="" ;;
esac

if [ -n "$ZELLIJ" ] && [ -n "$ZELLIJ_PANE_ID" ] && [ -n "$_cockpit_interactive" ] &&
  command -v zellij >/dev/null 2>&1; then

  # This shell's "era" is its start time, and the counter restarts with it.
  # The bar orders messages by (era, counter), so a shell started later in the
  # same pane always wins - otherwise a counter restarting at 1 after
  # `exec zsh` would look old and every message would be dropped.
  _COCKPIT_ERA=$(date +%s 2>/dev/null || echo 0)
  _COCKPIT_SEQ=0

  # The pipe runs in the background: it talks to the zellij server over a
  # socket, and no prompt should wait on that. Background delivery can reorder,
  # so each message carries a counter and the plugin drops the older one.
  #
  # stdin MUST come from /dev/null. `zellij pipe` reads stdin for the payload,
  # and a background process reading the terminal is stopped with SIGTTIN - the
  # pipe would then never reach the plugin.
  _cockpit_activity() {
    ( zellij pipe \
      --name "cockpit::activity::$1::${ZELLIJ_PANE_ID}::${_COCKPIT_ERA}::${_COCKPIT_SEQ}" \
      </dev/null >/dev/null 2>&1 & )
  }

  # Commands that never mark the tab. A coding agent is one foreground command
  # that runs for hours, so it would keep its tab marked the whole session - and
  # the bar already says what those are doing, with their own attention icons.
  # Override with COCKPIT_SKIP="claude codex vim" before sourcing this file.
  COCKPIT_SKIP="${COCKPIT_SKIP:-claude codex}"

  # The command word, past anything that only wraps it: leading VAR=value
  # assignments and runners like sudo or nohup. `sudo claude` is still claude.
  _cockpit_command_word() {
    _cw_rest=$1
    while [ -n "$_cw_rest" ]; do
      _cw_word=${_cw_rest%% *}
      case "$_cw_word" in
        *=*) ;;
        sudo | command | exec | nohup | time | env | doas) ;;
        *)
          printf '%s' "${_cw_word##*/}"
          return 0
          ;;
      esac
      case "$_cw_rest" in
        *" "*) _cw_rest=${_cw_rest#* } ;;
        *) _cw_rest="" ;;
      esac
    done
    return 0
  }

  _cockpit_skipped() {
    case " $COCKPIT_SKIP " in
      *" $(_cockpit_command_word "$1") "*) return 0 ;;
    esac
    return 1
  }

  # A skipped command still sends "end": that clears any marker left over from
  # whatever ran before it, so the tab is honest either way.
  _cockpit_start() {
    _COCKPIT_SEQ=$((_COCKPIT_SEQ + 1))
    if _cockpit_skipped "$1"; then
      _cockpit_activity end
    else
      _cockpit_activity start
    fi
  }

  _cockpit_end() {
    _COCKPIT_SEQ=$((_COCKPIT_SEQ + 1))
    _cockpit_activity end
  }

  if [ -n "$ZSH_VERSION" ]; then
    autoload -Uz add-zsh-hook
    # preexec gets the command line as typed in $1.
    add-zsh-hook preexec _cockpit_start
    add-zsh-hook precmd _cockpit_end
  elif [ -n "$BASH_VERSION" ]; then
    # DEBUG fires before each command, including the ones PROMPT_COMMAND runs
    # itself - $_COCKPIT_IN_PROMPT keeps those from counting as your command.
    _cockpit_debug() {
      case "$BASH_COMMAND" in
        _cockpit_*|"$PROMPT_COMMAND") return 0 ;;
      esac
      [ -n "$_COCKPIT_IN_PROMPT" ] && return 0
      _COCKPIT_IN_PROMPT=1
      _cockpit_start "$BASH_COMMAND"
    }
    _cockpit_prompt() {
      [ -n "$_COCKPIT_IN_PROMPT" ] && _cockpit_end
      _COCKPIT_IN_PROMPT=""
    }
    trap '_cockpit_debug' DEBUG
    case "$PROMPT_COMMAND" in
      *_cockpit_prompt*) ;;
      "") PROMPT_COMMAND="_cockpit_prompt" ;;
      *) PROMPT_COMMAND="_cockpit_prompt;$PROMPT_COMMAND" ;;
    esac
  fi
fi
