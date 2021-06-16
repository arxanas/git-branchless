#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/demo_helpers.sh

parse_args "$@"
set_up_git_repo
echo 'Hello, world!' >foo
git add foo
git commit -m 'Commit foo'
git checkout -b 'conflict' HEAD~
echo 'Goodbye, world!' >foo
git add foo
git commit -m 'Also commit foo'

run_demo '
spawn asciinema rec
expect_prompt

run_command "cat foo"
expect_prompt

run_command "git show master:foo"
expect_prompt

run_command "git rebase master"

run_command "vim foo"
sleep 1
send_keystroke_to_interactive_process "V"
send_keystroke_to_interactive_process "G"
send_keystroke_to_interactive_process "C"
send -h "Bad merge conflict resolution"
sleep 1
send -h \x03
sleep 1
send -h ":wq\r"
sleep 1
expect_prompt

run_command "git add foo"
run_command "git rebase --continue"
send_keystroke_to_interactive_process "C"
send -h "Bad merge"
sleep 1
send -h \x03
sleep 1
send -h ":wq\r"
sleep 1
expect_prompt

run_command "cat foo"
run_command "echo oh no"

run_command "git undo"
expect -timeout 3
send_keystroke_to_interactive_process "p" 2
send_keystroke_to_interactive_process "\r" 1
expect "Confirm?"
run_command "y"

run_command "cat foo"
run_command "echo crisis averted"

quit_and_dump_asciicast_path
'
