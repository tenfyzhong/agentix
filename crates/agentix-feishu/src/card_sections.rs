//! Render typed turn sections without interpreting arbitrary Markdown as controls.
use super::{CARD_BODY_LIMIT, render_action, truncate_utf8};
use agentix_domain::{OutboundView, ViewStatus};
use larksuite_oapi_sdk_rs::card::v2::{
    BackgroundStyle, Body, CollapsiblePanel, CollapsiblePanelHeader, Color, Column, ColumnSet,
    Element, HeaderIcon, IconPosition, Markdown, PanelIconExpandedAngle, Text,
};
use std::collections::HashSet;

pub(super) fn view_body(view: &OutboundView, actions_disabled: bool) -> Body {
    let mut placed_actions = HashSet::new();
    let elements = if view.sections.is_empty() {
        vec![Element::Markdown(Markdown::new(truncate_utf8(
            &view.body,
            CARD_BODY_LIMIT,
        )))]
    } else {
        let limit = CARD_BODY_LIMIT / view.sections.len();
        view.sections
            .iter()
            .enumerate()
            .flat_map(|(index, section)| {
                let content = truncate_utf8(&section.body, limit);
                let element = if section.collapsible {
                    let mut header = CollapsiblePanelHeader::new(Text::plain(&section.title));
                    header.icon = Some(HeaderIcon::standard("right-small-ccm_outlined"));
                    header.icon_position = Some(IconPosition::Left);
                    header.icon_expanded_angle = Some(PanelIconExpandedAngle::Ninety);
                    let mut panel = CollapsiblePanel::new(header)
                        .element(Element::Markdown(Markdown::new(content)));
                    panel.element_id = Some(format!("turn_section_{index}"));
                    panel.expanded =
                        Some(section.expanded.unwrap_or(index + 1 == view.sections.len()));
                    panel.margin = Some("8px 0px 8px 0px".into());
                    Element::CollapsiblePanel(panel)
                } else if section.title.is_empty() {
                    Element::Markdown(Markdown::new(content))
                } else {
                    Element::Markdown(Markdown::new(format!("**{}**\n{content}", section.title)))
                };
                let mut elements = vec![element];
                for token in &section.action_tokens {
                    if let Some(action) = view.actions.iter().find(|action| &action.token == token)
                        && placed_actions.insert(token.as_str())
                    {
                        elements.push(render_action(action, actions_disabled));
                    }
                }
                elements
            })
            .collect()
    };
    let mut body = if view.status == ViewStatus::Background {
        let mut quote = elements.into_iter().fold(Column::new(), Column::element);
        quote.background_style = Some(BackgroundStyle::Color(Color::Grey));
        quote.padding = Some("12px".into());
        Body::new().element(Element::ColumnSet(ColumnSet::new().column(quote)))
    } else {
        elements.into_iter().fold(Body::new(), Body::element)
    };
    for action in &view.actions {
        if !placed_actions.contains(action.token.as_str()) {
            body = body.element(render_action(action, actions_disabled));
        }
    }
    body
}
