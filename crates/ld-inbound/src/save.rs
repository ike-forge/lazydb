//! The save form: the name, the statement it names, and the overwrite question
//! once `⏎` finds the name taken.

use ld_domain::model::Key;
use ld_domain::pointer::Press;
use ld_domain::style::{Severity, Token};
use ld_domain::view::SaveView;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use crate::drop::{Button, keys};
use crate::hit::Modal;
use crate::theme::Theme;

const MIN_WIDTH: u16 = 44;
const MAX_WIDTH: u16 = 100;
/// Past this the head is kept and the rest counted.
const MAX_SQL_LINES: usize = 8;
/// Borders, a blank, the name, a blank, a blank under the statement, the keys.
const CHROME_HEIGHT: u16 = 7;
/// The question and the blank under it.
const QUESTION_HEIGHT: u16 = 2;
const PROMPT: &str = "Name ❯ ";
const ELLIPSIS: char = '…';

/// Measured against the tallest form: the gate is asked before the statement
/// is known.
pub(crate) fn fits(area: Rect) -> bool {
    let tallest =
        CHROME_HEIGHT + QUESTION_HEIGHT + u16::try_from(MAX_SQL_LINES).unwrap_or(u16::MAX);

    area.width >= MIN_WIDTH && area.height >= tallest
}

fn statement(view: &SaveView) -> Vec<String> {
    if view.sql.len() <= MAX_SQL_LINES {
        return view.sql.clone();
    }

    let head = MAX_SQL_LINES - 1;
    let mut lines: Vec<String> = view.sql.iter().take(head).cloned().collect();
    lines.push(format!("… and {} more lines", view.sql.len() - head));
    lines
}

fn question(view: &SaveView, taken: &str) -> String {
    format!("{taken} already exists for {}, override?", view.connection)
}

/// Cut at `width` characters with an ellipsis.
fn clipped(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }

    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push(ELLIPSIS);
    cut
}

fn width(view: &SaveView, sql: &[String], frame_width: u16) -> u16 {
    let longest = sql
        .iter()
        .map(|line| line.width())
        .chain(view.taken.iter().map(|taken| question(view, taken).width()))
        .chain([PROMPT.width() + view.name.width()])
        .max()
        .unwrap_or_default();
    let needed = u16::try_from(longest + 4).unwrap_or(u16::MAX);

    needed.clamp(MIN_WIDTH, MAX_WIDTH).min(frame_width)
}

pub(crate) fn render(
    frame: &mut Frame,
    view: &SaveView,
    pressed: Option<Press>,
    theme: Theme,
) -> Option<Modal> {
    if !fits(frame.area()) {
        return None;
    }

    let sql = statement(view);
    let height = CHROME_HEIGHT
        + u16::try_from(sql.len()).unwrap_or(u16::MAX)
        + if view.taken.is_some() {
            QUESTION_HEIGHT
        } else {
            0
        };
    let area = frame.area();
    let width = width(view, &sql, area.width);
    let area = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.focus_border())
        .title(TextLine::from(vec![
            Span::raw("Save query for "),
            Span::styled(view.connection.clone(), theme.style(Token::Section)),
        ]));
    let inner = block.inner(area);
    let room = usize::from(inner.width);

    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, theme.ground());
    frame.render_widget(block, area);

    let name_room = room.saturating_sub(PROMPT.width()).saturating_sub(1);
    let skipped = view.caret.saturating_sub(name_room);
    let shown: String = view.name.chars().skip(skipped).take(name_room).collect();

    let mut lines = vec![
        TextLine::default(),
        TextLine::from(vec![
            Span::styled(PROMPT, theme.style(Token::Faint)),
            Span::styled(shown, theme.style(Token::Selected)),
        ]),
        TextLine::default(),
    ];
    lines.extend(sql.iter().map(|line| {
        TextLine::from(Span::styled(
            clipped(line, room),
            theme.style(Token::Keyword),
        ))
    }));
    lines.push(TextLine::default());

    let buttons = match &view.taken {
        Some(taken) => {
            lines.push(TextLine::from(Span::styled(
                clipped(&question(view, taken), room),
                theme.style(Token::Severity(Severity::Warning)),
            )));
            lines.push(TextLine::default());
            keys(
                &[
                    Button::new(Key::Char('y'), "Overwrite", true),
                    Button::new(Key::Char('n'), "Rename", true),
                    Button::new(Key::Escape, "Cancel", true),
                ],
                pressed,
                theme,
            )
        }
        None => keys(
            &[
                Button::new(Key::Enter, "Save", view.valid),
                Button::new(Key::Escape, "Cancel", true),
            ],
            pressed,
            theme,
        ),
    };
    let row = Rect::new(
        inner.x,
        inner.y + u16::try_from(lines.len()).unwrap_or(u16::MAX),
        inner.width,
        1,
    );
    lines.push(buttons.line());

    frame.render_widget(Paragraph::new(lines), inner);

    if view.taken.is_none() {
        let before_caret: String = view
            .name
            .chars()
            .skip(skipped)
            .take(view.caret - skipped)
            .collect();
        let caret = u16::try_from(PROMPT.width() + before_caret.width()).unwrap_or(inner.width);
        frame.set_cursor_position((
            (inner.x + caret).min(inner.right().saturating_sub(1)),
            inner.y + 1,
        ));
    }

    Some(Modal::new(area, buttons.at(row)))
}

#[cfg(test)]
mod tests {
    use ld_domain::ThemeChoice;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    use super::*;

    fn view(sql: &[&str]) -> SaveView {
        SaveView {
            connection: "prod".to_owned(),
            name: "pending".to_owned(),
            caret: 7,
            sql: sql.iter().map(|line| (*line).to_owned()).collect(),
            valid: true,
            taken: None,
        }
    }

    fn drawn(width: u16, height: u16, view: &SaveView) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a backend");
        let theme = Theme::of(&ThemeChoice::default());

        terminal
            .draw(|frame| {
                render(frame, view, None, theme);
            })
            .expect("a frame");

        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width))
            .map(|line| line.iter().map(Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_form_shows_the_connection_the_name_and_the_statement() {
        let frame = drawn(80, 24, &view(&["SELECT *", "FROM orders;"]));

        assert!(frame.contains("Save query for prod"), "{frame}");
        assert!(frame.contains("Name ❯ pending"), "{frame}");
        assert!(frame.contains("SELECT *"), "{frame}");
        assert!(frame.contains("FROM orders;"), "{frame}");
        assert!(frame.contains("⏎ Save"), "{frame}");
    }

    #[test]
    fn a_taken_name_asks_before_it_overwrites() {
        let mut taken = view(&["SELECT 1;"]);
        taken.taken = Some("pending".to_owned());

        let frame = drawn(80, 24, &taken);

        assert!(
            frame.contains("pending already exists for prod, override?"),
            "{frame}"
        );
        assert!(frame.contains("y Overwrite"), "{frame}");
        assert!(frame.contains("n Rename"), "{frame}");
    }

    #[test]
    fn a_long_statement_keeps_its_head_and_counts_the_rest() {
        let lines: Vec<String> = (0..20).map(|n| format!("  , column_{n}")).collect();
        let sql: Vec<&str> = lines.iter().map(String::as_str).collect();

        let frame = drawn(80, 24, &view(&sql));

        assert!(frame.contains("column_6"), "{frame}");
        assert!(!frame.contains("column_7"), "{frame}");
        assert!(frame.contains("… and 13 more lines"), "{frame}");
    }

    #[test]
    fn a_frame_too_small_draws_nothing() {
        let frame = drawn(MIN_WIDTH - 1, 24, &view(&["SELECT 1;"]));

        assert!(!frame.contains("Save query"));
    }
}
