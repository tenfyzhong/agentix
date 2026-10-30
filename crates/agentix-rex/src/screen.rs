//! Decode the bounded VT snapshot emitted by Rex's format method.
//! Unknown controls fail closed instead of guessing a composer's position.
use crate::{MultiplexerError, error};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Default, PartialEq)]
struct Style {
    background: String,
    dim: bool,
}
#[derive(Clone)]
struct Cell {
    text: String,
    style: Style,
}
fn blank(style: &Style) -> Cell {
    Cell {
        text: " ".into(),
        style: style.clone(),
    }
}

// Keep the fail-closed VT state transitions together.
#[allow(clippy::too_many_lines)]
pub(crate) fn decode_vt(
    input: &str,
    rows: usize,
    cols: usize,
) -> Result<(String, usize, usize), MultiplexerError> {
    if rows == 0 || cols == 0 || rows > 1024 || cols > 4096 || rows * cols > 1_048_576 {
        return Err(error("Invalid Rex terminal size"));
    }
    let invalid = || error("Unsupported or incomplete Rex VT snapshot");
    let mut cells = vec![vec![blank(&Style::default()); cols]; rows];
    let (mut x, mut y) = (0usize, 0usize);
    let mut style = Style::default();
    let mut rest = input;
    let mut cursor = false;
    while !rest.is_empty() {
        if let Some(csi) = rest.strip_prefix("\x1b[") {
            let end = csi
                .bytes()
                .position(|b| (0x40..=0x7e).contains(&b))
                .ok_or_else(invalid)?;
            let params = &csi[..end];
            let command = csi.as_bytes()[end];
            let values: Vec<usize> = params
                .split(';')
                .map(|v| {
                    if v.is_empty() {
                        Ok(0)
                    } else {
                        v.parse().map_err(|_| invalid())
                    }
                })
                .collect::<Result<_, _>>()?;
            let count = values.first().copied().unwrap_or(0).max(1);
            match command {
                b'H' | b'f' => {
                    y = count - 1;
                    x = values.get(1).copied().unwrap_or(1).max(1) - 1;
                    if y >= rows || x >= cols {
                        return Err(invalid());
                    }
                    cursor = true;
                }
                b'C' => {
                    x = x.saturating_add(count).min(cols - 1);
                }
                b'D' => {
                    x = x.saturating_sub(count);
                }
                b'G' => {
                    x = count - 1;
                    if x >= cols {
                        return Err(invalid());
                    }
                }
                b'X' => {
                    for cell in cells[y].iter_mut().skip(x).take(count) {
                        *cell = blank(&style);
                    }
                }
                b'K' => {
                    let (start, end) = match values[0] {
                        0 => (x.min(cols), cols),
                        1 => (0, (x + 1).min(cols)),
                        2 => (0, cols),
                        _ => return Err(invalid()),
                    };
                    for cell in &mut cells[y][start..end] {
                        *cell = blank(&style);
                    }
                }
                b'm' => {
                    let mut i = 0;
                    while i < values.len() {
                        match values[i] {
                            0 => style = Style::default(),
                            2 => style.dim = true,
                            22 => style.dim = false,
                            49 => style.background.clear(),
                            40..=47 | 100..=107 => style.background = values[i].to_string(),
                            38 | 48 => {
                                let n = match values.get(i + 1) {
                                    Some(2) => 5,
                                    Some(5) => 3,
                                    _ => return Err(invalid()),
                                };
                                if i + n > values.len()
                                    || values[i + 2..i + n].iter().any(|v| *v > 255)
                                {
                                    return Err(invalid());
                                }
                                if values[i] == 48 {
                                    style.background = values[i..i + n]
                                        .iter()
                                        .map(ToString::to_string)
                                        .collect::<Vec<_>>()
                                        .join(";");
                                }
                                i += n - 1;
                            }
                            // Other rendition attributes do not affect our dialog recognition.
                            1 | 3..=9 | 21 | 23..=29 | 30..=37 | 39 | 90..=97 | 53 | 55 | 59 => {}
                            _ => return Err(invalid()),
                        }
                        i += 1;
                    }
                }
                _ => return Err(invalid()),
            }
            rest = &csi[end + 1..];
            continue;
        }
        let ch = rest.graphemes(true).next().ok_or_else(invalid)?;
        rest = &rest[ch.len()..];
        match ch {
            "\r" => x = 0,
            "\n" | "\r\n" => {
                if ch == "\r\n" {
                    x = 0;
                }
                y += 1;
                if y == rows {
                    cells.remove(0);
                    cells.push(vec![blank(&style); cols]);
                    y -= 1;
                }
            }
            "\t" => x = ((x / 8 + 1) * 8).min(cols - 1),
            ch if ch.chars().any(char::is_control) => return Err(invalid()),
            ch => {
                let width = ch.width();
                if width > cols {
                    return Err(invalid());
                }
                if width == 0 {
                    if x > 0 {
                        let mut previous = x.min(cols) - 1;
                        while previous > 0 && cells[y][previous].text.is_empty() {
                            previous -= 1;
                        }
                        cells[y][previous].text.push_str(ch);
                    }
                    continue;
                }
                if x + width > cols {
                    x = 0;
                    y += 1;
                    if y == rows {
                        cells.remove(0);
                        cells.push(vec![blank(&style); cols]);
                        y -= 1;
                    }
                }
                cells[y][x] = Cell {
                    text: ch.to_string(),
                    style: style.clone(),
                };
                for offset in 1..width {
                    cells[y][x + offset] = Cell {
                        text: String::new(),
                        style: style.clone(),
                    };
                }
                x += width;
            }
        }
    }
    if !cursor || x >= cols {
        return Err(invalid());
    }
    let mut output = String::new();
    for (index, row) in cells.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        let mut previous: Option<&Style> = None;
        for cell in row {
            if cell.text.is_empty() {
                continue;
            }
            if previous != Some(&cell.style) {
                output.push_str("\x1b[0m");
                if !cell.style.background.is_empty() {
                    output.push_str(&format!("\x1b[{}m", cell.style.background));
                }
                if cell.style.dim {
                    output.push_str("\x1b[2m");
                }
                previous = Some(&cell.style);
            }
            output.push_str(&cell.text);
        }
        output.push_str("\x1b[0m");
    }
    Ok((output, x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joined_emoji_is_one_wide_grapheme() {
        let (screen, _, _) = decode_vt("👩‍💻abc\x1b[1;6H", 2, 6).unwrap();
        assert!(screen.lines().next().unwrap().contains("👩‍💻abc"));
    }
    #[test]
    fn wide_grapheme_cannot_overflow_a_one_column_terminal() {
        assert!(decode_vt("中\x1b[1;1H", 2, 1).is_err());
    }
}
