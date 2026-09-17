# Off-screen plumbing for the two scenes that record a coding agent CLI.
#
# The tape hides these lines with vhs Hide, so the only thing on a visible frame
# is the CLI invocation itself and what it printed. `open` puts the agent in a
# scratch directory holding the selectors file, starts a copy of everything
# printed there, and clears the screen. `close` stops the copy and hands it to
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
    cp "$SCENE_ROOT/media/demo/fields.json" "$SCENE_WORK/"
    printf 'pending\n' > "$SCENE_ROOT/media/out/$SCENE_NAME.exit"
    rm -f "$SCENE_ROOT/media/out/$SCENE_NAME.summary.json" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.stdout" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.started" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.finished"
    cd "$SCENE_WORK" || return 1

    # The tape closes the scene by name. A tape string cannot carry a quoted
    # path, and an unquoted one would break on a scratch directory with a space.
    scene_close() { source "$SCENE_ROOT/media/demo/agent-scene.sh" close; }

    # bash expands PS0 once a command has been read and before it runs, so the
    # take is timed from the keystroke that submits the invocation rather than
    # from here, with a long string still left to type. PROMPT_COMMAND stops the
    # clock when the invocation returns, so the fixed stretch the tape sleeps
    # afterwards waiting on a slow one is left out. Both write their stamp to a
    # file and print nothing, so no frame carries one.
    PS0='$(python3 -c "import time; print(int(time.time() * 1000))" >> "$SCENE_ROOT/media/out/$SCENE_NAME.started")'
    PROMPT_COMMAND='python3 -c "import time; print(int(time.time() * 1000))" > "$SCENE_ROOT/media/out/$SCENE_NAME.finished"'

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
    python3 "$SCENE_ROOT/media/demo/agent-scene.py" "$SCENE_NAME" "$scene_status"
    cd "$SCENE_ROOT" || return 1
    rm -rf "$SCENE_WORK"
    ;;
esac
