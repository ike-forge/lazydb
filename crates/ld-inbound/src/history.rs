//! The query history picker: the query on top, the rows under it, the layers
//! along the bottom border. Unlike the other pickers it scrolls: the history
//! can outgrow any frame.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use ld_domain::history::Layer;
use ld_domain::style::Token;
use ld_domain::view::{HistoryRow, HistoryView};

use crate::theme::Theme;

const MIN_WIDTH: u16 = 30;
const MAX_WIDTH: u16 = 120;
const PAD: u16 = 1;
/// Two borders and the query line.
const CHROME: u16 = 3;
const PLACE_WIDTH: usize = 16;
const NAME_WIDTH: usize = 24;
const PROMPT: &str = "❯ ";
const ELLIPSIS: char = '…';

/// Zero gates `ctrl-r`.
pub(crate) fn visible_rows(area: Rect) -> usize {
    if area.width < MIN_WIDTH || area.height < CHROME + 1 {
        return 0;
    }

    usize::from(area.height - CHROME)
}

fn rect(area: Rect, rows: usize) -> Option<Rect> {
    let visible = visible_rows(area);

    if visible == 0 {
        return None;
    }

    let height = u16::try_from(rows.clamp(1, visible)).unwrap_or(u16::MAX) + CHROME;
    let width = area.width.saturating_sub(4).clamp(MIN_WIDTH, MAX_WIDTH);

    Some(Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    ))
}

/// Cut at `width` characters with an ellipsis; the byte length kept before it.
fn clipped(text: &str, width: usize) -> (String, usize) {
    if text.chars().count() <= width {
        return (text.to_owned(), text.len());
    }

    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    let length = kept.len();
    let mut cut = kept;
    cut.push(ELLIPSIS);
    (cut, length)
}

fn padded(text: &str, width: usize) -> String {
    let (cut, _) = clipped(text, width);
    let fill = width.saturating_sub(cut.width());

    format!("{cut}{}", " ".repeat(fill))
}

pub(crate) fn render(frame: &mut Frame, view: &HistoryView, theme: Theme) {
    let Some(area) = rect(frame.area(), view.total) else {
        return;
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.focus_border())
        .title("History")
        .title_bottom(layers(view, theme));
    let inner = block.inner(area).inner(Margin {
        horizontal: PAD,
        vertical: 0,
    });

    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, theme.ground());
    frame.render_widget(block, area);

    let query_line = Rect::new(inner.x, inner.y, inner.width, 1);
    let list = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );

    render_query(frame, query_line, view, theme);

    if view.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no statement matches",
                theme.style(Token::Faint),
            )),
            list,
        );
        return;
    }

    let age_width = view
        .rows
        .iter()
        .map(|row| row.age.width())
        .max()
        .unwrap_or(0);
    let place_width = view
        .rows
        .iter()
        .map(|row| row.place.width())
        .max()
        .unwrap_or(0)
        .min(PLACE_WIDTH);
    let name_width = view
        .rows
        .iter()
        .map(|row| row.name.width())
        .max()
        .unwrap_or(0)
        .min(NAME_WIDTH);
    let widths = Widths {
        age: age_width,
        place: place_width,
        name: name_width,
        row: usize::from(list.width),
    };
    let items: Vec<ListItem> = view
        .rows
        .iter()
        .map(|row| item(row, widths, theme))
        .collect();
    let mut state = ListState::default().with_selected(Some(view.selected));

    frame.render_stateful_widget(
        List::new(items).highlight_style(theme.style(Token::Selected)),
        list,
        &mut state,
    );
}

/// The caret stays on screen: a query wider than the line scrolls left.
fn render_query(frame: &mut Frame, area: Rect, view: &HistoryView, theme: Theme) {
    let room = usize::from(area.width)
        .saturating_sub(PROMPT.width())
        .saturating_sub(1);
    let skipped = view.caret.saturating_sub(room);
    let shown: String = view.query.chars().skip(skipped).take(room).collect();
    let before_caret: String = view
        .query
        .chars()
        .skip(skipped)
        .take(view.caret - skipped)
        .collect();

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(PROMPT, theme.style(Token::Faint)),
            Span::raw(shown),
        ])),
        area,
    );

    let caret = u16::try_from(PROMPT.width() + before_caret.width()).unwrap_or(area.width);
    frame.set_cursor_position(((area.x + caret).min(area.right().saturating_sub(1)), area.y));
}

/// Each column's, widest row's; zero leaves the column out.
#[derive(Clone, Copy)]
struct Widths {
    age: usize,
    place: usize,
    name: usize,
    row: usize,
}

/// `kept` bytes of `ranges`, which index the uncut text.
fn within(ranges: &[Range<usize>], kept: usize) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter(|range| range.start < kept)
        .map(|range| range.start..range.end.min(kept))
        .collect()
}

fn item(row: &HistoryRow, widths: Widths, theme: Theme) -> ListItem<'static> {
    let faint = theme.style(Token::Faint);
    let mut spans = Vec::new();
    let mut used = 0;

    if widths.age > 0 {
        spans.push(Span::styled(
            format!("{:>1$}  ", row.age, widths.age),
            faint,
        ));
        used += widths.age + 2;
    }
    if widths.place > 0 {
        spans.push(Span::styled(
            format!("{}  ", padded(&row.place, widths.place)),
            faint,
        ));
        used += widths.place + 2;
    }
    if widths.name > 0 {
        let (name, kept) = clipped(&row.name, widths.name);
        let fill = widths.name.saturating_sub(name.width()) + 2;
        spans.extend(crate::highlight::ranged(
            &name,
            &within(&row.name_matched, kept),
            theme.style(Token::Section),
            theme,
        ));
        spans.push(Span::raw(" ".repeat(fill)));
        used += widths.name + 2;
    }

    let (statement, kept) = clipped(&row.statement, widths.row.saturating_sub(used));
    spans.extend(crate::highlight::ranged(
        &statement,
        &within(&row.matched, kept),
        Style::default(),
        theme,
    ));

    ListItem::new(Line::from(spans))
}

/// The current layer stands out; the others say where `ctrl-r` goes next.
fn layers(view: &HistoryView, theme: Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];

    for (layer, name) in Layer::ALL.into_iter().zip(&view.layers) {
        if layer != Layer::Default {
            spans.push(Span::styled(" · ", theme.style(Token::Faint)));
        }
        let style = if layer == view.layer {
            theme.style(Token::Selected)
        } else {
            theme.style(Token::Faint)
        };
        spans.push(Span::styled(name.clone(), style));
    }

    spans.push(Span::raw(" "));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use ld_domain::ThemeChoice;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    use super::*;

    fn view(rows: Vec<HistoryRow>, selected: usize) -> HistoryView {
        HistoryView {
            total: rows.len(),
            rows,
            selected,
            layer: Layer::Default,
            layers: [
                "default".to_owned(),
                "saved".to_owned(),
                "prod".to_owned(),
                "PostgreSQL".to_owned(),
                "all".to_owned(),
            ],
            query: String::new(),
            caret: 0,
        }
    }

    fn row(statement: &str) -> HistoryRow {
        HistoryRow {
            age: "3h".to_owned(),
            place: String::new(),
            name: String::new(),
            name_matched: Vec::new(),
            statement: statement.to_owned(),
            matched: Vec::new(),
        }
    }

    fn drawn(width: u16, height: u16, view: &HistoryView) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a backend");
        let theme = Theme::of(&ThemeChoice::default());

        terminal
            .draw(|frame| render(frame, view, theme))
            .expect("a frame");

        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|line| line.iter().map(Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn numbered(count: usize) -> Vec<HistoryRow> {
        (0..count)
            .map(|n| row(&format!("SELECT {n} FROM t;")))
            .collect()
    }

    #[test]
    fn the_block_lists_the_rows_under_its_title() {
        let frame = drawn(80, 24, &view(numbered(3), 0));

        assert!(frame.contains("History"));
        for n in 0..3 {
            assert!(frame.contains(&format!("SELECT {n} FROM t;")), "row {n}");
        }
    }

    #[test]
    fn each_row_leads_with_its_age() {
        let frame = drawn(80, 24, &view(numbered(1), 0));

        assert!(frame.contains("3h  SELECT 0 FROM t;"), "{frame}");
    }

    #[test]
    fn a_wider_layer_names_where_each_ran() {
        let mut rows = numbered(2);
        rows[0].place = "prod".to_owned();
        rows[1].place = "staging".to_owned();

        let frame = drawn(80, 24, &view(rows, 0));

        assert!(frame.contains("3h  prod     SELECT 0 FROM t;"), "{frame}");
        assert!(frame.contains("3h  staging  SELECT 1 FROM t;"), "{frame}");
    }

    #[test]
    fn the_bottom_border_names_every_layer() {
        let frame = drawn(80, 24, &view(numbered(1), 0));

        assert!(
            frame.contains("default · saved · prod · PostgreSQL · all"),
            "{frame}"
        );
    }

    #[test]
    fn a_saved_query_is_named_in_its_own_column() {
        let mut rows = numbered(2);
        rows[0].name = "pending".to_owned();

        let frame = drawn(80, 24, &view(rows, 0));

        assert!(frame.contains("3h  pending  SELECT 0 FROM t;"), "{frame}");
        assert!(frame.contains("3h           SELECT 1 FROM t;"), "{frame}");
    }

    #[test]
    fn a_long_name_is_cut_with_an_ellipsis() {
        let mut rows = numbered(1);
        rows[0].name = "n".repeat(NAME_WIDTH + 10);

        let frame = drawn(80, 24, &view(rows, 0));

        assert!(
            frame.contains(&format!("{}…  SELECT 0", "n".repeat(NAME_WIDTH - 1))),
            "{frame}"
        );
    }

    #[test]
    fn the_query_is_drawn_after_the_prompt() {
        let mut typed = view(numbered(1), 0);
        typed.query = "orders".to_owned();
        typed.caret = 6;

        let frame = drawn(80, 24, &typed);

        assert!(frame.contains("❯ orders"), "{frame}");
    }

    #[test]
    fn a_list_longer_than_the_frame_scrolls_to_the_cursor() {
        let frame = drawn(80, 12, &view(numbered(100), 80));

        assert!(frame.contains("SELECT 80 FROM t;"));
        assert!(!frame.contains("SELECT 0 FROM t;"));
    }

    #[test]
    fn a_statement_wider_than_the_block_is_cut_with_an_ellipsis() {
        let long = format!("SELECT {} FROM t;", "column, ".repeat(40));
        let frame = drawn(60, 12, &view(vec![row(&long)], 0));

        assert!(frame.contains('…'));
        assert!(!frame.contains("FROM t;"));
    }

    #[test]
    fn a_query_that_matched_nothing_says_so() {
        let mut empty = view(Vec::new(), 0);
        empty.total = 3;

        let frame = drawn(80, 24, &empty);

        assert!(frame.contains("no statement matches"));
    }

    #[test]
    fn the_block_keeps_its_height_while_the_query_narrows_it() {
        let all = view(numbered(5), 0);
        let mut narrowed = view(numbered(1), 0);
        narrowed.total = 5;

        let height = |frame: String| frame.lines().filter(|line| line.contains('│')).count();

        assert_eq!(
            height(drawn(80, 24, &all)),
            height(drawn(80, 24, &narrowed))
        );
    }

    #[test]
    fn a_frame_too_small_to_hold_it_draws_nothing() {
        let frame = drawn(MIN_WIDTH - 1, 24, &view(numbered(3), 0));

        assert!(!frame.contains("History"));
    }

    #[test]
    fn visible_rows_is_the_frame_less_the_borders_and_the_query() {
        assert_eq!(visible_rows(Rect::new(0, 0, 80, 24)), 21);
        assert_eq!(visible_rows(Rect::new(0, 0, 80, 3)), 0);
    }
}
