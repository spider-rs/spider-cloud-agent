Output media/out/frames/codex-calls-agent/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1800
Set Height 330
Set Padding 32
Set Framerate 24
Set TypingSpeed 35ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open codex-calls-agent"
Enter
Sleep 3s
Show

Type "codex exec -p spider --json 'Run spider-agent schema, then pull the title, price and stock of each"
Enter
Type "book on the books.toscrape.com front page, without the page body crossing the wire' | tee turn.jsonl |"
Enter
Type "jq -rs 'map(.item.text? // empty)[-1]'"
Sleep 300ms
Enter
Sleep 180s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
