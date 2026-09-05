# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_agentix_global_optspecs
    string join \n c/config= h/help V/version
end

function __fish_agentix_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_agentix_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_agentix_using_subcommand
    set -l cmd (__fish_agentix_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c agentix -n "__fish_agentix_needs_command" -s c -l config -r -F
complete -c agentix -n "__fish_agentix_needs_command" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_needs_command" -s V -l version -d 'Print version'
complete -c agentix -n "__fish_agentix_needs_command" -f -a "serve" -d 'Run the Agentix bridge until interrupted'
complete -c agentix -n "__fish_agentix_needs_command" -f -a "doctor" -d 'Validate configuration, credentials, and the selected agent transport'
complete -c agentix -n "__fish_agentix_needs_command" -f -a "client" -d 'Use the running Agentix server for local diagnostics and setup'
complete -c agentix -n "__fish_agentix_needs_command" -f -a "completions" -d 'Print a shell completion script to stdout'
complete -c agentix -n "__fish_agentix_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c agentix -n "__fish_agentix_using_subcommand serve" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand client; and not __fish_seen_subcommand_from claim sessions call help" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand client; and not __fish_seen_subcommand_from claim sessions call help" -f -a "claim" -d 'Generate a temporary in-memory owner claim code for the selected IM channel'
complete -c agentix -n "__fish_agentix_using_subcommand client; and not __fish_seen_subcommand_from claim sessions call help" -f -a "sessions" -d 'List sessions available through the running Agentix server'
complete -c agentix -n "__fish_agentix_using_subcommand client; and not __fish_seen_subcommand_from claim sessions call help" -f -a "call" -d 'Ask the server to send a raw JSON RPC request to Codex'
complete -c agentix -n "__fish_agentix_using_subcommand client; and not __fish_seen_subcommand_from claim sessions call help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from claim" -l ttl-minutes -r
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from claim" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from sessions" -l cursor -r
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from sessions" -l limit -r
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from sessions" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from call" -l params -r
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from call" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from help" -f -a "claim" -d 'Generate a temporary in-memory owner claim code for the selected IM channel'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from help" -f -a "sessions" -d 'List sessions available through the running Agentix server'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from help" -f -a "call" -d 'Ask the server to send a raw JSON RPC request to Codex'
complete -c agentix -n "__fish_agentix_using_subcommand client; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c agentix -n "__fish_agentix_using_subcommand completions" -s h -l help -d 'Print help'
complete -c agentix -n "__fish_agentix_using_subcommand help; and not __fish_seen_subcommand_from serve doctor client completions help" -f -a "serve" -d 'Run the Agentix bridge until interrupted'
complete -c agentix -n "__fish_agentix_using_subcommand help; and not __fish_seen_subcommand_from serve doctor client completions help" -f -a "doctor" -d 'Validate configuration, credentials, and the selected agent transport'
complete -c agentix -n "__fish_agentix_using_subcommand help; and not __fish_seen_subcommand_from serve doctor client completions help" -f -a "client" -d 'Use the running Agentix server for local diagnostics and setup'
complete -c agentix -n "__fish_agentix_using_subcommand help; and not __fish_seen_subcommand_from serve doctor client completions help" -f -a "completions" -d 'Print a shell completion script to stdout'
complete -c agentix -n "__fish_agentix_using_subcommand help; and not __fish_seen_subcommand_from serve doctor client completions help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c agentix -n "__fish_agentix_using_subcommand help; and __fish_seen_subcommand_from client" -f -a "claim" -d 'Generate a temporary in-memory owner claim code for the selected IM channel'
complete -c agentix -n "__fish_agentix_using_subcommand help; and __fish_seen_subcommand_from client" -f -a "sessions" -d 'List sessions available through the running Agentix server'
complete -c agentix -n "__fish_agentix_using_subcommand help; and __fish_seen_subcommand_from client" -f -a "call" -d 'Ask the server to send a raw JSON RPC request to Codex'
