Output media/out/frames/store-to-rows/

Set Shell "bash"
Set FontSize 20
Set FontFamily "Menlo"
Set Width 1560
Set Height 480
Set Padding 32
Set Framerate 24
Set TypingSpeed 40ms
Set Theme { "background": "#10151e", "foreground": "#e0e6ef", "cursorColor": "#e0e6ef", "black": "#10151e", "white": "#e0e6ef" }

Hide
Type@1ms "source media/demo/entry-scene.sh open store-to-rows"
Enter
Sleep 3s
Show

Type "cat fields.json"
Sleep 300ms
Enter
Sleep 1500ms
Type "spider-agent crawl https://books.toscrape.com --selectors fields.json --budget 1500 --ndjson -q > books.ndjson"
Sleep 300ms
Enter
Sleep 45s
Type `jq -r 'select(.fields.title[0]) | .fields | "\(.price[0])  \(.title[0])"' books.ndjson | awk 'NR<6'`
Sleep 300ms
Enter
Sleep 3s
Type "jq -c 'select(.fields.title[0])' books.ndjson | wc -l"
Sleep 300ms
Enter
Sleep 3s
Type "tail -n 1 books.ndjson | jq -c '{served, refused, elapsed_ms}'"
Sleep 300ms
Enter
Sleep 4s

Hide
# vhs has one captured frame in flight when Hide lands, and the teardown starts
# typing a millisecond later. Wait it out before touching the keyboard.
Sleep 1s
Type@1ms "scene_close"
Enter
Sleep 3s
