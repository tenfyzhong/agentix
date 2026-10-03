//! The two terminal drivers share a restricted tmux-compatible command surface.
use super::MultiplexerError;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

fn error(value: impl std::fmt::Display) -> MultiplexerError {
    MultiplexerError::Backend(value.to_string())
}

pub(crate) fn validate_codex_prompt(state: &str, screen: &str) -> Result<(), MultiplexerError> {
    let parts: Vec<_> = state.trim().split('|').collect();
    let valid = parts.len() == 5
        && parts[0] == "codex"
        && parts[1] == "2"
        && parts[3] == "0"
        && parts[4] == "0"
        && parts[2]
            .parse::<usize>()
            .ok()
            .and_then(|row| screen.lines().nth(row))
            .is_some_and(|line| line.starts_with("› "));
    if valid {
        Ok(())
    } else {
        Err(error(
            "Codex must be at an empty input prompt; dismiss dialogs and clear drafts",
        ))
    }
}

fn codex_draft(state: &str, screen: &str) -> Result<Option<String>, MultiplexerError> {
    let parts: Vec<_> = state.trim().split('|').collect();
    if parts.len() != 5 || parts[0] != "codex" || parts[3] != "0" || parts[4] != "0" {
        return Err(error("Original Codex input is not ready"));
    }
    let row = parts[2].parse::<usize>().map_err(error)?;
    let lines: Vec<_> = screen.lines().collect();
    let border =
        |line: &&str| line.trim().chars().count() >= 3 && line.trim().chars().all(|c| c == '─');
    let top = lines
        .iter()
        .take(row)
        .rposition(border)
        .ok_or_else(|| error("Cannot read the entire Codex input box"))?;
    let bottom = lines
        .iter()
        .enumerate()
        .skip(row + 1)
        .find(|(_, line)| border(line))
        .map(|(i, _)| i)
        .ok_or_else(|| error("Cannot read the entire Codex input box"))?;
    if !lines
        .get(top + 1)
        .is_some_and(|line| line.starts_with("› "))
    {
        return Err(error("Dismiss the Codex dialog before sending"));
    }
    let draft = lines[top + 1..bottom]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.strip_prefix("› ").unwrap_or(line)
            } else {
                line.strip_prefix("  ").unwrap_or(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok((!draft.is_empty()).then_some(draft))
}

// Preserve the composer's background boundary and dim placeholder information.
type StyledLine = (String, Option<String>, bool);

pub(crate) fn styled_lines(screen: &str) -> Result<Vec<StyledLine>, MultiplexerError> {
    let mut background: Option<String> = None;
    let mut dim = false;
    let mut rows = Vec::new();
    for raw in screen.lines() {
        let mut text = String::new();
        let mut row_background = None;
        let mut placeholder = false;
        let mut rest = raw;
        while !rest.is_empty() {
            if let Some(escape) = rest.strip_prefix("\x1b[") {
                let end = escape
                    .find('m')
                    .ok_or_else(|| error("Unrecognized terminal escape"))?;
                let codes: Vec<_> = escape[..end].split(';').collect();
                let mut n = 0;
                while n < codes.len() {
                    match codes[n] {
                        "0" | "" => {
                            background = None;
                            dim = false;
                        }
                        "2" => dim = true,
                        "22" => dim = false,
                        "49" => background = None,
                        "48" => {
                            let count = if codes.get(n + 1) == Some(&"2") { 5 } else { 3 };
                            if n + count > codes.len() {
                                return Err(error("Invalid background escape"));
                            }
                            background = Some(codes[n..n + count].join(";"));
                            n += count - 1;
                        }
                        "38" => {
                            n += if codes.get(n + 1) == Some(&"2") { 4 } else { 2 };
                        }
                        code if code
                            .parse::<u8>()
                            .is_ok_and(|v| (40..=47).contains(&v) || (100..=107).contains(&v)) =>
                        {
                            background = Some(code.into());
                        }
                        _ => {}
                    }
                    if text.is_empty() && row_background.is_none() {
                        row_background.clone_from(&background);
                    }
                    n += 1;
                }
                rest = &escape[end + 1..];
            } else {
                let ch = rest.chars().next().unwrap();
                if text.is_empty() {
                    row_background.clone_from(&background);
                }
                if text == "› " && dim {
                    placeholder = true;
                }
                text.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
        rows.push((text.trim_end().to_owned(), row_background, placeholder));
    }
    Ok(rows)
}

fn codex_ansi_draft(state: &str, screen: &str) -> Result<Option<String>, MultiplexerError> {
    let rows = styled_lines(screen)?;
    let parts: Vec<_> = state.trim().split('|').collect();
    let row = parts
        .get(2)
        .ok_or_else(|| error("Missing cursor"))?
        .parse::<usize>()
        .map_err(error)?;
    if rows
        .iter()
        .any(|r| r.0.contains("Vim: Normal") || r.0.contains("Vim: Visual"))
    {
        return Err(error("Switch Codex to insert mode before sending"));
    }
    let Some((_, Some(bg), _)) = rows.get(row) else {
        return codex_uncolored_draft(state, &rows, row);
    };
    let mut top = row;
    while top > 0 && rows[top - 1].1.as_ref() == Some(bg) {
        top -= 1;
    }
    let mut bottom = row;
    while bottom + 1 < rows.len() && rows[bottom + 1].1.as_ref() == Some(bg) {
        bottom += 1;
    }
    if bottom <= top + 1 || !rows[top].0.is_empty() || !rows[bottom].0.is_empty() {
        return Err(error("Cannot read the entire Codex composer"));
    }
    let placeholder = rows[top + 1].2;
    let mut plain: Vec<_> = rows.iter().map(|r| r.0.clone()).collect();
    plain[top] = "───".into();
    plain[bottom] = "───".into();
    if placeholder || plain[top + 1] == "›" {
        plain[top + 1] = "› ".into();
    }
    let draft = codex_draft(state, &plain.join("\n"))?;
    if draft
        .as_ref()
        .is_some_and(|text| text.contains("[Pasted Content") || text.contains("[Image #"))
    {
        return Err(error(
            "Expand pasted content and remove attachments before confirming terminal input",
        ));
    }
    Ok(draft)
}

fn codex_model_directory_footer(line: &str) -> bool {
    let mut fields = line.trim_start().split(" · ");
    let Some((model, effort)) = fields.next().and_then(|model| model.rsplit_once(' ')) else {
        return false;
    };
    !model.is_empty()
        && matches!(
            effort,
            "default" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
        && fields
            .next()
            .is_some_and(|cwd| Path::new(cwd).is_absolute())
        && fields.next().is_none_or(|hint| hint == "← for agents")
        && fields.next().is_none()
}

fn codex_uncolored_draft(
    state: &str,
    rows: &[StyledLine],
    row: usize,
) -> Result<Option<String>, MultiplexerError> {
    let mut plain: Vec<_> = rows.iter().map(|r| r.0.clone()).collect();
    if let Some(top) = rows
        .iter()
        .take(row + 1)
        .rposition(|r| r.0 == "›" || r.0.starts_with("› "))
        && top > 0
        && rows[top - 1].0.is_empty()
        && let Some(footer) = rows.iter().enumerate().skip(row + 2).find_map(|(i, r)| {
            (r.0.starts_with("  ")
                && (r.0.contains("Context ")
                    || r.0.contains("context left")
                    || r.0.contains("? for shortcuts")
                    || codex_model_directory_footer(&r.0)
                    || r.0.contains("Vim: Insert")))
            .then_some(i)
        })
        && rows[footer - 1].0.is_empty()
    {
        plain[top - 1] = "───".into();
        plain[footer - 1] = "───".into();
        if rows[top].2 || plain[top] == "›" {
            plain[top] = "› ".into();
        }
    }
    let draft = codex_draft(state, &plain.join("\n"))?;
    if draft
        .as_ref()
        .is_some_and(|s| s.contains("[Pasted Content") || s.contains("[Image #"))
    {
        return Err(error(
            "Expand pasted content and remove attachments before confirming terminal input",
        ));
    }
    Ok(draft)
}

async fn read_draft(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
) -> Result<Option<String>, MultiplexerError> {
    verify_process(pid).await?;
    let state = run(
        command,
        prefix,
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
        ],
    )
    .await?;
    let screen = run(command, prefix, &["capture-pane", "-p", "-e", "-t", pane]).await?;
    codex_ansi_draft(&state, &screen)
}

/// Read the whole visible input, or clear only the exact confirmed draft.
pub async fn codex_terminal_input(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    expected: Option<&str>,
) -> Result<Option<String>, MultiplexerError> {
    let current = read_draft(command, prefix, pane, pid).await?;
    if expected.is_none() || current.as_deref() != expected {
        return Ok(current);
    }
    verify_process(pid).await?;
    run(command, prefix, &["send-keys", "-t", pane, "C-c"]).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let remaining = read_draft(command, prefix, pane, pid).await?;
    if remaining.is_some() {
        return Err(error(
            "Codex input was not cleared; the new request was not sent",
        ));
    }
    Ok(None)
}

pub(crate) async fn run(
    command: &Path,
    prefix: &[String],
    args: &[&str],
) -> Result<String, MultiplexerError> {
    let mut process = Command::new(command);
    process
        .args(prefix)
        .args(args)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .kill_on_drop(true);
    let output = super::terminal_command_output(&mut process).await?;
    if !output.status.success() {
        return Err(error(String::from_utf8_lossy(&output.stderr)));
    }
    String::from_utf8(output.stdout).map_err(error)
}

async fn verify_process(pid: u32) -> Result<(), MultiplexerError> {
    let output = run(
        Path::new("ps"),
        &[],
        &["-p", &pid.to_string(), "-o", "pgid=,tpgid=,comm="],
    )
    .await?;
    let parts: Vec<_> = output.split_whitespace().collect();
    if parts.len() < 3
        || parts[0] != parts[1]
        || parts[0] == "0"
        || parts[0] == "-1"
        || Path::new(parts[2]).file_name().and_then(|v| v.to_str()) != Some("codex")
    {
        return Err(error(
            "Original Codex process is no longer in the foreground",
        ));
    }
    Ok(())
}

async fn wait_for_codex_command(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    text: &str,
) -> Result<(), MultiplexerError> {
    // Unbracketed input is buffered by Codex's paste-burst detector. Enter must
    // arrive only after the command has rendered and remained stable.
    let mut ready = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        verify_process(pid).await?;
        let state = run(
            command,
            prefix,
            &[
                "display-message",
                "-p",
                "-t",
                pane,
                "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
            ],
        )
        .await?;
        let screen = run(command, prefix, &["capture-pane", "-p", "-t", pane]).await?;
        let parts: Vec<_> = state.trim().split('|').collect();
        let matches = parts.len() == 5
            && parts[0] == "codex"
            && parts[1].parse::<usize>().ok() == Some(text.len() + 2)
            && parts[3] == "0"
            && parts[4] == "0"
            && parts[2]
                .parse::<usize>()
                .ok()
                .and_then(|r| screen.lines().nth(r))
                .is_some_and(|line| line.trim_end() == format!("› {text}"));
        if matches && ready {
            return Ok(());
        }
        ready = matches;
    }
    Err(error(format!(
        "Codex did not display {text} ready for submission; Enter was not sent",
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexNewSessionOutcome {
    Started,
    CheckoutChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexCheckoutChoice {
    CurrentCheckout,
    NewWorktree,
    Cancel,
}

fn checkout_selection(screen: &str) -> Result<Option<CodexCheckoutChoice>, MultiplexerError> {
    let rows = styled_lines(screen)?;
    let Some(title) = rows
        .iter()
        .position(|row| row.0.trim() == "Where should the new conversation run?")
    else {
        return Ok(None);
    };
    let choices = rows[title + 1..]
        .iter()
        .filter(|row| !row.0.trim().is_empty())
        .take(2)
        .map(|row| row.0.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>();
    match choices.as_slice() {
        [current, new]
            if current == "› 1. Current checkout Keep using the current working directory"
                && new == "2. New worktree Create an isolated managed checkout" =>
        {
            Ok(Some(CodexCheckoutChoice::CurrentCheckout))
        }
        [current, new]
            if current == "1. Current checkout Keep using the current working directory"
                && new == "› 2. New worktree Create an isolated managed checkout" =>
        {
            Ok(Some(CodexCheckoutChoice::NewWorktree))
        }
        _ => Err(error("Codex checkout choices changed; check the terminal")),
    }
}

async fn codex_new_screen(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
) -> Result<(String, String), MultiplexerError> {
    verify_process(pid).await?;
    let state = run(
        command,
        prefix,
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
        ],
    )
    .await?;
    let screen = run(command, prefix, &["capture-pane", "-p", "-e", "-t", pane]).await?;
    let parts: Vec<_> = state.trim().split('|').collect();
    if parts.len() != 5 || parts[0] != "codex" || parts[3] != "0" || parts[4] != "0" {
        return Err(error("Original Codex input is not ready after /new"));
    }
    Ok((state, screen))
}

async fn finish_codex_new(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
) -> Result<CodexNewSessionOutcome, MultiplexerError> {
    let mut previous = None;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let (state, screen) = codex_new_screen(command, prefix, pane, pid).await?;
        if codex_ansi_draft(&state, &screen).is_ok_and(|draft| draft.is_none()) {
            return Ok(CodexNewSessionOutcome::Started);
        }
        let cursor = state
            .trim()
            .split('|')
            .nth(2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let dialog = super::parse_terminal_interaction(&screen, cursor);
        let selected = dialog.map(|dialog| dialog.fingerprint);
        if selected.is_some() && selected == previous {
            return Ok(CodexNewSessionOutcome::CheckoutChoice);
        }
        previous = selected;
    }
    Err(error(
        "Codex did not finish /new or display checkout choices; check the terminal",
    ))
}

/// Submit only the checkout choice explicitly selected by the IM user.
pub async fn select_codex_checkout(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    choice: CodexCheckoutChoice,
) -> Result<(), MultiplexerError> {
    let mut ready = false;
    for _ in 0..20 {
        let (state, screen) = codex_new_screen(command, prefix, pane, pid).await?;
        if codex_ansi_draft(&state, &screen).is_ok() {
            return Err(error("Codex checkout choice expired; no input was sent"));
        }
        let selected = checkout_selection(&screen)?
            .ok_or_else(|| error("Codex checkout choice expired; no input was sent"))?;
        if choice == CodexCheckoutChoice::Cancel {
            run(command, prefix, &["send-keys", "-t", pane, "Escape"]).await?;
            return Ok(());
        }
        if selected != choice {
            let key = if choice == CodexCheckoutChoice::CurrentCheckout {
                "Up"
            } else {
                "Down"
            };
            run(command, prefix, &["send-keys", "-t", pane, key]).await?;
            ready = false;
        } else if ready {
            run(command, prefix, &["send-keys", "-t", pane, "Enter"]).await?;
            return Ok(());
        } else {
            ready = true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(error(
        "Codex did not display the requested checkout selection; Enter was not sent",
    ))
}

/// Submit Codex's native /new, reporting any checkout picker without choosing an item.
pub async fn send_codex_new(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
) -> Result<CodexNewSessionOutcome, MultiplexerError> {
    submit_codex_command(command, prefix, pane, pid, "/new").await?;
    finish_codex_new(command, prefix, pane, pid).await
}

/// Submit Codex's native /exit to the verified original CLI.
pub async fn send_codex_exit(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
) -> Result<(), MultiplexerError> {
    submit_codex_command(command, prefix, pane, pid, "/exit").await
}

async fn submit_codex_command(
    command: &Path,
    prefix: &[String],
    pane: &str,
    pid: u32,
    text: &str,
) -> Result<(), MultiplexerError> {
    if !pane.starts_with('%') || !pane[1..].chars().all(|c| c.is_ascii_digit()) {
        return Err(error("Invalid pane"));
    }
    verify_process(pid).await?;
    let state = run(
        command,
        prefix,
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_current_command}|#{cursor_x}|#{cursor_y}|#{pane_in_mode}|#{pane_dead}",
        ],
    )
    .await?;
    let screen = run(command, prefix, &["capture-pane", "-p", "-t", pane]).await?;
    validate_codex_prompt(&state, &screen)?;
    if read_draft(command, prefix, pane, pid).await?.is_some() {
        return Err(error(
            "Terminal draft changed; confirm it in IM before sending",
        ));
    }
    run(command, prefix, &["send-keys", "-t", pane, "-l", text]).await?;
    wait_for_codex_command(command, prefix, pane, pid, text).await?;
    run(command, prefix, &["send-keys", "-t", pane, "Enter"]).await?;
    Ok(())
}

#[cfg(test)]
mod draft_tests {
    use super::*;
    #[test]
    fn codex_draft_reads_the_entire_input_box_even_with_cursor_at_start() {
        let screen = "history\n──────────\n› first\n  第二行\n──────────\nfooter";
        assert_eq!(
            codex_draft("codex|2|2|0|0", screen).unwrap(),
            Some("first\n第二行".into())
        );
        assert!(codex_draft("zsh|2|2|0|0", screen).is_err());
        assert!(codex_draft("codex|2|2|1|0", screen).is_err());
        assert!(codex_draft("codex|2|2|0|1", screen).is_err());
        assert!(
            codex_draft(
                "codex|2|2|0|0",
                "history\n──────────\n› hidden continuation"
            )
            .is_err()
        );
        assert_eq!(
            codex_draft("codex|2|1|0|0", "──────────\n› \n──────────").unwrap(),
            None
        );
    }
}

#[cfg(test)]
mod styled_draft_tests {
    use super::codex_ansi_draft;
    #[test]
    fn codex_model_directory_footer_identifies_a_nonempty_composer() {
        let draft =
            "history\n\n› local draft\n  second line\n\n  GPT-6-Astra default · /tmp/repository\n";
        assert_eq!(
            codex_ansi_draft("codex|13|3|0|0", draft).unwrap(),
            Some("local draft\nsecond line".into())
        );
        assert!(
            codex_ansi_draft(
                "codex|13|3|0|0",
                &draft.replace("/tmp/repository", "unrelated dialog")
            )
            .is_err()
        );
    }

    #[test]
    fn codex_agent_navigation_footer_identifies_the_default_background_composer() {
        let empty = "history\n\n\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m\n\n  GPT-6-Astra default · /tmp/repository · ← for agents\n";
        assert_eq!(codex_ansi_draft("codex|2|2|0|0", empty).unwrap(), None);
        let draft = "history\n\n› local draft\n  second line\n\n  GPT-6-Astra default · /tmp/repository · ← for agents\n";
        assert_eq!(
            codex_ansi_draft("codex|13|3|0|0", draft).unwrap(),
            Some("local draft\nsecond line".into())
        );
        assert!(
            codex_ansi_draft(
                "codex|13|3|0|0",
                &draft.replace("← for agents", "unrelated dialog")
            )
            .is_err()
        );
    }

    #[test]
    fn codex_default_background_composer_is_read_using_padding_and_status() {
        let empty = "history\n\n\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m\n\n  gpt-6-astra · Context 0% used    Vim: Insert\n";
        assert_eq!(codex_ansi_draft("codex|2|2|0|0", empty).unwrap(), None);
        let draft =
            "history\n\n› first\n  second\n\n  gpt-6-astra · Context 0% used    Vim: Insert\n";
        assert_eq!(
            codex_ansi_draft("codex|8|3|0|0", draft).unwrap(),
            Some("first\nsecond".into())
        );
        assert!(
            codex_ansi_draft(
                "codex|8|3|0|0",
                &draft.replace("Vim: Insert", "Vim: Normal")
            )
            .is_err()
        );
        assert!(codex_ansi_draft("codex|2|2|0|0", "history\n\n› draft\nother dialog").is_err());
    }
    #[test]
    fn codex_borderless_composer_uses_background_and_preserves_multiline_draft() {
        let screen =
            "history\n\x1b[48;5;235m          \n› first   \n  second  \n          \x1b[0m\nfooter";
        assert_eq!(
            codex_ansi_draft("codex|2|2|0|0", screen).unwrap(),
            Some("first\nsecond".into())
        );
        let empty = "\x1b[48;5;235m          \n\x1b[1m› \x1b[22;2mAsk anything\x1b[22m\n          \x1b[0m\nfooter";
        assert_eq!(codex_ansi_draft("codex|2|1|0|0", empty).unwrap(), None);
    }
}

#[cfg(all(test, unix))]
#[path = "native_command_tests.rs"]
mod command_tests;

#[cfg(all(test, unix))]
#[path = "codex_new_tests.rs"]
mod codex_new_tests;
