Output media/out/frames/setup/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1500
Set Height 480
Set Padding 32
Set Framerate 24
Set TypingSpeed 60ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/entry-scene.sh open setup"
Enter
Sleep 3s
Show

Type "spider-agent setup --dry-run"
Sleep 300ms
Enter
Sleep 8s
Type "spider-agent setup"
Sleep 300ms
Enter
Sleep 15s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
