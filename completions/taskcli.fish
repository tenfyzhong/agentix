# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_taskcli_global_optspecs
    string join \n config= json project= actor= executor= session= delegated-by= lease-token= expect-revision= idempotency-key= h/help V/version
end

function __fish_taskcli_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_taskcli_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_taskcli_using_subcommand
    set -l cmd (__fish_taskcli_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c taskcli -n "__fish_taskcli_needs_command" -l config -r -F
complete -c taskcli -n "__fish_taskcli_needs_command" -l project -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l actor -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l executor -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l session -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_needs_command" -l json
complete -c taskcli -n "__fish_taskcli_needs_command" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_needs_command" -s V -l version -d 'Print version'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "completions" -d 'Print a shell completion script without loading task configuration'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "init" -d 'Create configuration, initialize `SQLite` storage, and generate task documents'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "obsidian" -d 'Install and configure Obsidian task views in the configured vault'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "doctor" -d 'Check for missing Plan files and documents that are behind the event log'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "sync" -d 'Regenerate task documents from the current database state'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "project" -d 'Register, inspect, archive, or delete Projects shared across worktrees'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "job" -d 'Manage Jobs that group Tasks for an independently deliverable requirement'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "task" -d 'Manage Task dependencies, ownership leases, and execution status'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "plan" -d 'Publish, revise, or inspect the execution Plan in a Task note'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "event" -d 'Inspect the ordered audit log of task coordination events'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "context" -d 'Show Task, Job, lease, Plan, and document context for a session or explicit IDs'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "hook" -d 'Handle agent session lifecycle events and maintain Task leases'
complete -c taskcli -n "__fish_taskcli_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand completions" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l format -r -f -a "obsidian\t''
markdown\t''"
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l root -r -f -a "(__fish_complete_directories)"
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l directory -r -f -a "(__fish_complete_directories)"
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l database -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand init" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -f -a "setup" -d 'Install `TaskNotes` and configure its task statuses and Bases. Close Obsidian first'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and not __fish_seen_subcommand_from setup help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l plugin-dir -d 'Use a local `TaskNotes` release directory instead of downloading the bundled version' -r -f -a "(__fish_complete_directories)"
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from setup" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from help" -f -a "setup" -d 'Install `TaskNotes` and configure its task statuses and Bases. Close Obsidian first'
complete -c taskcli -n "__fish_taskcli_using_subcommand obsidian; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand sync" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "delete" -d 'Delete the Project, its work, and its entire generated document directory'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "register" -d 'Register a Project using its root directory and Git identity when available'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "list" -d 'List unarchived Projects, or archived Projects with --archived'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "show" -d 'Show a Project\'s identity, root directory, and archival state'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "archive" -d 'Archive a Project after all of its Jobs are closed'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "unarchive" -d 'Restore an archived Project to the active project views'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and not __fish_seen_subcommand_from delete register list show archive unarchive help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from delete" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l name -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l root -r -f -a "(__fish_complete_directories)"
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from register" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l archived
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from archive" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from unarchive" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "delete" -d 'Delete the Project, its work, and its entire generated document directory'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "register" -d 'Register a Project using its root directory and Git identity when available'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "list" -d 'List unarchived Projects, or archived Projects with --archived'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show a Project\'s identity, root directory, and archival state'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "archive" -d 'Archive a Project after all of its Jobs are closed'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "unarchive" -d 'Restore an archived Project to the active project views'
complete -c taskcli -n "__fish_taskcli_using_subcommand project; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "delete" -d 'Delete the Job, its Tasks, and their Plan documents'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "create" -d 'Create a Job with a title and acceptance goal in the selected Project'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "update" -d 'Change a Job\'s display name, title, acceptance goal, or original prompt'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "list" -d 'List Jobs, optionally filtered by Project, status, or date'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "show" -d 'Show a Job\'s goal, status, and lifecycle metadata'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "cancel" -d 'Cancel a Job and its unfinished Tasks after their leases have been released'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "archive" -d 'Archive a closed Job and move its document to the archive'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "unarchive" -d 'Restore an archived Job document without changing its completion status'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and not __fish_seen_subcommand_from delete create update list show cancel archive unarchive help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from delete" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l name -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l title -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l goal -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l prompt -d 'Original user prompt, preserved verbatim in the Job document' -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from create" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l name -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l title -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l goal -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l prompt -d 'Replace the original user prompt; an empty string clears it' -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from update" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l period -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l created-from -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l created-to -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l active
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l completed
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l archived
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from cancel" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from archive" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from unarchive" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "delete" -d 'Delete the Job, its Tasks, and their Plan documents'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "create" -d 'Create a Job with a title and acceptance goal in the selected Project'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "update" -d 'Change a Job\'s display name, title, acceptance goal, or original prompt'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "list" -d 'List Jobs, optionally filtered by Project, status, or date'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show a Job\'s goal, status, and lifecycle metadata'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "cancel" -d 'Cancel a Job and its unfinished Tasks after their leases have been released'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "archive" -d 'Archive a closed Job and move its document to the archive'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "unarchive" -d 'Restore an archived Job document without changing its completion status'
complete -c taskcli -n "__fish_taskcli_using_subcommand job; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "add" -d 'Add a Task to a Job and create its note without publishing a Plan'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "update" -d 'Change a Task\'s display name, title, or board position'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "list" -d 'List Tasks, optionally filtered by Job, Project, status, or readiness'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "show" -d 'Show a Task\'s status, dependencies, Plan reference, and current lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "depend" -d 'Add a prerequisite Task in the same Project before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "undepend" -d 'Remove a prerequisite Task before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "claim" -d 'Acquire a planning lease using --executor and --session'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "start" -d 'Begin execution with the current lease, a published Plan, and DONE prerequisites'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "heartbeat" -d 'Renew a Task lease using its owning session and lease token'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "release" -d 'Release ownership and mark the Task BLOCKED with a handoff reason'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "block" -d 'Mark a Task BLOCKED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "wait" -d 'Mark a Task `WAITING_USER` with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "fail" -d 'Mark a Task FAILED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "done" -d 'Mark an EXECUTING Task DONE and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "cancel" -d 'Cancel a Task and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "retry" -d 'Return a FAILED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "reopen" -d 'Return a DONE or CANCELLED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and not __fish_seen_subcommand_from add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l name -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l job -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l title -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from add" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l name -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l title -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l position -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from update" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l job -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l status -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l ready
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from depend" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from undepend" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from claim" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from start" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from heartbeat" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l reason -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from release" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l reason -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from block" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l reason -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from wait" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l reason -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from fail" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from done" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from cancel" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from retry" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from reopen" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "add" -d 'Add a Task to a Job and create its note without publishing a Plan'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "update" -d 'Change a Task\'s display name, title, or board position'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "list" -d 'List Tasks, optionally filtered by Job, Project, status, or readiness'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show a Task\'s status, dependencies, Plan reference, and current lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "depend" -d 'Add a prerequisite Task in the same Project before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "undepend" -d 'Remove a prerequisite Task before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "claim" -d 'Acquire a planning lease using --executor and --session'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "start" -d 'Begin execution with the current lease, a published Plan, and DONE prerequisites'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "heartbeat" -d 'Renew a Task lease using its owning session and lease token'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "release" -d 'Release ownership and mark the Task BLOCKED with a handoff reason'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "block" -d 'Mark a Task BLOCKED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "wait" -d 'Mark a Task `WAITING_USER` with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "fail" -d 'Mark a Task FAILED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "done" -d 'Mark an EXECUTING Task DONE and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "cancel" -d 'Cancel a Task and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "retry" -d 'Return a FAILED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "reopen" -d 'Return a DONE or CANCELLED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand task; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -f -a "create" -d 'Publish a Plan from --body or --file while holding the Task lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -f -a "revise" -d 'Replace the Plan body in the same Task note while holding its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -f -a "show" -d 'Show the current Plan\'s metadata and absolute file path for a Task'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and not __fish_seen_subcommand_from create revise show help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l body -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l file -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from create" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l body -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l file -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from revise" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from help" -f -a "create" -d 'Publish a Plan from --body or --file while holding the Task lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from help" -f -a "revise" -d 'Replace the Plan body in the same Task note while holding its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show the current Plan\'s metadata and absolute file path for a Task'
complete -c taskcli -n "__fish_taskcli_using_subcommand plan; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -f -a "list" -d 'List events after a sequence cursor, optionally filtered by Job and limited in count'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and not __fish_seen_subcommand_from list help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l job -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l after -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l limit -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from help" -f -a "list" -d 'List events after a sequence cursor, optionally filtered by Job and limited in count'
complete -c taskcli -n "__fish_taskcli_using_subcommand event; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l task -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l job -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand context" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -f -a "session-start" -d 'Recover the session\'s Tasks blocked by interruption or lease expiry into planning'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -f -a "session-end" -d 'Record session shutdown and release its active Task leases'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -f -a "interrupt" -d 'Release an interrupted session\'s Task leases while preserving its Plans'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -f -a "heartbeat" -d 'Renew all active Task leases owned by the session'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and not __fish_seen_subcommand_from session-start session-end interrupt heartbeat help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-start" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from session-end" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from interrupt" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l config -r -F
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l project -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l actor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l executor -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l session -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l delegated-by -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l lease-token -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l expect-revision -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l idempotency-key -r
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -l json
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from heartbeat" -s h -l help -d 'Print help'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "session-start" -d 'Recover the session\'s Tasks blocked by interruption or lease expiry into planning'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "session-end" -d 'Record session shutdown and release its active Task leases'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "interrupt" -d 'Release an interrupted session\'s Task leases while preserving its Plans'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "heartbeat" -d 'Renew all active Task leases owned by the session'
complete -c taskcli -n "__fish_taskcli_using_subcommand hook; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "completions" -d 'Print a shell completion script without loading task configuration'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "init" -d 'Create configuration, initialize `SQLite` storage, and generate task documents'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "obsidian" -d 'Install and configure Obsidian task views in the configured vault'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "doctor" -d 'Check for missing Plan files and documents that are behind the event log'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "sync" -d 'Regenerate task documents from the current database state'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "project" -d 'Register, inspect, archive, or delete Projects shared across worktrees'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "job" -d 'Manage Jobs that group Tasks for an independently deliverable requirement'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "task" -d 'Manage Task dependencies, ownership leases, and execution status'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "plan" -d 'Publish, revise, or inspect the execution Plan in a Task note'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "event" -d 'Inspect the ordered audit log of task coordination events'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "context" -d 'Show Task, Job, lease, Plan, and document context for a session or explicit IDs'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "hook" -d 'Handle agent session lifecycle events and maintain Task leases'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and not __fish_seen_subcommand_from completions init obsidian doctor sync project job task plan event context hook help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from obsidian" -f -a "setup" -d 'Install `TaskNotes` and configure its task statuses and Bases. Close Obsidian first'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "delete" -d 'Delete the Project, its work, and its entire generated document directory'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "register" -d 'Register a Project using its root directory and Git identity when available'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "list" -d 'List unarchived Projects, or archived Projects with --archived'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "show" -d 'Show a Project\'s identity, root directory, and archival state'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "archive" -d 'Archive a Project after all of its Jobs are closed'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from project" -f -a "unarchive" -d 'Restore an archived Project to the active project views'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "delete" -d 'Delete the Job, its Tasks, and their Plan documents'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "create" -d 'Create a Job with a title and acceptance goal in the selected Project'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "update" -d 'Change a Job\'s display name, title, acceptance goal, or original prompt'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "list" -d 'List Jobs, optionally filtered by Project, status, or date'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "show" -d 'Show a Job\'s goal, status, and lifecycle metadata'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "cancel" -d 'Cancel a Job and its unfinished Tasks after their leases have been released'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "archive" -d 'Archive a closed Job and move its document to the archive'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from job" -f -a "unarchive" -d 'Restore an archived Job document without changing its completion status'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "add" -d 'Add a Task to a Job and create its note without publishing a Plan'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "update" -d 'Change a Task\'s display name, title, or board position'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "list" -d 'List Tasks, optionally filtered by Job, Project, status, or readiness'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "show" -d 'Show a Task\'s status, dependencies, Plan reference, and current lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "depend" -d 'Add a prerequisite Task in the same Project before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "undepend" -d 'Remove a prerequisite Task before execution has started'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "claim" -d 'Acquire a planning lease using --executor and --session'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "start" -d 'Begin execution with the current lease, a published Plan, and DONE prerequisites'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "heartbeat" -d 'Renew a Task lease using its owning session and lease token'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "release" -d 'Release ownership and mark the Task BLOCKED with a handoff reason'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "block" -d 'Mark a Task BLOCKED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "wait" -d 'Mark a Task `WAITING_USER` with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "fail" -d 'Mark a Task FAILED with a reason and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "done" -d 'Mark an EXECUTING Task DONE and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "cancel" -d 'Cancel a Task and release its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "retry" -d 'Return a FAILED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from task" -f -a "reopen" -d 'Return a DONE or CANCELLED Task to TODO so it can be claimed again'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from plan" -f -a "create" -d 'Publish a Plan from --body or --file while holding the Task lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from plan" -f -a "revise" -d 'Replace the Plan body in the same Task note while holding its lease'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from plan" -f -a "show" -d 'Show the current Plan\'s metadata and absolute file path for a Task'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from event" -f -a "list" -d 'List events after a sequence cursor, optionally filtered by Job and limited in count'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "session-start" -d 'Recover the session\'s Tasks blocked by interruption or lease expiry into planning'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "session-end" -d 'Record session shutdown and release its active Task leases'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "interrupt" -d 'Release an interrupted session\'s Task leases while preserving its Plans'
complete -c taskcli -n "__fish_taskcli_using_subcommand help; and __fish_seen_subcommand_from hook" -f -a "heartbeat" -d 'Renew all active Task leases owned by the session'
