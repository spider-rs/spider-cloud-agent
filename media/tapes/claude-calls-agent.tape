Output media/out/frames/claude-calls-agent/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1850
Set Height 300
Set Padding 32
Set Framerate 24
Set TypingSpeed 10ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open claude-calls-agent"
Enter
Sleep 3s
Show

Type "claude -p 'spider-agent scrape https://example.com --selectors fields.json --json --budget 2 --wall 20 --no-router."
Enter
Type "Print 2 lines, no markdown: the command, then served/refused/cost_credits/elapsed_ms JSON.' --allowed-tools Bash --model sonnet"
Sleep 300ms
Enter
Sleep 35s

Hide
Type@1ms "scene_close"
Enter
Sleep 3s
