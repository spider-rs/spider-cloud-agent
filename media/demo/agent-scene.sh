# Off-screen plumbing for the two scenes that record a coding agent CLI.
#
# The tape hides these lines with vhs Hide, so the only thing on a visible frame
# is the CLI invocation itself, what it printed, and one harness line under it.
# `open` puts the agent in an empty scratch directory, leaves the notes file
# that CLI reads in a working directory, starts a copy of everything printed
# there, and clears the screen. `close` stops the copy and hands it to
# agent-scene.py, which decides whether the take is usable.
#
# Sourced, not run, because it has to move the tape's own shell. Nothing here
# sets shell options: one bad exit would take the recording down with it.
scene_status=$?

case "$1" in
  open)
    SCENE_NAME="$2"
    SCENE_ROOT="$PWD"
    SCENE_WORK="$(mktemp -d)"
    printf 'pending\n' > "$SCENE_ROOT/media/out/$SCENE_NAME.exit"
    rm -f "$SCENE_ROOT/media/out/$SCENE_NAME.summary.json" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.stdout" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.started" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.finished"
    cd "$SCENE_WORK" || return 1

    # Codex refuses to run outside a repository unless it is told to. A coding
    # agent works in one anyway, so the scratch directory is one, and the
    # invocation on screen keeps a flag fewer.
    git init -q .

    # Where the shape of the answer lives. Each CLI reads the notes file in its
    # working directory, and a caller who wants plain lines back writes that
    # down once instead of retyping it. The task on screen is then the task.
    case "$SCENE_NAME" in
      claude-calls-agent) scene_notes=CLAUDE.md ;;
      *)                  scene_notes=AGENTS.md ;;
    esac
    cat > "$scene_notes" <<'NOTES'
Answer in plain lines and nothing else. No markdown, no fenced blocks, no prose.

1. the command you ran to find out what spider-agent can do
2. the spider-agent command that worked
3. served, refused, cost_credits and elapsed_ms out of its report, as one JSON
   object, credits rounded to six decimals

Selectors go in a file. Nobody can read a command with a JSON document inside it.
NOTES

    # The tape closes the scene by name. A tape string cannot carry a quoted
    # path, and an unquoted one would break on a scratch directory with a space.
    scene_close() { source "$SCENE_ROOT/media/demo/agent-scene.sh" close; }

    # bash expands PS0 once a command has been read and before it runs, so the
    # take is timed from the keystroke that submits the invocation rather than
    # from here, with a long string still left to type. PROMPT_COMMAND stops the
    # clock when the invocation returns, so the fixed stretch the tape sleeps
    # afterwards waiting on a slow one is left out. PS0 prints nothing. The
    # prompt hook prints the one harness line the scene shows, and prints it
    # only once the invocation it is reporting on has run.
    PS0='$(python3 -c "import time; print(int(time.time() * 1000))" >> "$SCENE_ROOT/media/out/$SCENE_NAME.started")'
    PROMPT_COMMAND='python3 "$SCENE_ROOT/media/demo/agent-scene.py" note "$SCENE_NAME" "$SCENE_WORK"'

    clear
    exec 3>&1
    exec > >(tee "$SCENE_ROOT/media/out/$SCENE_NAME.stdout")
    ;;

  close)
    exec 1>&3 3>&-
    PS0=''
    PROMPT_COMMAND=''
    # tee ends when the last writer lets go of the pipe. Give it that moment
    # before anything reads the file back.
    sleep 0.5
    python3 "$SCENE_ROOT/media/demo/agent-scene.py" score "$SCENE_NAME" "$scene_status"
    cd "$SCENE_ROOT" || return 1
    rm -rf "$SCENE_WORK"
    ;;
esac
