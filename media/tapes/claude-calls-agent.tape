Output media/out/frames/claude-calls-agent/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1560
Set Height 300
Set Padding 32
Set Framerate 24
Set TypingSpeed 40ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open claude-calls-agent"
Enter
Sleep 3s
Show

Type "claude -p 'Run spider-agent schema, then pull the title, price and stock of each book on the"
Enter
Type "books.toscrape.com front page, without the page body crossing the wire' --allowed-tools Bash --model sonnet"
Sleep 300ms
Enter
Sleep 150s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
