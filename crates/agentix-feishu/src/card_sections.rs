//! Render typed turn sections without interpreting arbitrary Markdown as controls.
use super::{CARD_BODY_LIMIT, truncate_utf8};
use agentix_domain::{OutboundView, ViewStatus};
use larksuite_oapi_sdk_rs::card::v2::{
    BackgroundStyle, Body, CollapsiblePanel, CollapsiblePanelHeader, Color, Column, ColumnSet,
    Element, Markdown, Text,
};

pub(super) fn view_body(view: &OutboundView) -> Body {
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
            .map(|(index, section)| {
                let content = truncate_utf8(&section.body, limit);
                if section.collapsible {
                    let mut panel = CollapsiblePanel::new(CollapsiblePanelHeader::new(
                        Text::plain(&section.title),
                    ))
                    .element(Element::Markdown(Markdown::new(content)));
                    panel.element_id = Some(format!("turn_section_{index}"));
                    panel.expanded = Some(false);
                    panel.margin = Some("8px 0px 8px 0px".into());
                    Element::CollapsiblePanel(panel)
                } else {
                    Element::Markdown(Markdown::new(format!("**{}**\n{content}", section.title)))
                }
            })
            .collect()
    };
    if view.status == ViewStatus::Background {
        let mut quote = elements.into_iter().fold(Column::new(), Column::element);
        quote.background_style = Some(BackgroundStyle::Color(Color::Grey));
        quote.padding = Some("12px".into());
        Body::new().element(Element::ColumnSet(ColumnSet::new().column(quote)))
    } else {
        elements.into_iter().fold(Body::new(), Body::element)
    }
}
