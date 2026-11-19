//! The cell inspector, drawn over the panels. Wrapped here, where the width is;
//! the line count returns through `Geometry`.

use ld_domain::contents::SelectionMode;
use ld_domain::inspect::Strip;
use ld_domain::selection::Ranges;
use ld_domain::style::Token;
use ld_domain::view::{EditView, InspectBody, InspectValue, InspectView};
use ld_domain::viewport::Span;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span as TextSpan};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use crate::body::Extent;
use crate::highlight::spans;
use crate::theme::Theme;

/// Past this a line is longer than the eye tracks back from.
const MAX_WIDTH: u16 = 100;
/// Two columns each side, two rows above and below.
const MARGIN: u16 = 4;
const MIN_WIDTH: u16 = 20;
/// The border and three rows of body.
const MIN_HEIGHT: u16 = 5;
const PAD: u16 = 1;
const GAP: usize = 2;

/// Sized from the frame alone, so the height bounding the offset never moves
/// with the value.
fn rect(area: Rect) -> Option<Rect> {
    let width = area.width.saturating_sub(MARGIN).min(MAX_WIDTH);
    let height = area.height.saturating_sub(MARGIN);

    if width < MIN_WIDTH || height < MIN_HEIGHT {
        return None;
    }

    Some(Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    ))
}

fn body(block: Rect) -> Rect {
    block.inner(Margin {
        horizontal: 1 + PAD,
        vertical: 1,
    })
}

/// Zero makes `Enter` inert.
pub(crate) fn visible_rows(area: Rect) -> usize {
    rect(area).map_or(0, |block| usize::from(body(block).height))
}

pub(crate) fn render(frame: &mut Frame, view: &InspectView, theme: Theme) -> (usize, Span) {
    let Some(area) = rect(frame.area()) else {
        return (0, Span::default());
    };
    let inner = body(area);
    let (lines, selection) = lay_out(view, usize::from(inner.width), theme);
    let total = lines.len();

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.focus_border())
        .title(crate::drop::title(&view.title, theme));
    if let Some(size) = &view.size {
        block = block.title_bottom(
            Line::from(TextSpan::styled(
                format!(" {size} "),
                theme.style(Token::Muted),
            ))
            .right_aligned(),
        );
    }

    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, theme.ground());
    frame.render_widget(block, area);

    let offset = u16::try_from(view.offset).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), inner);

    let extent = Extent {
        position: view.offset,
        total,
        visible: usize::from(inner.height),
    };
    if extent.overflows() {
        crate::render::scrollbar(frame, area, &extent, theme.focus_border(), theme);
    }

    if let Some(message) = &view.message {
        crate::overlay::render(frame, message, theme);
    }

    (total, selection)
}

fn lay_out(view: &InspectView, width: usize, theme: Theme) -> (Vec<Line<'static>>, Span) {
    let (body, cursor) = (&view.body, view.cursor);
    let search = view.search.as_ref();
    let mut laid = Laid::default();
    // Drawn in place of the value, so the block says one thing per cell.
    let typing = |at: usize| view.editing.as_ref().filter(|edit| edit.field == at);

    match body {
        InspectBody::Cell(value) => {
            if let Some(edit) = typing(0) {
                laid.field(edit, 0, width, theme);
                return (laid.lines, laid.selection);
            }
            let style = value_style(value, theme);

            for (at, rows) in wrapped(value, width, 0).into_iter().enumerate() {
                let mark = marks(view, at, at == cursor)[Strip::Value.index()];
                let rows = rows
                    .into_iter()
                    .map(|row| {
                        banded(
                            Vec::new(),
                            spans(&row, search, style, theme),
                            [None, mark],
                            width,
                            theme,
                        )
                    })
                    .collect();

                laid.push(rows, at == cursor);
            }
        }
        InspectBody::Row { names, fields } => {
            let names = usize::from(*names);
            let hang = names + GAP;
            let mut at = 0;

            for (index, field) in fields.iter().enumerate() {
                let style = value_style(&field.value, theme);
                let name = field
                    .token
                    .map_or_else(|| theme.style(Token::Muted), |token| theme.style(token));
                let pad = names.saturating_sub(field.name.width());

                if let Some(edit) = typing(index) {
                    let label = field.name.clone();
                    laid.field_under(
                        edit,
                        &format!("{label}{}", " ".repeat(pad + GAP)),
                        name,
                        hang,
                        width,
                        theme,
                    );
                    at += field.value.lines.len();
                    continue;
                }

                for (line, rows) in wrapped(&field.value, width, hang).into_iter().enumerate() {
                    let [name_mark, value_mark] = marks(view, index, at + line == cursor);
                    let rows = rows
                        .into_iter()
                        .enumerate()
                        .map(|(row, text)| {
                            let rest: String = text.chars().skip(hang).collect();
                            let mut label = if (line, row) == (0, 0) {
                                spans(&field.name, search, name, theme)
                            } else {
                                Vec::new()
                            };
                            let drawn = if (line, row) == (0, 0) {
                                field.name.width()
                            } else {
                                0
                            };
                            label.push(TextSpan::styled(" ".repeat(hang - drawn), name));

                            banded(
                                label,
                                spans(&rest, search, style, theme),
                                [name_mark, value_mark],
                                width,
                                theme,
                            )
                        })
                        .collect();

                    laid.push(rows, at + line == cursor);
                }

                at += field.value.lines.len();
            }
        }
    }

    (laid.lines, laid.selection)
}

/// Per strip, name first, as the grid draws its cursor in each mode: a row
/// bands the whole line, a cell its one strip under a crosshair, a column the
/// strip on every line.
fn marks(view: &InspectView, entry: usize, cursor: bool) -> [Option<Token>; 2] {
    let held = |strip: Strip| ranged(entry, strip.index(), &view.entries, &view.strips);

    [Strip::Name, Strip::Value].map(|strip| match view.mode {
        SelectionMode::Row if cursor => Some(Token::Selected),
        SelectionMode::Row => held(strip).then_some(Token::Ranged),
        SelectionMode::Cell | SelectionMode::Column if cursor && strip == view.strip => {
            Some(Token::SelectedCell)
        }
        SelectionMode::Column if strip == view.strip => Some(Token::Selected),
        SelectionMode::Cell | SelectionMode::Column if held(strip) => Some(Token::Ranged),
        SelectionMode::Cell | SelectionMode::Column => cursor.then_some(Token::Crosshair),
    })
}

/// As the grid decides a held cell: an empty axis constrains nothing, both
/// empty holds nothing.
fn ranged(entry: usize, strip: usize, entries: &Ranges, strips: &Ranges) -> bool {
    (!entries.is_empty() || !strips.is_empty())
        && (entries.is_empty() || entries.contains(entry))
        && (strips.is_empty() || strips.contains(strip))
}

/// The value strip's mark runs to the block's edge: a band that stopped at
/// the text would read as a match, not a cursor.
fn banded(
    name: Vec<TextSpan<'static>>,
    value: Vec<TextSpan<'static>>,
    [name_mark, value_mark]: [Option<Token>; 2],
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let mut spans = marked(name, name_mark, theme);
    spans.extend(marked(value, value_mark, theme));

    if let Some(mark) = value_mark {
        let used: usize = spans.iter().map(TextSpan::width).sum();
        spans.push(TextSpan::styled(
            " ".repeat(width.saturating_sub(used)),
            theme.style(mark),
        ));
    }

    Line::from(spans)
}

fn marked(
    spans: Vec<TextSpan<'static>>,
    mark: Option<Token>,
    theme: Theme,
) -> Vec<TextSpan<'static>> {
    let Some(mark) = mark else {
        return spans;
    };
    let band = theme.style(mark);

    spans
        .into_iter()
        .map(|span| {
            let style = span.style.patch(band);
            span.style(style)
        })
        .collect()
}

#[derive(Default)]
struct Laid {
    lines: Vec<Line<'static>>,
    selection: Span,
}

impl Laid {
    /// The text on the selection band with a caret; the parse error below it.
    fn field(&mut self, edit: &EditView, hang: usize, width: usize, theme: Theme) {
        self.field_under(
            edit,
            &" ".repeat(hang),
            theme.style(Token::Muted),
            hang,
            width,
            theme,
        );
    }

    fn field_under(
        &mut self,
        edit: &EditView,
        label: &str,
        label_style: Style,
        hang: usize,
        width: usize,
        theme: Theme,
    ) {
        let room = width.saturating_sub(hang).max(1);
        // The tail: the caret and the latest typing stay on screen.
        let shown: String = edit
            .text
            .chars()
            .skip(edit.cursor.saturating_sub(room.saturating_sub(1)))
            .collect();
        let typed = format!("{shown}▏");

        self.selection = Span {
            first: self.lines.len(),
            lines: 1,
        };
        self.lines.push(Line::from(vec![
            TextSpan::styled(label.to_owned(), label_style),
            TextSpan::styled(typed, theme.style(Token::Selected)),
        ]));

        if let Some(error) = &edit.error {
            self.lines.push(Line::from(vec![
                TextSpan::raw(" ".repeat(hang)),
                TextSpan::styled(
                    error.clone(),
                    theme.style(Token::Severity(ld_domain::style::Severity::Danger)),
                ),
            ]));
            self.selection.lines = 2;
        }
    }

    /// The cursor's line, every row it wrapped to, is what the offset follows.
    fn push(&mut self, rows: Vec<Line<'static>>, cursor: bool) {
        if cursor {
            self.selection = Span {
                first: self.lines.len(),
                lines: rows.len(),
            };
        }
        self.lines.extend(rows);
    }
}

/// Dirty outranks the grid's token.
fn value_style(value: &InspectValue, theme: Theme) -> Style {
    if value.dirty {
        return theme.style(Token::Dirty);
    }

    value
        .token
        .map_or_else(Style::default, |token| theme.style(token))
}

/// Grouped by source line, which the cursor counts. A blank line still takes a
/// row, or the field would lose its name.
fn wrapped(value: &InspectValue, width: usize, hang: usize) -> Vec<Vec<String>> {
    value
        .lines
        .iter()
        .map(|line| {
            let text = line.trim_start_matches(' ');
            let indent = (hang + line.len() - text.len()).min(width.saturating_sub(1));
            let rows = crate::wrap::plain(text, width, indent);

            if rows.is_empty() {
                vec![" ".repeat(hang.min(width))]
            } else {
                rows
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ld_domain::contents::{Subject, TableRef};
    use ld_domain::inspect::{Inspected, Inspector};
    use ld_domain::model::Value;
    use ld_domain::overlay::Overlay;
    use ld_domain::state::State;
    use ld_domain::view::{OverlayView, view_model};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};

    use super::*;

    fn drawn(width: u16, height: u16, inspected: Inspected) -> (Buffer, usize) {
        let (buffer, lines, _) = drawn_at(width, height, inspected, 0);

        (buffer, lines)
    }

    fn drawn_at(
        width: u16,
        height: u16,
        inspected: Inspected,
        cursor: usize,
    ) -> (Buffer, usize, Span) {
        let mut state = State::default();
        let mut inspector = Inspector::opened(
            0,
            Subject::Table(TableRef::new("shop", "orders")),
            inspected,
        );
        for _ in 0..cursor {
            inspector.down();
        }
        state.overlay = Some(Overlay::Inspect(Box::new(inspector)));
        let Some(OverlayView::Inspect(view)) = view_model(&state).overlay else {
            panic!("the inspector projects");
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a backend");
        let mut drawn = (0, Span::default());

        terminal
            .draw(|frame| drawn = render(frame, &view, Theme::default()))
            .expect("TestBackend always draws");

        (terminal.backend().buffer().clone(), drawn.0, drawn.1)
    }

    fn banded(buffer: &Buffer) -> Vec<u16> {
        let band = Theme::default().style(Token::Selected);

        (0..buffer.area.height)
            .filter(|&y| {
                buffer
                    .cell((buffer.area.width / 2, y))
                    .is_some_and(|cell| cell.style().bg == band.bg)
            })
            .collect()
    }

    fn rows(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(Cell::symbol)
                    .collect()
            })
            .collect()
    }

    fn cell(value: Value) -> Inspected {
        Inspected::Cell {
            at: 0,
            column: "spec".to_owned(),
            value,
        }
    }

    #[test]
    fn a_cell_is_drawn_laid_out_under_its_title() {
        let (buffer, lines) = drawn(
            40,
            12,
            cell(Value::Text(r#"{"a":1,"b":[true]}"#.to_owned())),
        );

        assert_eq!(
            rows(&buffer)[2..10],
            [
                "  ╭shop.orders · spec────────────────╮  ",
                "  │ {                                │  ",
                "  │   \"a\": 1,                        │  ",
                "  │   \"b\": [                         │  ",
                "  │     true                         │  ",
                "  │   ]                              │  ",
                "  │ }                                │  ",
                "  ╰──────────────────────────── 18 B ╯  ",
            ]
            .map(str::to_owned)
        );
        assert_eq!(lines, 6);
    }

    #[test]
    fn a_row_is_drawn_one_field_per_column() {
        let (buffer, _) = drawn(
            40,
            12,
            Inspected::Row {
                number: 4,
                columns: vec!["id".to_owned(), "tags".to_owned()],
                values: vec![Value::Int(7), Value::Array("{x}".to_owned())],
            },
        );

        assert_eq!(
            rows(&buffer)[2..9],
            [
                "  ╭shop.orders · row 4───────────────╮  ",
                "  │ id    7                          │  ",
                "  │ tags  {                          │  ",
                "  │         x                        │  ",
                "  │       }                          │  ",
                "  │                                  │  ",
                "  │                                  │  ",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn a_long_value_wraps_under_its_own_indent() {
        let (buffer, lines) = drawn(
            30,
            12,
            Inspected::Row {
                number: 1,
                columns: vec!["note".to_owned()],
                values: vec![Value::Array(
                    r#"{"one two three four five six"}"#.to_owned(),
                )],
            },
        );

        assert_eq!(
            rows(&buffer)[3..8],
            [
                "  │ note  {                │  ",
                "  │         one two three  │  ",
                "  │         four five six  │  ",
                "  │       }                │  ",
                "  │                        │  ",
            ]
            .map(str::to_owned)
        );
        assert_eq!(lines, 4);
    }

    #[test]
    fn a_value_longer_than_the_body_gets_a_scrollbar() {
        let value = Value::Array(format!(
            "{{{}}}",
            (0..40).map(|n| n.to_string()).collect::<Vec<_>>().join(",")
        ));

        let (buffer, lines) = drawn(40, 12, cell(value));

        assert_eq!(lines, 42);
        assert!(
            rows(&buffer).iter().any(|row| row.contains('▐')),
            "{:#?}",
            rows(&buffer)
        );
    }

    #[test]
    fn the_frame_reports_the_body_height_and_the_wrapped_lines() {
        let mut state = State::default();
        state.overlay = Some(Overlay::Inspect(Box::new(Inspector::opened(
            0,
            Subject::Table(TableRef::new("shop", "orders")),
            cell(Value::Array("{1,2}".to_owned())),
        ))));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("a backend");
        let mut geometry = None;

        terminal
            .draw(|frame| {
                geometry = Some(
                    crate::render::render(frame, &view_model(&state), Theme::default()).geometry,
                );
            })
            .expect("TestBackend always draws");

        let geometry = geometry.expect("drawn");
        assert_eq!(geometry.inspector_rows(), 18);
        assert_eq!(geometry.inspector_lines(), 4);
    }

    #[test]
    fn the_cursor_line_is_banded_across_every_row_it_wrapped_to() {
        let (buffer, _, selection) = drawn_at(
            30,
            12,
            Inspected::Row {
                number: 1,
                columns: vec!["note".to_owned()],
                values: vec![Value::Array(
                    r#"{"one two three four five six"}"#.to_owned(),
                )],
            },
            1,
        );

        assert_eq!(banded(&buffer), [4, 5], "the element, both of its rows");
        assert_eq!(selection, Span { first: 1, lines: 2 });
    }

    #[test]
    fn a_frame_too_small_for_the_block_draws_none_and_reports_no_room() {
        let area = Rect::new(0, 0, 23, 8);

        assert_eq!(rect(area), None);
        assert_eq!(visible_rows(area), 0);
        assert_eq!(visible_rows(Rect::new(0, 0, 24, 9)), 3);
    }

    #[test]
    fn the_block_is_sized_by_the_frame_and_capped_in_width() {
        assert_eq!(rect(Rect::new(0, 0, 80, 24)), Some(Rect::new(2, 2, 76, 20)));
        assert_eq!(
            rect(Rect::new(0, 0, 200, 50)),
            Some(Rect::new(50, 2, 100, 46))
        );
    }

    #[test]
    fn an_open_field_is_drawn_in_place_of_the_value_it_types_over() {
        let mut state = State::default();
        let mut inspector = Inspector::opened(
            0,
            Subject::Table(TableRef::new("shop", "orders")),
            cell(Value::Text("ada".to_owned())),
        );
        let mut field = ld_domain::edit::Field::opened(
            ld_domain::Engine::Postgres,
            0,
            "spec",
            "integer",
            "ada".to_owned(),
        );
        field.line_mut().insert('!');
        inspector.start_edit(field);
        state.overlay = Some(Overlay::Inspect(Box::new(inspector)));
        let Some(OverlayView::Inspect(view)) = view_model(&state).overlay else {
            panic!("the inspector projects");
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("a backend");
        terminal
            .draw(|frame| {
                render(frame, &view, Theme::default());
            })
            .expect("TestBackend always draws");

        let drawn = rows(terminal.backend().buffer());
        assert!(
            drawn.iter().any(|row| row.contains("ada!▏")),
            "the field and its caret: {drawn:#?}"
        );
        assert!(
            drawn.iter().any(|row| row.contains("Not a valid integer.")),
            "the reason it does not parse: {drawn:#?}"
        );
    }

    fn order() -> Inspected {
        Inspected::Row {
            number: 1,
            columns: vec!["id".to_owned(), "note".to_owned()],
            values: vec![Value::Int(7), Value::Text("blue".to_owned())],
        }
    }

    fn drawn_with(width: u16, height: u16, inspector: Inspector) -> Buffer {
        let mut state = State::default();
        state.overlay = Some(Overlay::Inspect(Box::new(inspector)));
        let Some(OverlayView::Inspect(view)) = view_model(&state).overlay else {
            panic!("the inspector projects");
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a backend");
        terminal
            .draw(|frame| {
                render(frame, &view, Theme::default());
            })
            .expect("TestBackend always draws");

        terminal.backend().buffer().clone()
    }

    fn opened(inspected: Inspected) -> Inspector {
        Inspector::opened(
            0,
            Subject::Table(TableRef::new("shop", "orders")),
            inspected,
        )
    }

    fn styled_at(buffer: &Buffer, needle: &str) -> Style {
        let rows = rows(buffer);
        let (y, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} drawn: {rows:#?}"));
        let x = row[..row.find(needle).expect("found")].chars().count();

        buffer
            .cell((
                u16::try_from(x).expect("narrow"),
                u16::try_from(y).expect("short"),
            ))
            .expect("in the buffer")
            .style()
    }

    #[test]
    fn a_hit_wears_the_match_colour_in_a_name_and_in_a_value() {
        let mut inspector = opened(order());
        inspector.open_search();
        for character in "e".chars() {
            inspector.search_push(character);
        }

        let buffer = drawn_with(40, 12, inspector);

        let hit = Theme::default().style(Token::Match).fg;
        assert_eq!(styled_at(&buffer, "e  blue").fg, hit, "the name's e");
        assert_eq!(styled_at(&buffer, "e   ").fg, hit, "the value's e");
        assert_ne!(styled_at(&buffer, "blu").fg, hit, "only the hit");
    }

    #[test]
    fn cell_mode_bands_the_cursors_strip_and_not_its_neighbour() {
        let mut inspector = opened(order());
        inspector.cycle_mode();
        inspector.cycle_mode();
        inspector.to_strip(ld_domain::inspect::Strip::Name);

        let buffer = drawn_with(40, 12, inspector);

        let cell = Theme::default().style(Token::SelectedCell);
        assert!(
            styled_at(&buffer, "id")
                .add_modifier
                .contains(cell.add_modifier),
            "the name under the cursor"
        );
        assert!(
            !styled_at(&buffer, "7")
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED),
            "its value is only crosshaired"
        );
    }

    #[test]
    fn column_mode_bands_the_strip_on_every_line() {
        let mut inspector = opened(order());
        inspector.cycle_mode();

        let buffer = drawn_with(40, 12, inspector);

        let band = Theme::default().style(Token::Selected).bg;
        assert_eq!(styled_at(&buffer, "blue").bg, band, "a line off the cursor");
        assert_ne!(styled_at(&buffer, "note").bg, band, "the name strip");
    }

    #[test]
    fn a_stacked_message_is_drawn_over_the_block() {
        let mut inspector = opened(order());
        inspector.raise(ld_domain::overlay::Message {
            title: "Not editable".to_owned(),
            message: "id is part of the primary key.".to_owned(),
            severity: ld_domain::style::Severity::Warning,
        });

        let drawn = rows(&drawn_with(60, 16, inspector));

        assert!(
            drawn.iter().any(|row| row.contains("Not editable")),
            "{drawn:#?}"
        );
        assert!(
            drawn.iter().any(|row| row.contains("shop.orders · row 1")),
            "the inspector stays under it: {drawn:#?}"
        );
    }

    #[test]
    fn a_dirty_field_is_drawn_in_the_uncommitted_colour() {
        let mut state = State::default();
        state.overlay = Some(Overlay::Inspect(Box::new(
            Inspector::opened(
                0,
                Subject::Table(TableRef::new("shop", "orders")),
                cell(Value::Text("ada".to_owned())),
            )
            .with_edited(vec![0]),
        )));
        let Some(OverlayView::Inspect(view)) = view_model(&state).overlay else {
            panic!("the inspector projects");
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("a backend");
        terminal
            .draw(|frame| {
                render(frame, &view, Theme::default());
            })
            .expect("TestBackend always draws");

        let theme = Theme::default();
        let buffer = terminal.backend().buffer();
        let dirty = theme.style(Token::Dirty);
        assert!(
            (0..buffer.area.height).any(|y| (0..buffer.area.width).any(|x| {
                buffer
                    .cell((x, y))
                    .is_some_and(|cell| cell.symbol() == "a" && cell.style().fg == dirty.fg)
            })),
            "the value wears Token::Dirty"
        );
    }
}
