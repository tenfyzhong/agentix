use super::*;

#[cfg(unix)]
#[test]
fn context_discovers_directory_once_without_reading_remote() {
    use std::os::unix::fs::PermissionsExt;
    let cli = Cli::new();
    let git = Command::new("which").arg("git").output().unwrap();
    assert!(git.status.success());
    let git = String::from_utf8(git.stdout).unwrap();
    let shim = cli.dir.path().join("bin");
    std::fs::create_dir(&shim).unwrap();
    let log = cli.dir.path().join("git.log");
    std::fs::write(shim.join("git"), "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$TASKIX_GIT_LOG\"\nexec \"$TASKIX_REAL_GIT\" \"$@\"\n").unwrap();
    std::fs::set_permissions(shim.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(shim).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for git_repo in [false, true] {
        if git_repo {
            assert!(
                Command::new(git.trim())
                    .args(["init", "--quiet"])
                    .arg(cli.dir.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        for _ in 0..2 {
            std::fs::write(&log, "").unwrap();
            let output = cli
                .command(&["context"])
                .env("PATH", &path)
                .env("TASKIX_REAL_GIT", git.trim())
                .env("TASKIX_GIT_LOG", &log)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            let calls = std::fs::read_to_string(&log).unwrap();
            assert_eq!(
                calls.lines().count(),
                1,
                "one discovery per context: {calls}"
            );
            assert!(
                !calls.contains("remote"),
                "lookup must not query remote: {calls}"
            );
            let id =
                serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["project_id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
            std::fs::write(&log, "").unwrap();
            let explicit = cli
                .command(&["context", "--project", &id])
                .env("PATH", &path)
                .env("TASKIX_REAL_GIT", git.trim())
                .env("TASKIX_GIT_LOG", &log)
                .output()
                .unwrap();
            assert!(explicit.status.success());
            assert!(
                std::fs::read_to_string(&log).unwrap().is_empty(),
                "explicit selection needs no discovery"
            );
        }
    }
}

/// Reusable end-to-end benchmark; timings are reports, never flaky CI thresholds.
#[tokio::test]
#[ignore = "manual project scaling benchmark"]
async fn project_resolution_scaling_benchmark() {
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
    use std::time::Instant;
    for count in [1, 1000, 10000] {
        let cli = Cli::new();
        let template = cli.ok(&["project", "register", "--name", "seed"]);
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(cli.dir.path().join("state.sqlite3")),
        )
        .await
        .unwrap();
        let has_lookup: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='project_lookup')",
        )
        .fetch_one(&mut db)
        .await
        .unwrap();
        sqlx::query("BEGIN").execute(&mut db).await.unwrap();
        for index in 0..count {
            let path = cli.dir.path().join(format!("directory-{index}"));
            std::fs::create_dir(&path).unwrap();
            let mut project = template.clone();
            project["id"] = json!(format!("prj_bench_{index}"));
            project["name"] = json!(format!("directory-{index}"));
            project["key"] = project["name"].clone();
            project["root"] = json!(path.canonicalize().unwrap());
            sqlx::query("INSERT INTO projects(id,data) VALUES (?,?)")
                .bind(project["id"].as_str().unwrap())
                .bind(project.to_string())
                .execute(&mut db)
                .await
                .unwrap();
            if has_lookup {
                sqlx::query("INSERT INTO project_lookup(project_id,canonical_root,folded_key) VALUES (?,?,?)")
                    .bind(project["id"].as_str().unwrap()).bind(project["root"].as_str().unwrap()).bind(project["key"].as_str().unwrap())
                    .execute(&mut db).await.unwrap();
            }
        }
        sqlx::query("COMMIT").execute(&mut db).await.unwrap();
        for scenario in ["warm", "cold"] {
            let mut timings = Vec::new();
            for sample in 0..20 {
                let path = if scenario == "warm" {
                    cli.dir.path().join(format!("directory-{}", count - 1))
                } else {
                    let p = cli.dir.path().join(format!("fresh-{sample}"));
                    std::fs::create_dir(&p).unwrap();
                    p
                };
                let start = Instant::now();
                let output = cli
                    .command(&["context"])
                    .current_dir(path)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stdout)
                );
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            timings.sort_by(f64::total_cmp);
            eprintln!(
                "project_benchmark count={count} scenario={scenario} median_ms={:.3} p95_ms={:.3}",
                timings[10], timings[18]
            );
        }
    }
}

#[test]
fn concurrent_directory_contexts_create_one_project_and_one_registration_event() {
    let cli = Cli::new();
    let children: Vec<_> = (0..6)
        .map(|_| {
            cli.command(&["context"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let ids: Vec<_> = children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["result"]["project_id"].clone()
        })
        .collect();
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
    for _ in 0..3 {
        assert_eq!(cli.ok(&["context"])["project_id"], ids[0]);
    }
    let events = cli.ok(&["event", "list"]);
    assert_eq!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event_type"] == "project.register")
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn symlink_directory_and_real_directory_reuse_one_project() {
    let cli = Cli::new();
    let real = cli.dir.path().join("客户 项目");
    let link = cli.dir.path().join("alias");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let run = |cwd: &std::path::Path| {
        let out = cli.command(&["context"]).current_dir(cwd).output().unwrap();
        assert!(out.status.success());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["result"]["project_id"].clone()
    };
    let id = run(&link);
    assert_eq!(run(&real), id);
    let project = cli.ok(&["project", "show", id.as_str().unwrap()]);
    assert_eq!(project["name"], "客户 项目");
    assert_eq!(project["root"], json!(real.canonicalize().unwrap()));
}

#[test]
fn same_directory_names_keep_distinct_projects_and_explicit_context_wins() {
    let cli = Cli::new();
    let mut ids = Vec::new();
    for parent in ["first", "second"] {
        let root = cli.dir.path().join(parent).join("customer");
        std::fs::create_dir_all(&root).unwrap();
        let out = cli
            .command(&["context"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success());
        let id =
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["result"]["project_id"].clone();
        assert!(!ids.contains(&id));
        ids.push(id);
    }
    assert_eq!(
        cli.ok(&["project", "show", ids[0].as_str().unwrap()])["name"],
        "customer"
    );
    assert_eq!(
        cli.ok(&["project", "show", ids[1].as_str().unwrap()])["name"],
        "customer-2"
    );
    assert_eq!(
        cli.ok(&["context", "--project", ids[0].as_str().unwrap()])["project_id"],
        ids[0]
    );
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 2);
}

#[test]
fn directory_registration_recovers_pending_projection_without_duplicate_projects() {
    let cli = Cli::new();
    let projects = cli.dir.path().join("vault/Tasks \u{2603}/Projects");
    if projects.exists() {
        std::fs::remove_dir_all(&projects).unwrap();
    }
    std::fs::write(&projects, "block projection").unwrap();
    let failed = cli.run(&["context"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stdout).contains("Project synchronization pending"));
    let project = cli.ok(&["project", "list"])[0].clone();
    std::fs::remove_file(projects).unwrap();
    assert_eq!(cli.ok(&["context"])["project_id"], project["id"]);
    assert_eq!(cli.ok(&["project", "list"]).as_array().unwrap().len(), 1);
    assert_eq!(cli.ok(&["doctor"])["healthy"], true);
}
