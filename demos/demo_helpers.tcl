set send_slow {1 0.2}
set send_human {0.1 0.3 1 0.05 1}
set timeout 1
set CTRLC \003
set ESC \033

proc expect_prompt {} {
    expect "$ "
}

proc run_command {cmd} {
    send -h "$cmd"
    sleep 3
    send "\r"
    expect -timeout 1
}

proc send_keystroke_to_interactive_process {key {addl_sleep 2}} {
    send "$key"
    expect -timeout 1
    sleep $addl_sleep
}

proc quit_and_dump_asciicast_path {} {
    set CTRLC \003
    set ESC \033

    send "exit\r"
    expect "asciinema: recording finished"
    sleep 1
    send $CTRLC
    expect -re "asciicast saved to (.+)$ESC.*\r" {
        send_user "$expect_out(1,string)\n"
    }
}
