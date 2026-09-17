Output media/out/frames/codex-calls-agent/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1900
Set Height 340
Set Padding 32
Set Framerate 24
Set TypingSpeed 10ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/agent-scene.sh open codex-calls-agent"
Enter
Sleep 3s
Show

Type "codex exec -s workspace-write -c sandbox_workspace_write.network_access=true --skip-git-repo-check --json '"
Enter
Type "spider-agent scrape https://example.com --selectors fields.json --json --budget 2 --wall 20 --no-router."
Enter
Type "Print 2 lines, no markdown: the command, then served/refused/cost_credits/elapsed_ms JSON.' | jq -rs 'map(.item.text? // empty)[-1]'"
Sleep 300ms
Enter
Sleep 40s

Hide
Type@1ms "scene_close"
Enter
Sleep 3s
