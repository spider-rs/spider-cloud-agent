# Off-screen plumbing for the scenes a person types by hand: the install and a
# first page with no account, `setup`, `login --device`, and a signed-in crawl
# of a whole site into rows.
#
# The tape hides these lines with vhs Hide. `open` moves the tape's shell
# somewhere empty and `close` hands the take to entry-scene.py to be judged.
#
# The first three scenes show a new machine, so `open` gives them an empty home
# under /tmp with no key in the environment and none on disk. What is on screen
# is then what a person with nothing installed would see, and every path the
# scene prints is a path in that home rather than one on the recording machine.
#
# The signed-in scene keeps the real home, because the key it uses is the one
# the CLI already has stored. It works in a scratch directory, and the hidden
# setup leaves one file there: the selectors the command on screen names.
#
# Sourced, not run, because it has to move the tape's own shell. Nothing here
# sets shell options: one bad exit would take the recording down with it.
case "$1" in
  open)
    SCENE_NAME="$2"
    SCENE_ROOT="$PWD"
    printf 'pending\n' > "$SCENE_ROOT/media/out/$SCENE_NAME.exit"
    rm -f "$SCENE_ROOT/media/out/$SCENE_NAME.summary.json" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.started" \
          "$SCENE_ROOT/media/out/$SCENE_NAME.finished"

    case "$SCENE_NAME" in
      store-to-rows)
        SCENE_WORK="$(mktemp -d)"
        cd "$SCENE_WORK" || return 1
        printf '%s\n' '{"title":".product_main h1","price":".product_main .price_color"}' > fields.json
        ;;
      *)
        SCENE_WORK=/tmp/spider-demo
        rm -rf /tmp/spider-demo
        mkdir -p /tmp/spider-demo
        cd /tmp/spider-demo || return 1

        export HOME=/tmp/spider-demo
        unset SPIDER_API_KEY SPIDER_CLOUD_API_KEY XDG_BIN_HOME XDG_CONFIG_HOME XDG_DATA_HOME
        # The home's own bin directory goes first, which is where the installer
        # puts the binary, so the scene runs what it installed and nothing older.
        export PATH="/tmp/spider-demo/.local/bin:$PATH"

        if [ "$SCENE_NAME" != first-run ]; then
          # The other scenes start one step later: the binary is already there.
          mkdir -p .local/bin
          cp "$SCENE_ROOT/target/release/spider-agent" .local/bin/spider-agent
        fi
        if [ "$SCENE_NAME" = setup ]; then
          # Three of the clients are found by the directory they keep their
          # settings in. Claude Code and Codex are found on PATH and asked
          # through their own commands, which write under this home too.
          mkdir -p .cursor .codeium/windsurf "Library/Application Support/Claude"
        fi
        ;;
    esac

    scene_close() { source "$SCENE_ROOT/media/demo/entry-scene.sh" close; }

    # bash expands PS0 once a command has been read and before it runs, and
    # runs PROMPT_COMMAND when it returns. Between them they time each command
    # the scene types and keep its exit status, with none of the typing and
    # none of the tape's fixed sleeps in the number.
    PS0='$(python3 "$SCENE_ROOT/media/demo/entry-scene.py" start "$SCENE_NAME")'
    PROMPT_COMMAND='python3 "$SCENE_ROOT/media/demo/entry-scene.py" finish "$SCENE_NAME" "$?"'
    clear
    ;;

  close)
    PS0=''
    PROMPT_COMMAND=''
    python3 "$SCENE_ROOT/media/demo/entry-scene.py" score "$SCENE_NAME" "$SCENE_WORK"
    cd "$SCENE_ROOT" || return 1
    case "$SCENE_WORK" in
      /tmp/spider-demo) rm -rf /tmp/spider-demo ;;
      *) rm -f "$SCENE_WORK/fields.json" "$SCENE_WORK/books.ndjson"; rmdir "$SCENE_WORK" ;;
    esac
    ;;
esac
