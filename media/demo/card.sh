#!/usr/bin/env bash
# Prints one caption in the middle of an empty screen and holds it there, so
# cut.py can lift a title card off a vhs frame. The ffmpeg this repository
# records with has no text filter, and vhs already draws Menlo on the scene
# background, so a card comes out of the same terminal as the scenes.
set -euo pipefail
text="$1"
rows="$(tput lines)"
cols="$(tput cols)"
tput civis
clear
for _ in $(seq 1 $(((rows - 1) / 2))); do printf '\n'; done
printf '%*s%s' $(((cols - ${#text}) / 2)) '' "$text"
sleep 30
