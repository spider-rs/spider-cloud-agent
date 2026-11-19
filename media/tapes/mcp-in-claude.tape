Output media/out/frames/mcp-in-claude/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1560
Set Height 350
Set Padding 32
Set Framerate 24
Set TypingSpeed 40ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open mcp-in-claude"
Enter
Sleep 3s
Show

Type "claude -p 'Read https://example.com with the spider tool. Give me its title and one line on what"
Enter
Type "it says.' --mcp-config mcp.json --strict-mcp-config --allowed-tools mcp__spider__spider_scrape --model sonnet"
Sleep 300ms
Enter
Sleep 90s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
