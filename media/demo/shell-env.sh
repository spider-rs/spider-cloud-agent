# Keep VHS independent of the user's prompt and startup customizations.
export PS1='$ '
export PROMPT_COMMAND=''
export PATH="$PWD/target/release:/opt/homebrew/bin:$PATH"
export SPIDER_AGENT_NO_UPDATE=1 NO_COLOR=1
export HISTFILE=/dev/null
