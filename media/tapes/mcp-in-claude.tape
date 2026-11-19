Output media/out/frames/mcp-in-claude/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1560
Set Height 400
Set Padding 32
Set Framerate 24
Set TypingSpeed 40ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open mcp-in-claude"
Enter
Sleep 3s
Show

Type "claude -p 'Use spider to search the web for the Tokio 1.0 release announcement, then read it. In three"
Enter
Type `lines, what did it promise? End with the address you read.' \`
Enter
Type "--mcp-config mcp.json --strict-mcp-config --allowed-tools mcp__spider --model sonnet"
Sleep 300ms
Enter
Sleep 120s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
