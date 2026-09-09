use std::{env, fs, io::Write, path::Path, process, thread, time::Duration};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let scenario = env::var("OBSIDIAN_TEST_SCENARIO").unwrap();
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("cli-calls")
        .unwrap();
    writeln!(log, "{}", args.join(" ")).unwrap();
    let command = args
        .iter()
        .find(|arg| !arg.starts_with("vault="))
        .unwrap()
        .as_str();
    match command {
        "vault" => {
            if scenario == "startup-settings" {
                fs::write(
                    ".obsidian/core-plugins.json",
                    r#"{"graph":true,"bases":false}"#,
                )
                .unwrap();
                fs::write(".obsidian/community-plugins.json", r#"["other"]"#).unwrap();
            }
            if scenario == "wrong-vault" {
                println!("/another/vault");
            } else {
                println!("{}", env::current_dir().unwrap().display());
            }
        }
        "plugins:enabled" => {
            if [
                "loaded",
                "publication-fails",
                "loaded-reload-fails",
                "disable-fails",
            ]
            .contains(&scenario.as_str())
            {
                println!(r#"[{{"id":"other"}},{{"id":"tasknotes"}},{{"id":"taskix-sync"}}]"#);
            } else {
                println!("[]");
            }
        }
        "plugin:disable" => {
            fs::create_dir_all(".obsidian/plugins/tasknotes").unwrap();
            fs::write(
                ".obsidian/plugins/tasknotes/data.json",
                r#"{"calendarView":"month","taskTag":"old"}"#,
            )
            .unwrap();
            fs::write(".obsidian/community-plugins.json", r#"["other"]"#).unwrap();
            if scenario == "publication-fails" {
                fs::write(".obsidian/plugins/tasknotes/data.json", "invalid").unwrap();
            }
            if scenario == "disable-fails" {
                eprintln!("Error: disable failed");
                process::exit(1);
            }
            println!("Disabled plugin");
        }
        "plugin:enable" => {
            if scenario == "loaded-reload-fails" || scenario == "disable-fails" {
                fs::write(
                    ".obsidian/community-plugins.json",
                    r#"["other","taskix-sync"]"#,
                )
                .unwrap();
            }
            println!("Enabled plugin");
        }
        "reload" => {
            assert!(Path::new(".obsidian/plugins/tasknotes/main.js").is_file());
            assert!(Path::new(".obsidian/plugins/taskix-sync/main.js").is_file());
            match scenario.as_str() {
                "reload-fails" | "loaded-reload-fails" => {
                    eprintln!("Obsidian unavailable");
                    process::exit(1);
                }
                "reload-error-output" => {
                    println!("Error: reload unavailable");
                    return;
                }
                "reload-timeout" => thread::sleep(Duration::from_secs(60)),
                _ => {}
            }
            fs::write("reloaded", "yes").unwrap();
            println!("Reloading vault...");
        }
        _ => panic!("unexpected command: {args:?}"),
    }
}
