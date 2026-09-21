//! Capacity-based pages. The budget includes JSON string escaping and leaves
//! room for the `OpenAPI` envelope; it is not a per-section text allowance.
use agentix_domain::{ChannelError, OutboundView, ViewSection};

use super::{card_sections::wire_json, render_card_with_action_state};

const CARD_WIRE_BUDGET: usize = 25_000;
const ENVELOPE_RESERVE: usize = 1_024;
const MAX_SECTIONS: usize = 80;

#[derive(Clone)]
pub(super) struct Page {
    pub view: OutboundView,
    pub json: String,
}

pub(super) fn render(view: OutboundView) -> Result<Page, ChannelError> {
    let json = wire_json(&render_card_with_action_state(&view, false)?)?;
    Ok(Page { view, json })
}

fn fits(view: &OutboundView) -> Result<bool, ChannelError> {
    if view.sections.len() > MAX_SECTIONS {
        return Ok(false);
    }
    let body_bytes = if view.sections.is_empty() {
        view.body.len()
    } else {
        view.sections.iter().map(|section| section.body.len()).sum()
    };
    if body_bytes > CARD_WIRE_BUDGET {
        return Ok(false);
    }
    let json = wire_json(&render_card_with_action_state(view, false)?)?;
    let escaped = serde_json::to_string(&json)
        .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
    Ok(escaped.len() + ENVELOPE_RESERVE <= CARD_WIRE_BUDGET)
}

fn empty_page(view: &OutboundView, index: usize) -> OutboundView {
    OutboundView {
        title: if index == 0 {
            view.title.clone()
        } else {
            format!("{} · {}", view.title, index + 1)
        },
        subtitle: view.subtitle.clone(),
        body: String::new(),
        sections: Vec::new(),
        status: view.status,
        // Callbacks continue to refer to the logical message's first card.
        actions: if index == 0 {
            view.actions.clone()
        } else {
            Vec::new()
        },
    }
}

pub(super) fn paginate(view: &OutboundView) -> Result<Vec<Page>, ChannelError> {
    if fits(view)? {
        return Ok(vec![render(view.clone())?]);
    }
    let sections = if view.sections.is_empty() {
        vec![ViewSection {
            body: view.body.clone(),
            ..ViewSection::default()
        }]
    } else {
        view.sections.clone()
    };
    let mut pages = Vec::new();
    let mut current = empty_page(view, 0);
    if !fits(&current)? {
        return Err(ChannelError::InvalidPayload(
            "Feishu card header or actions exceed the card budget".into(),
        ));
    }
    for (index, mut section) in sections.into_iter().enumerate() {
        // Resolve the default against the original view, not each page.
        if section.collapsible && section.expanded.is_none() {
            section.expanded = Some(index + 1 == view.sections.len());
        }
        section.action_tokens.clear();
        current.sections.push(section.clone());
        if fits(&current)? {
            continue;
        }
        current.sections.pop();
        if !current.sections.is_empty() {
            pages.push(render(current)?);
            current = empty_page(view, pages.len());
        }
        let text = std::mem::take(&mut section.body);
        let mut rest = text.as_str();
        let mut opening = String::new();
        loop {
            if rest.len() + opening.len() <= CARD_WIRE_BUDGET {
                section.body = format!("{opening}{rest}");
                current.sections.push(section.clone());
                if fits(&current)? {
                    break;
                }
                current.sections.pop();
            }
            // The encoded body cannot be smaller than its UTF-8 input. Bound
            // each search independently of the total response length.
            let mut low = 0;
            let mut high = rest.len().min(CARD_WIRE_BUDGET);
            while low < high {
                let midpoint = low + (high - low).div_ceil(2);
                let end = rest.floor_char_boundary(midpoint);
                section.body = fenced_fragment(&opening, &rest[..end]).0;
                current.sections.push(section.clone());
                let fits = fits(&current)?;
                current.sections.pop();
                if fits {
                    low = midpoint;
                } else {
                    high = midpoint - 1;
                }
            }
            let mut end = rest.floor_char_boundary(low);
            if end == 0 {
                return Err(ChannelError::InvalidPayload(
                    "Feishu section metadata leaves no room for content".into(),
                ));
            }
            end = preferred_boundary(&opening, rest, end);
            (section.body, opening) = fenced_fragment(&opening, &rest[..end]);
            current.sections.push(section.clone());
            pages.push(render(current)?);
            current = empty_page(view, pages.len());
            rest = &rest[end..];
        }
    }
    pages.push(render(current)?);
    Ok(pages)
}

#[derive(Clone)]
struct Fence {
    marker: char,
    count: usize,
    opening: String,
}

fn visit_fence(state: &mut Option<Fence>, line: &str) {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return;
    }
    let Some(marker @ ('`' | '~')) = trimmed.chars().next() else {
        return;
    };
    let count = trimmed.chars().take_while(|c| *c == marker).count();
    if count < 3 {
        return;
    }
    if let Some(fence) = state {
        if marker == fence.marker && count >= fence.count && trimmed[count..].trim().is_empty() {
            *state = None;
        }
    } else {
        *state = Some(Fence {
            marker,
            count,
            opening: format!("{}\n", line.trim_end_matches(['\r', '\n'])),
        });
    }
}

fn fenced_fragment(opening: &str, text: &str) -> (String, String) {
    let mut fragment = format!("{opening}{text}");
    let mut state = None;
    for line in fragment.split_inclusive('\n') {
        visit_fence(&mut state, line);
    }
    if let Some(fence) = state {
        if !fragment.ends_with('\n') {
            fragment.push('\n');
        }
        fragment.extend(std::iter::repeat_n(fence.marker, fence.count));
        fragment.push('\n');
        (fragment, fence.opening)
    } else {
        (fragment, String::new())
    }
}

fn preferred_boundary(opening: &str, text: &str, end: usize) -> usize {
    let mut state = None;
    visit_fence(&mut state, opening);
    let mut offset = 0;
    let mut paragraph = None;
    let mut outside_line = None;
    let mut any_line = None;
    for line in text[..end].split_inclusive('\n') {
        offset += line.len();
        visit_fence(&mut state, line);
        if line.ends_with('\n') {
            any_line = Some(offset);
            if state.is_none() {
                outside_line = Some(offset);
                if line.trim().is_empty() {
                    paragraph = Some(offset);
                }
            }
        }
    }
    paragraph.or(outside_line).or(any_line).unwrap_or(end)
}
