_taskix() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="taskix"
                ;;
            taskix,completions)
                cmd="taskix__subcmd__completions"
                ;;
            taskix,context)
                cmd="taskix__subcmd__context"
                ;;
            taskix,doctor)
                cmd="taskix__subcmd__doctor"
                ;;
            taskix,event)
                cmd="taskix__subcmd__event"
                ;;
            taskix,help)
                cmd="taskix__subcmd__help"
                ;;
            taskix,hook)
                cmd="taskix__subcmd__hook"
                ;;
            taskix,inbox)
                cmd="taskix__subcmd__inbox"
                ;;
            taskix,init)
                cmd="taskix__subcmd__init"
                ;;
            taskix,job)
                cmd="taskix__subcmd__job"
                ;;
            taskix,obsidian)
                cmd="taskix__subcmd__obsidian"
                ;;
            taskix,plan)
                cmd="taskix__subcmd__plan"
                ;;
            taskix,project)
                cmd="taskix__subcmd__project"
                ;;
            taskix,sync)
                cmd="taskix__subcmd__sync"
                ;;
            taskix,task)
                cmd="taskix__subcmd__task"
                ;;
            taskix__subcmd__event,help)
                cmd="taskix__subcmd__event__subcmd__help"
                ;;
            taskix__subcmd__event,list)
                cmd="taskix__subcmd__event__subcmd__list"
                ;;
            taskix__subcmd__event__subcmd__help,help)
                cmd="taskix__subcmd__event__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__event__subcmd__help,list)
                cmd="taskix__subcmd__event__subcmd__help__subcmd__list"
                ;;
            taskix__subcmd__help,completions)
                cmd="taskix__subcmd__help__subcmd__completions"
                ;;
            taskix__subcmd__help,context)
                cmd="taskix__subcmd__help__subcmd__context"
                ;;
            taskix__subcmd__help,doctor)
                cmd="taskix__subcmd__help__subcmd__doctor"
                ;;
            taskix__subcmd__help,event)
                cmd="taskix__subcmd__help__subcmd__event"
                ;;
            taskix__subcmd__help,help)
                cmd="taskix__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__help,hook)
                cmd="taskix__subcmd__help__subcmd__hook"
                ;;
            taskix__subcmd__help,inbox)
                cmd="taskix__subcmd__help__subcmd__inbox"
                ;;
            taskix__subcmd__help,init)
                cmd="taskix__subcmd__help__subcmd__init"
                ;;
            taskix__subcmd__help,job)
                cmd="taskix__subcmd__help__subcmd__job"
                ;;
            taskix__subcmd__help,obsidian)
                cmd="taskix__subcmd__help__subcmd__obsidian"
                ;;
            taskix__subcmd__help,plan)
                cmd="taskix__subcmd__help__subcmd__plan"
                ;;
            taskix__subcmd__help,project)
                cmd="taskix__subcmd__help__subcmd__project"
                ;;
            taskix__subcmd__help,sync)
                cmd="taskix__subcmd__help__subcmd__sync"
                ;;
            taskix__subcmd__help,task)
                cmd="taskix__subcmd__help__subcmd__task"
                ;;
            taskix__subcmd__help__subcmd__event,list)
                cmd="taskix__subcmd__help__subcmd__event__subcmd__list"
                ;;
            taskix__subcmd__help__subcmd__hook,heartbeat)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__heartbeat"
                ;;
            taskix__subcmd__help__subcmd__hook,interrupt)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__interrupt"
                ;;
            taskix__subcmd__help__subcmd__hook,record)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__record"
                ;;
            taskix__subcmd__help__subcmd__hook,session-end)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__session__subcmd__end"
                ;;
            taskix__subcmd__help__subcmd__hook,session-start)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__session__subcmd__start"
                ;;
            taskix__subcmd__help__subcmd__hook,stop)
                cmd="taskix__subcmd__help__subcmd__hook__subcmd__stop"
                ;;
            taskix__subcmd__help__subcmd__inbox,add)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__add"
                ;;
            taskix__subcmd__help__subcmd__inbox,cancel)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__cancel"
                ;;
            taskix__subcmd__help__subcmd__inbox,claim-next)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__claim__subcmd__next"
                ;;
            taskix__subcmd__help__subcmd__inbox,list)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__list"
                ;;
            taskix__subcmd__help__subcmd__inbox,release)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__release"
                ;;
            taskix__subcmd__help__subcmd__inbox,set-status)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__set__subcmd__status"
                ;;
            taskix__subcmd__help__subcmd__inbox,sync)
                cmd="taskix__subcmd__help__subcmd__inbox__subcmd__sync"
                ;;
            taskix__subcmd__help__subcmd__job,approve)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__approve"
                ;;
            taskix__subcmd__help__subcmd__job,archive)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__archive"
                ;;
            taskix__subcmd__help__subcmd__job,cancel)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__cancel"
                ;;
            taskix__subcmd__help__subcmd__job,create)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__create"
                ;;
            taskix__subcmd__help__subcmd__job,delete)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__delete"
                ;;
            taskix__subcmd__help__subcmd__job,followup)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__followup"
                ;;
            taskix__subcmd__help__subcmd__job,list)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__list"
                ;;
            taskix__subcmd__help__subcmd__job,reject)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__reject"
                ;;
            taskix__subcmd__help__subcmd__job,show)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__show"
                ;;
            taskix__subcmd__help__subcmd__job,submit)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__submit"
                ;;
            taskix__subcmd__help__subcmd__job,unarchive)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__unarchive"
                ;;
            taskix__subcmd__help__subcmd__job,update)
                cmd="taskix__subcmd__help__subcmd__job__subcmd__update"
                ;;
            taskix__subcmd__help__subcmd__obsidian,connection)
                cmd="taskix__subcmd__help__subcmd__obsidian__subcmd__connection"
                ;;
            taskix__subcmd__help__subcmd__obsidian,setup)
                cmd="taskix__subcmd__help__subcmd__obsidian__subcmd__setup"
                ;;
            taskix__subcmd__help__subcmd__obsidian,show)
                cmd="taskix__subcmd__help__subcmd__obsidian__subcmd__show"
                ;;
            taskix__subcmd__help__subcmd__obsidian,snapshot)
                cmd="taskix__subcmd__help__subcmd__obsidian__subcmd__snapshot"
                ;;
            taskix__subcmd__help__subcmd__plan,create)
                cmd="taskix__subcmd__help__subcmd__plan__subcmd__create"
                ;;
            taskix__subcmd__help__subcmd__plan,revise)
                cmd="taskix__subcmd__help__subcmd__plan__subcmd__revise"
                ;;
            taskix__subcmd__help__subcmd__plan,show)
                cmd="taskix__subcmd__help__subcmd__plan__subcmd__show"
                ;;
            taskix__subcmd__help__subcmd__project,archive)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__archive"
                ;;
            taskix__subcmd__help__subcmd__project,delete)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__delete"
                ;;
            taskix__subcmd__help__subcmd__project,list)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__list"
                ;;
            taskix__subcmd__help__subcmd__project,register)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__register"
                ;;
            taskix__subcmd__help__subcmd__project,show)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__show"
                ;;
            taskix__subcmd__help__subcmd__project,unarchive)
                cmd="taskix__subcmd__help__subcmd__project__subcmd__unarchive"
                ;;
            taskix__subcmd__help__subcmd__task,add)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__add"
                ;;
            taskix__subcmd__help__subcmd__task,block)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__block"
                ;;
            taskix__subcmd__help__subcmd__task,cancel)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__cancel"
                ;;
            taskix__subcmd__help__subcmd__task,claim)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__claim"
                ;;
            taskix__subcmd__help__subcmd__task,depend)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__depend"
                ;;
            taskix__subcmd__help__subcmd__task,done)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__done"
                ;;
            taskix__subcmd__help__subcmd__task,fail)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__fail"
                ;;
            taskix__subcmd__help__subcmd__task,heartbeat)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__heartbeat"
                ;;
            taskix__subcmd__help__subcmd__task,list)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__list"
                ;;
            taskix__subcmd__help__subcmd__task,release)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__release"
                ;;
            taskix__subcmd__help__subcmd__task,reopen)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__reopen"
                ;;
            taskix__subcmd__help__subcmd__task,retry)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__retry"
                ;;
            taskix__subcmd__help__subcmd__task,show)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__show"
                ;;
            taskix__subcmd__help__subcmd__task,start)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__start"
                ;;
            taskix__subcmd__help__subcmd__task,undepend)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__undepend"
                ;;
            taskix__subcmd__help__subcmd__task,update)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__update"
                ;;
            taskix__subcmd__help__subcmd__task,wait)
                cmd="taskix__subcmd__help__subcmd__task__subcmd__wait"
                ;;
            taskix__subcmd__hook,heartbeat)
                cmd="taskix__subcmd__hook__subcmd__heartbeat"
                ;;
            taskix__subcmd__hook,help)
                cmd="taskix__subcmd__hook__subcmd__help"
                ;;
            taskix__subcmd__hook,interrupt)
                cmd="taskix__subcmd__hook__subcmd__interrupt"
                ;;
            taskix__subcmd__hook,record)
                cmd="taskix__subcmd__hook__subcmd__record"
                ;;
            taskix__subcmd__hook,session-end)
                cmd="taskix__subcmd__hook__subcmd__session__subcmd__end"
                ;;
            taskix__subcmd__hook,session-start)
                cmd="taskix__subcmd__hook__subcmd__session__subcmd__start"
                ;;
            taskix__subcmd__hook,stop)
                cmd="taskix__subcmd__hook__subcmd__stop"
                ;;
            taskix__subcmd__hook__subcmd__help,heartbeat)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__heartbeat"
                ;;
            taskix__subcmd__hook__subcmd__help,help)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__hook__subcmd__help,interrupt)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__interrupt"
                ;;
            taskix__subcmd__hook__subcmd__help,record)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__record"
                ;;
            taskix__subcmd__hook__subcmd__help,session-end)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__session__subcmd__end"
                ;;
            taskix__subcmd__hook__subcmd__help,session-start)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__session__subcmd__start"
                ;;
            taskix__subcmd__hook__subcmd__help,stop)
                cmd="taskix__subcmd__hook__subcmd__help__subcmd__stop"
                ;;
            taskix__subcmd__inbox,add)
                cmd="taskix__subcmd__inbox__subcmd__add"
                ;;
            taskix__subcmd__inbox,cancel)
                cmd="taskix__subcmd__inbox__subcmd__cancel"
                ;;
            taskix__subcmd__inbox,claim-next)
                cmd="taskix__subcmd__inbox__subcmd__claim__subcmd__next"
                ;;
            taskix__subcmd__inbox,help)
                cmd="taskix__subcmd__inbox__subcmd__help"
                ;;
            taskix__subcmd__inbox,list)
                cmd="taskix__subcmd__inbox__subcmd__list"
                ;;
            taskix__subcmd__inbox,release)
                cmd="taskix__subcmd__inbox__subcmd__release"
                ;;
            taskix__subcmd__inbox,set-status)
                cmd="taskix__subcmd__inbox__subcmd__set__subcmd__status"
                ;;
            taskix__subcmd__inbox,sync)
                cmd="taskix__subcmd__inbox__subcmd__sync"
                ;;
            taskix__subcmd__inbox__subcmd__help,add)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__add"
                ;;
            taskix__subcmd__inbox__subcmd__help,cancel)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__cancel"
                ;;
            taskix__subcmd__inbox__subcmd__help,claim-next)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__claim__subcmd__next"
                ;;
            taskix__subcmd__inbox__subcmd__help,help)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__inbox__subcmd__help,list)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__list"
                ;;
            taskix__subcmd__inbox__subcmd__help,release)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__release"
                ;;
            taskix__subcmd__inbox__subcmd__help,set-status)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__set__subcmd__status"
                ;;
            taskix__subcmd__inbox__subcmd__help,sync)
                cmd="taskix__subcmd__inbox__subcmd__help__subcmd__sync"
                ;;
            taskix__subcmd__job,approve)
                cmd="taskix__subcmd__job__subcmd__approve"
                ;;
            taskix__subcmd__job,archive)
                cmd="taskix__subcmd__job__subcmd__archive"
                ;;
            taskix__subcmd__job,cancel)
                cmd="taskix__subcmd__job__subcmd__cancel"
                ;;
            taskix__subcmd__job,create)
                cmd="taskix__subcmd__job__subcmd__create"
                ;;
            taskix__subcmd__job,delete)
                cmd="taskix__subcmd__job__subcmd__delete"
                ;;
            taskix__subcmd__job,followup)
                cmd="taskix__subcmd__job__subcmd__followup"
                ;;
            taskix__subcmd__job,help)
                cmd="taskix__subcmd__job__subcmd__help"
                ;;
            taskix__subcmd__job,list)
                cmd="taskix__subcmd__job__subcmd__list"
                ;;
            taskix__subcmd__job,reject)
                cmd="taskix__subcmd__job__subcmd__reject"
                ;;
            taskix__subcmd__job,show)
                cmd="taskix__subcmd__job__subcmd__show"
                ;;
            taskix__subcmd__job,submit)
                cmd="taskix__subcmd__job__subcmd__submit"
                ;;
            taskix__subcmd__job,unarchive)
                cmd="taskix__subcmd__job__subcmd__unarchive"
                ;;
            taskix__subcmd__job,update)
                cmd="taskix__subcmd__job__subcmd__update"
                ;;
            taskix__subcmd__job__subcmd__help,approve)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__approve"
                ;;
            taskix__subcmd__job__subcmd__help,archive)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__archive"
                ;;
            taskix__subcmd__job__subcmd__help,cancel)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__cancel"
                ;;
            taskix__subcmd__job__subcmd__help,create)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__create"
                ;;
            taskix__subcmd__job__subcmd__help,delete)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__delete"
                ;;
            taskix__subcmd__job__subcmd__help,followup)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__followup"
                ;;
            taskix__subcmd__job__subcmd__help,help)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__job__subcmd__help,list)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__list"
                ;;
            taskix__subcmd__job__subcmd__help,reject)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__reject"
                ;;
            taskix__subcmd__job__subcmd__help,show)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__show"
                ;;
            taskix__subcmd__job__subcmd__help,submit)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__submit"
                ;;
            taskix__subcmd__job__subcmd__help,unarchive)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__unarchive"
                ;;
            taskix__subcmd__job__subcmd__help,update)
                cmd="taskix__subcmd__job__subcmd__help__subcmd__update"
                ;;
            taskix__subcmd__obsidian,connection)
                cmd="taskix__subcmd__obsidian__subcmd__connection"
                ;;
            taskix__subcmd__obsidian,help)
                cmd="taskix__subcmd__obsidian__subcmd__help"
                ;;
            taskix__subcmd__obsidian,setup)
                cmd="taskix__subcmd__obsidian__subcmd__setup"
                ;;
            taskix__subcmd__obsidian,show)
                cmd="taskix__subcmd__obsidian__subcmd__show"
                ;;
            taskix__subcmd__obsidian,snapshot)
                cmd="taskix__subcmd__obsidian__subcmd__snapshot"
                ;;
            taskix__subcmd__obsidian__subcmd__help,connection)
                cmd="taskix__subcmd__obsidian__subcmd__help__subcmd__connection"
                ;;
            taskix__subcmd__obsidian__subcmd__help,help)
                cmd="taskix__subcmd__obsidian__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__obsidian__subcmd__help,setup)
                cmd="taskix__subcmd__obsidian__subcmd__help__subcmd__setup"
                ;;
            taskix__subcmd__obsidian__subcmd__help,show)
                cmd="taskix__subcmd__obsidian__subcmd__help__subcmd__show"
                ;;
            taskix__subcmd__obsidian__subcmd__help,snapshot)
                cmd="taskix__subcmd__obsidian__subcmd__help__subcmd__snapshot"
                ;;
            taskix__subcmd__plan,create)
                cmd="taskix__subcmd__plan__subcmd__create"
                ;;
            taskix__subcmd__plan,help)
                cmd="taskix__subcmd__plan__subcmd__help"
                ;;
            taskix__subcmd__plan,revise)
                cmd="taskix__subcmd__plan__subcmd__revise"
                ;;
            taskix__subcmd__plan,show)
                cmd="taskix__subcmd__plan__subcmd__show"
                ;;
            taskix__subcmd__plan__subcmd__help,create)
                cmd="taskix__subcmd__plan__subcmd__help__subcmd__create"
                ;;
            taskix__subcmd__plan__subcmd__help,help)
                cmd="taskix__subcmd__plan__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__plan__subcmd__help,revise)
                cmd="taskix__subcmd__plan__subcmd__help__subcmd__revise"
                ;;
            taskix__subcmd__plan__subcmd__help,show)
                cmd="taskix__subcmd__plan__subcmd__help__subcmd__show"
                ;;
            taskix__subcmd__project,archive)
                cmd="taskix__subcmd__project__subcmd__archive"
                ;;
            taskix__subcmd__project,delete)
                cmd="taskix__subcmd__project__subcmd__delete"
                ;;
            taskix__subcmd__project,help)
                cmd="taskix__subcmd__project__subcmd__help"
                ;;
            taskix__subcmd__project,list)
                cmd="taskix__subcmd__project__subcmd__list"
                ;;
            taskix__subcmd__project,register)
                cmd="taskix__subcmd__project__subcmd__register"
                ;;
            taskix__subcmd__project,show)
                cmd="taskix__subcmd__project__subcmd__show"
                ;;
            taskix__subcmd__project,unarchive)
                cmd="taskix__subcmd__project__subcmd__unarchive"
                ;;
            taskix__subcmd__project__subcmd__help,archive)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__archive"
                ;;
            taskix__subcmd__project__subcmd__help,delete)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__delete"
                ;;
            taskix__subcmd__project__subcmd__help,help)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__project__subcmd__help,list)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__list"
                ;;
            taskix__subcmd__project__subcmd__help,register)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__register"
                ;;
            taskix__subcmd__project__subcmd__help,show)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__show"
                ;;
            taskix__subcmd__project__subcmd__help,unarchive)
                cmd="taskix__subcmd__project__subcmd__help__subcmd__unarchive"
                ;;
            taskix__subcmd__task,add)
                cmd="taskix__subcmd__task__subcmd__add"
                ;;
            taskix__subcmd__task,block)
                cmd="taskix__subcmd__task__subcmd__block"
                ;;
            taskix__subcmd__task,cancel)
                cmd="taskix__subcmd__task__subcmd__cancel"
                ;;
            taskix__subcmd__task,claim)
                cmd="taskix__subcmd__task__subcmd__claim"
                ;;
            taskix__subcmd__task,depend)
                cmd="taskix__subcmd__task__subcmd__depend"
                ;;
            taskix__subcmd__task,done)
                cmd="taskix__subcmd__task__subcmd__done"
                ;;
            taskix__subcmd__task,fail)
                cmd="taskix__subcmd__task__subcmd__fail"
                ;;
            taskix__subcmd__task,heartbeat)
                cmd="taskix__subcmd__task__subcmd__heartbeat"
                ;;
            taskix__subcmd__task,help)
                cmd="taskix__subcmd__task__subcmd__help"
                ;;
            taskix__subcmd__task,list)
                cmd="taskix__subcmd__task__subcmd__list"
                ;;
            taskix__subcmd__task,release)
                cmd="taskix__subcmd__task__subcmd__release"
                ;;
            taskix__subcmd__task,reopen)
                cmd="taskix__subcmd__task__subcmd__reopen"
                ;;
            taskix__subcmd__task,retry)
                cmd="taskix__subcmd__task__subcmd__retry"
                ;;
            taskix__subcmd__task,show)
                cmd="taskix__subcmd__task__subcmd__show"
                ;;
            taskix__subcmd__task,start)
                cmd="taskix__subcmd__task__subcmd__start"
                ;;
            taskix__subcmd__task,undepend)
                cmd="taskix__subcmd__task__subcmd__undepend"
                ;;
            taskix__subcmd__task,update)
                cmd="taskix__subcmd__task__subcmd__update"
                ;;
            taskix__subcmd__task,wait)
                cmd="taskix__subcmd__task__subcmd__wait"
                ;;
            taskix__subcmd__task__subcmd__help,add)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__add"
                ;;
            taskix__subcmd__task__subcmd__help,block)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__block"
                ;;
            taskix__subcmd__task__subcmd__help,cancel)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__cancel"
                ;;
            taskix__subcmd__task__subcmd__help,claim)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__claim"
                ;;
            taskix__subcmd__task__subcmd__help,depend)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__depend"
                ;;
            taskix__subcmd__task__subcmd__help,done)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__done"
                ;;
            taskix__subcmd__task__subcmd__help,fail)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__fail"
                ;;
            taskix__subcmd__task__subcmd__help,heartbeat)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__heartbeat"
                ;;
            taskix__subcmd__task__subcmd__help,help)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__help"
                ;;
            taskix__subcmd__task__subcmd__help,list)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__list"
                ;;
            taskix__subcmd__task__subcmd__help,release)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__release"
                ;;
            taskix__subcmd__task__subcmd__help,reopen)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__reopen"
                ;;
            taskix__subcmd__task__subcmd__help,retry)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__retry"
                ;;
            taskix__subcmd__task__subcmd__help,show)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__show"
                ;;
            taskix__subcmd__task__subcmd__help,start)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__start"
                ;;
            taskix__subcmd__task__subcmd__help,undepend)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__undepend"
                ;;
            taskix__subcmd__task__subcmd__help,update)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__update"
                ;;
            taskix__subcmd__task__subcmd__help,wait)
                cmd="taskix__subcmd__task__subcmd__help__subcmd__wait"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        taskix)
            opts="-h -V --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help --version inbox completions init obsidian doctor sync project job task plan event context hook help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__completions)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help bash elvish fish powershell zsh"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__context)
            opts="-h --task --job --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --task)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --job)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__doctor)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__event)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help list help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__event__subcmd__help)
            opts="list help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__event__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__event__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__event__subcmd__list)
            opts="-h --job --after --limit --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --job)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --after)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --limit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help)
            opts="inbox completions init obsidian doctor sync project job task plan event context hook help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__context)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__doctor)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__event)
            opts="list"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__event__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook)
            opts="record stop session-start session-end interrupt heartbeat"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__heartbeat)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__interrupt)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__record)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__session__subcmd__end)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__session__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__hook__subcmd__stop)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox)
            opts="add list sync claim-next release cancel set-status"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__add)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__claim__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__release)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__set__subcmd__status)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__inbox__subcmd__sync)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__init)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job)
            opts="followup submit approve reject delete create update list show cancel archive unarchive"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__approve)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__archive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__followup)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__reject)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__submit)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__unarchive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__job__subcmd__update)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__obsidian)
            opts="connection show snapshot setup"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__obsidian__subcmd__connection)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__obsidian__subcmd__setup)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__obsidian__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__obsidian__subcmd__snapshot)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__plan)
            opts="create revise show"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__plan__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__plan__subcmd__revise)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__plan__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project)
            opts="delete register list show archive unarchive"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__archive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__register)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__project__subcmd__unarchive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__sync)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task)
            opts="add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__add)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__block)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__claim)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__depend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__done)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__fail)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__heartbeat)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__release)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__reopen)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__retry)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__undepend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__update)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__help__subcmd__task__subcmd__wait)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help record stop session-start session-end interrupt heartbeat help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__heartbeat)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help)
            opts="record stop session-start session-end interrupt heartbeat help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__heartbeat)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__interrupt)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__record)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__session__subcmd__end)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__session__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__help__subcmd__stop)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__interrupt)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__record)
            opts="-h --file --job --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --job)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__session__subcmd__end)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__session__subcmd__start)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__hook__subcmd__stop)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help add list sync claim-next release cancel set-status help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__add)
            opts="-h --content --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --content)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__cancel)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__claim__subcmd__next)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help)
            opts="add list sync claim-next release cancel set-status help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__add)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__claim__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__release)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__set__subcmd__status)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__help__subcmd__sync)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__list)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__release)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__set__subcmd__status)
            opts="-h --status --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --status)
                    COMPREPLY=($(compgen -W "TODO ACTIVE PENDING_REVIEW COMPLETED CANCELLED IN_PROGRESS DONE" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__inbox__subcmd__sync)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__init)
            opts="-h --root --directory --database --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --root)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --directory)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --database)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help followup submit approve reject delete create update list show cancel archive unarchive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__approve)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__archive)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__cancel)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__create)
            opts="-h --name --title --goal --prompt --inbox --review-policy --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --goal)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prompt)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --inbox)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --review-policy)
                    COMPREPLY=($(compgen -W "required none" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__delete)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__followup)
            opts="-h --prompt --inbox --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --prompt)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --inbox)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help)
            opts="followup submit approve reject delete create update list show cancel archive unarchive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__approve)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__archive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__followup)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__reject)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__submit)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__unarchive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__help__subcmd__update)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__list)
            opts="-h --active --pending-review --completed --archived --period --created-from --created-to --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --period)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --created-from)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --created-to)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__reject)
            opts="-h --reason --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --reason)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__show)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__submit)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__unarchive)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__job__subcmd__update)
            opts="-h --name --title --goal --prompt --inbox --review-policy --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --goal)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prompt)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --inbox)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --review-policy)
                    COMPREPLY=($(compgen -W "required none" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help connection show snapshot setup help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__connection)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help)
            opts="connection show snapshot setup help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help__subcmd__connection)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help__subcmd__setup)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__help__subcmd__snapshot)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__setup)
            opts="-h --plugin-dir --no-reload --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --plugin-dir)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__show)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__obsidian__subcmd__snapshot)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help create revise show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__create)
            opts="-h --body --file --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --body)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --file)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__help)
            opts="create revise show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__help__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__help__subcmd__revise)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__revise)
            opts="-h --body --file --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --body)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --file)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__plan__subcmd__show)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help delete register list show archive unarchive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__archive)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__delete)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help)
            opts="delete register list show archive unarchive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__archive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__delete)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__register)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__help__subcmd__unarchive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__list)
            opts="-h --archived --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__register)
            opts="-h --name --root --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --root)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__show)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__project__subcmd__unarchive)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__sync)
            opts="-h --pending --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__add)
            opts="-h --name --job --title --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --job)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__block)
            opts="-h --reason --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --reason)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__cancel)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__claim)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__depend)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__done)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__fail)
            opts="-h --reason --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --reason)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__heartbeat)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help)
            opts="add update list show depend undepend claim start heartbeat release block wait fail done cancel retry reopen help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__add)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__block)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__cancel)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__claim)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__depend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__done)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__fail)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__heartbeat)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__list)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__release)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__reopen)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__retry)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__undepend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__update)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__help__subcmd__wait)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__list)
            opts="-h --job --ready --status --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --job)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --status)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__release)
            opts="-h --reason --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --reason)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__reopen)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__retry)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__show)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__start)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__undepend)
            opts="-h --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__update)
            opts="-h --name --title --position --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --position)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        taskix__subcmd__task__subcmd__wait)
            opts="-h --reason --config --json --project --actor --executor --session --delegated-by --lease-token --expect-revision --idempotency-key --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --reason)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    local oldifs
                    if [ -n "${IFS+x}" ]; then
                        oldifs="$IFS"
                    fi
                    IFS=$'\n'
                    COMPREPLY=($(compgen -f "${cur}"))
                    if [ -n "${oldifs+x}" ]; then
                        IFS="$oldifs"
                    fi
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o filenames
                    fi
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --executor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --session)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delegated-by)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-token)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --expect-revision)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --idempotency-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _taskix -o nosort -o bashdefault -o default taskix
else
    complete -F _taskix -o bashdefault -o default taskix
fi
